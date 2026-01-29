mod audit;
mod balance;
mod config;
mod engine;
mod idempotency;
mod kill_switch;
mod polymarket_executor;
mod rate_limiter;

use anyhow::Result;
use arbees_rust_core::models::{
    channels, ExecutionRequest, ExecutionResult, ExecutionStatus, NotificationEvent,
    NotificationPriority, NotificationType, TransportMode,
};
use arbees_rust_core::redis::bus::RedisBus;
use balance::{start_balance_refresh_loop, DailyPnlTracker};
use chrono::Utc;
use config::SafeguardConfig;
use dotenv::dotenv;
use engine::ExecutionEngine;
use futures_util::StreamExt;
use idempotency::start_cleanup_task;
use kill_switch::{KillSwitch, KillSwitchReason};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

/// ZMQ message envelope format (matches game_shard)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ZmqEnvelope {
    seq: u64,
    timestamp_ms: i64,
    source: Option<String>,
    payload: serde_json::Value,
}

/// Statistics for monitoring ZMQ message processing
struct ZmqStats {
    messages_received: AtomicU64,
    messages_processed: AtomicU64,
    parse_errors: AtomicU64,
    trades_published: AtomicU64,
}

impl ZmqStats {
    fn new() -> Self {
        Self {
            messages_received: AtomicU64::new(0),
            messages_processed: AtomicU64::new(0),
            parse_errors: AtomicU64::new(0),
            trades_published: AtomicU64::new(0),
        }
    }
}

/// ZMQ Publisher for trade results (port 5560)
/// Enables zmq_listener to observe complete pipeline
struct TradePublisher {
    socket: RwLock<Option<PubSocket>>,
    seq: AtomicU64,
    enabled: bool,
}

impl TradePublisher {
    async fn new(port: u16, enabled: bool) -> Result<Self> {
        let socket = if enabled {
            let mut pub_socket = PubSocket::new();
            let addr = format!("tcp://0.0.0.0:{}", port);
            pub_socket.bind(&addr).await?;
            info!("ZMQ PUB socket bound to {} for trade results", addr);
            Some(pub_socket)
        } else {
            None
        };

        Ok(Self {
            socket: RwLock::new(socket),
            seq: AtomicU64::new(0),
            enabled,
        })
    }

    async fn publish_trade(&self, result: &ExecutionResult, stats: &ZmqStats) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let socket_guard = self.socket.read().await;
        let socket = match socket_guard.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };

        // Determine topic based on status
        let status_str = match result.status {
            ExecutionStatus::Filled => "executed",
            ExecutionStatus::PartialFill => "partial",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Rejected => "rejected",
            ExecutionStatus::Pending => "pending",
            ExecutionStatus::Cancelled => "cancelled",
        };
        let topic = format!("trades.{}.{}", status_str, result.request_id);

        // Build envelope
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let envelope = ZmqEnvelope {
            seq,
            timestamp_ms: Utc::now().timestamp_millis(),
            source: Some("execution_service".to_string()),
            payload: serde_json::to_value(result)?,
        };

        // Send multipart message
        let topic_bytes = topic.as_bytes().to_vec();
        let payload_bytes = serde_json::to_vec(&envelope)?;

        let mut msg = ZmqMessage::from(topic_bytes);
        msg.push_back(payload_bytes.into());

        // Clone the socket for sending (workaround for immutable borrow)
        drop(socket_guard);
        let mut socket_guard = self.socket.write().await;
        if let Some(ref mut socket) = *socket_guard {
            socket.send(msg).await?;
            stats.trades_published.fetch_add(1, Ordering::Relaxed);
            debug!("Published trade result: {} status={}", result.request_id, status_str);
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting ExecutionService Rust Service...");

    // ============================================================
    // CRITICAL: Trading Authorization Check (Dual-Flag System)
    // ============================================================
    let paper_trading_val = env::var("PAPER_TRADING").unwrap_or_else(|_| "1".to_string());
    let paper_trading = matches!(paper_trading_val.to_lowercase().as_str(), "1" | "true" | "yes");

    if !paper_trading {
        // Live trading mode - require explicit authorization
        let config = SafeguardConfig::from_env();

        if !config.live_trading_authorized {
            error!("============================================================");
            error!("FATAL: Live trading requested but not authorized!");
            error!("");
            error!("To enable live trading, you must set BOTH:");
            error!("  1. PAPER_TRADING=0 (or false)");
            error!("  2. LIVE_TRADING_AUTHORIZED=true");
            error!("");
            error!("This dual-flag system prevents accidental live trading.");
            error!("============================================================");
            std::process::exit(1);
        }

        warn!("============================================================");
        warn!("LIVE TRADING MODE ENABLED");
        warn!("Real money will be used for trades!");
        warn!("============================================================");
    }

    // Initialize Redis
    let redis = Arc::new(RedisBus::new().await?);

    // Initialize shared components for safeguards
    let kill_switch = Arc::new(KillSwitch::new());
    let idempotency = Arc::new(idempotency::IdempotencyTracker::new());

    // Create engine with shared safeguard components
    let engine = ExecutionEngine::with_safeguards(
        paper_trading,
        Some(redis.clone()),
        Some(kill_switch.clone()),
        Some(idempotency.clone()),
    )
    .await;

    // Transport mode configuration
    let transport_mode = TransportMode::from_env();

    let zmq_endpoint = env::var("ZMQ_SUB_ENDPOINT")
        .unwrap_or_else(|_| "tcp://signal_processor:5559".to_string());

    let zmq_pub_port: u16 = env::var("ZMQ_PUB_PORT")
        .unwrap_or_else(|_| "5560".to_string())
        .parse()
        .unwrap_or(5560);

    // Initialize ZMQ trade publisher (for zmq_listener to observe)
    let trade_publisher = Arc::new(
        TradePublisher::new(zmq_pub_port, transport_mode.use_zmq())
            .await
            .expect("Failed to initialize trade publisher"),
    );

    info!(
        "Execution Service ready (Paper Trading: {}, Kalshi Live: {}, Polymarket Live: {}, Transport: {:?}, ZMQ PUB: {})",
        paper_trading,
        engine.kalshi_live_enabled(),
        engine.polymarket_live_enabled(),
        transport_mode,
        if transport_mode.use_zmq() { format!(":{}", zmq_pub_port) } else { "disabled".to_string() }
    );

    // Start background tasks
    let _kill_switch_task = kill_switch
        .start_redis_listener(redis.clone())
        .await?;

    let _idempotency_cleanup_task = start_cleanup_task(idempotency.clone());

    // Start balance refresh (live trading only)
    let _balance_refresh_task = if !paper_trading {
        Some(start_balance_refresh_loop(
            engine.balance_cache(),
            engine.kalshi_client(),
            engine.config().balance_refresh_secs,
        ))
    } else {
        None
    };

    // Start daily P&L monitoring (live trading only)
    let pnl_tracker = Arc::new(DailyPnlTracker::new());
    let _pnl_monitor_task = if !paper_trading {
        Some(start_pnl_monitor(
            pnl_tracker.clone(),
            kill_switch.clone(),
            redis.clone(),
            engine.config().max_daily_loss,
        ))
    } else {
        None
    };

    match transport_mode {
        TransportMode::ZmqOnly => {
            // ZMQ only mode
            run_zmq_only(engine, redis, zmq_endpoint, pnl_tracker, trade_publisher).await
        }
        TransportMode::Both => {
            // Run with both ZMQ and Redis listeners
            run_with_zmq(engine, redis, zmq_endpoint, pnl_tracker, trade_publisher).await
        }
        TransportMode::RedisOnly => {
            // Run with Redis only (backward compatible)
            run_redis_only(engine, redis, pnl_tracker, trade_publisher).await
        }
    }
}

/// Start P&L monitoring task
fn start_pnl_monitor(
    pnl_tracker: Arc<DailyPnlTracker>,
    kill_switch: Arc<KillSwitch>,
    redis: Arc<RedisBus>,
    max_daily_loss: f64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        let mut warned_at_80 = false;

        loop {
            interval.tick().await;

            let utilization = pnl_tracker.get_loss_utilization(max_daily_loss).await;
            let pnl = pnl_tracker.get_pnl().await;

            // 80% warning
            if utilization >= 0.80 && utilization < 1.0 && !warned_at_80 {
                warned_at_80 = true;
                warn!(
                    "Daily loss warning: ${:.2} of ${:.2} limit ({:.0}%)",
                    -pnl, max_daily_loss, utilization * 100.0
                );

                let event = NotificationEvent {
                    event_type: NotificationType::RiskRejection,
                    priority: NotificationPriority::Warning,
                    data: serde_json::json!({
                        "service": "execution_service_rust",
                        "message": format!("Daily loss at {:.0}% of limit", utilization * 100.0),
                        "daily_pnl": pnl,
                        "limit": max_daily_loss,
                    }),
                    ts: Some(Utc::now()),
                };
                let _ = redis.publish(channels::NOTIFICATION_EVENTS, &event).await;
            }

            // 100% - activate kill switch
            if utilization >= 1.0 {
                error!(
                    "Daily loss limit EXCEEDED: ${:.2} of ${:.2} limit",
                    -pnl, max_daily_loss
                );
                kill_switch.enable(KillSwitchReason::DailyLossExceeded);

                let event = NotificationEvent {
                    event_type: NotificationType::Error,
                    priority: NotificationPriority::Critical,
                    data: serde_json::json!({
                        "service": "execution_service_rust",
                        "message": "DAILY LOSS LIMIT EXCEEDED - TRADING HALTED",
                        "daily_pnl": pnl,
                        "limit": max_daily_loss,
                    }),
                    ts: Some(Utc::now()),
                };
                let _ = redis.publish(channels::NOTIFICATION_EVENTS, &event).await;
            }

            // Reset warning flag at start of new day (when utilization drops)
            if utilization < 0.50 {
                warned_at_80 = false;
            }
        }
    })
}

/// Run with Redis pub/sub only (backward compatible mode)
async fn run_redis_only(
    engine: ExecutionEngine,
    redis: Arc<RedisBus>,
    pnl_tracker: Arc<DailyPnlTracker>,
    trade_publisher: Arc<TradePublisher>,
) -> Result<()> {
    info!("Running in Redis-only mode");

    let zmq_stats = Arc::new(ZmqStats::new());
    let mut pubsub = redis.subscribe("execution:requests").await?;
    info!("Subscribed to execution:requests");

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
            Ok(p) => p,
            Err(e) => {
                warn!("Execution request: failed to read payload: {}", e);
                continue;
            }
        };

        let request: ExecutionRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("Execution request: invalid JSON: {}", e);
                continue;
            }
        };

        process_request(&engine, &redis, &pnl_tracker, &trade_publisher, &zmq_stats, request).await;
    }

    Ok(())
}

/// Run with ZMQ only (lowest latency mode)
async fn run_zmq_only(
    engine: ExecutionEngine,
    redis: Arc<RedisBus>,
    zmq_endpoint: String,
    pnl_tracker: Arc<DailyPnlTracker>,
    trade_publisher: Arc<TradePublisher>,
) -> Result<()> {
    info!("Running in ZMQ-only mode");

    let engine = Arc::new(engine);
    let zmq_stats = Arc::new(ZmqStats::new());

    // Run ZMQ listener
    zmq_listener_loop(engine, redis, zmq_endpoint, zmq_stats, pnl_tracker, trade_publisher).await
}

/// Run with both ZMQ (primary, low-latency) and Redis (fallback) listeners
async fn run_with_zmq(
    engine: ExecutionEngine,
    redis: Arc<RedisBus>,
    zmq_endpoint: String,
    pnl_tracker: Arc<DailyPnlTracker>,
    trade_publisher: Arc<TradePublisher>,
) -> Result<()> {
    info!("Running in ZMQ + Redis hybrid mode");

    let engine = Arc::new(engine);
    let zmq_stats = Arc::new(ZmqStats::new());

    // Spawn ZMQ listener task
    let zmq_engine = engine.clone();
    let zmq_redis = redis.clone();
    let zmq_stats_clone = zmq_stats.clone();
    let zmq_pnl = pnl_tracker.clone();
    let zmq_trade_pub = trade_publisher.clone();
    let zmq_handle = tokio::spawn(async move {
        zmq_listener_loop(zmq_engine, zmq_redis, zmq_endpoint, zmq_stats_clone, zmq_pnl, zmq_trade_pub).await
    });

    // Spawn Redis listener task (fallback)
    let redis_engine = engine.clone();
    let redis_bus = redis.clone();
    let redis_pnl = pnl_tracker.clone();
    let redis_stats = zmq_stats.clone();
    let redis_trade_pub = trade_publisher.clone();
    let redis_handle = tokio::spawn(async move {
        redis_listener_loop(redis_engine, redis_bus, redis_pnl, redis_trade_pub, redis_stats).await
    });

    // Spawn stats logging task
    let stats_clone = zmq_stats.clone();
    let stats_handle = tokio::spawn(async move { stats_logging_loop(stats_clone).await });

    // Wait for any task to complete (they shouldn't under normal operation)
    tokio::select! {
        result = zmq_handle => {
            if let Err(e) = result {
                error!("ZMQ listener task failed: {}", e);
            }
        }
        result = redis_handle => {
            if let Err(e) = result {
                error!("Redis listener task failed: {}", e);
            }
        }
        _ = stats_handle => {}
    }

    Ok(())
}

/// ZMQ listener for low-latency signal reception
async fn zmq_listener_loop(
    engine: Arc<ExecutionEngine>,
    redis: Arc<RedisBus>,
    endpoint: String,
    stats: Arc<ZmqStats>,
    pnl_tracker: Arc<DailyPnlTracker>,
    trade_publisher: Arc<TradePublisher>,
) -> Result<()> {
    info!("Starting ZMQ listener for signals from {}", endpoint);

    loop {
        // Create ZMQ SUB socket
        let mut socket = SubSocket::new();

        // Connect with retry
        match socket.connect(&endpoint).await {
            Ok(_) => info!("ZMQ connected to {}", endpoint),
            Err(e) => {
                warn!(
                    "Failed to connect to ZMQ {}: {}. Retrying in 5s...",
                    endpoint, e
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        // Subscribe to execution request topics
        if let Err(e) = socket.subscribe("execution.request.").await {
            warn!("Failed to subscribe to execution requests: {}", e);
        }
        info!("ZMQ subscribed to execution.request.*");

        // Message processing loop
        loop {
            let recv_result =
                tokio::time::timeout(Duration::from_secs(30), socket.recv()).await;

            match recv_result {
                Ok(Ok(msg)) => {
                    stats.messages_received.fetch_add(1, Ordering::Relaxed);

                    // ZMQ multipart: [topic, payload]
                    let parts: Vec<_> = msg.iter().collect();
                    if parts.len() < 2 {
                        debug!("ZMQ message with {} parts, expected 2+", parts.len());
                        continue;
                    }

                    let topic = String::from_utf8_lossy(parts[0].as_ref());
                    let payload_bytes = parts[1].as_ref();

                    // Parse envelope
                    let envelope: ZmqEnvelope = match serde_json::from_slice(payload_bytes) {
                        Ok(e) => e,
                        Err(e) => {
                            stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                            debug!("Failed to parse ZMQ envelope: {}", e);
                            continue;
                        }
                    };

                    // Calculate signal latency
                    let now_ms = Utc::now().timestamp_millis();
                    let latency_ms = now_ms - envelope.timestamp_ms;
                    debug!(
                        "ZMQ signal received: topic={} latency={}ms",
                        topic, latency_ms
                    );

                    // Parse execution request from signal payload
                    let request: ExecutionRequest =
                        match serde_json::from_value(envelope.payload) {
                            Ok(r) => r,
                            Err(e) => {
                                stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                                debug!("Failed to parse execution request from ZMQ: {}", e);
                                continue;
                            }
                        };

                    // Log ZMQ-specific latency info
                    let signal_age_ms = (Utc::now() - request.created_at).num_milliseconds();
                    info!(
                        "ZMQ signal: {} age={}ms (ZMQ latency: {}ms)",
                        request.request_id, signal_age_ms, latency_ms
                    );

                    process_request(&engine, &redis, &pnl_tracker, &trade_publisher, &stats, request).await;
                    stats.messages_processed.fetch_add(1, Ordering::Relaxed);
                }
                Ok(Err(e)) => {
                    warn!("ZMQ receive error: {}. Reconnecting...", e);
                    break;
                }
                Err(_) => {
                    // Timeout - normal for low-traffic periods
                    debug!("No ZMQ signal in 30s");
                }
            }
        }

        // Brief delay before reconnect
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Redis listener for backward compatibility
async fn redis_listener_loop(
    engine: Arc<ExecutionEngine>,
    redis: Arc<RedisBus>,
    pnl_tracker: Arc<DailyPnlTracker>,
    trade_publisher: Arc<TradePublisher>,
    stats: Arc<ZmqStats>,
) -> Result<()> {
    info!("Starting Redis listener for execution:requests");

    let mut pubsub = redis.subscribe("execution:requests").await?;
    info!("Redis subscribed to execution:requests");

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
            Ok(p) => p,
            Err(e) => {
                warn!("Redis execution request: failed to read payload: {}", e);
                continue;
            }
        };

        let request: ExecutionRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("Redis execution request: invalid JSON: {}", e);
                continue;
            }
        };

        // Log that this came via Redis path
        let signal_age_ms = (Utc::now() - request.created_at).num_milliseconds();
        debug!("Redis signal: {} age={}ms", request.request_id, signal_age_ms);

        process_request(&engine, &redis, &pnl_tracker, &trade_publisher, &stats, request).await;
    }

    Ok(())
}

/// Process an execution request
async fn process_request(
    engine: &ExecutionEngine,
    redis: &RedisBus,
    pnl_tracker: &DailyPnlTracker,
    trade_publisher: &TradePublisher,
    stats: &ZmqStats,
    request: ExecutionRequest,
) {
    let result = match engine.execute(request.clone()).await {
        Ok(res) => res,
        Err(e) => {
            error!("Execution failed for {}: {}", request.request_id, e);
            let executed_at = Utc::now();
            let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
            ExecutionResult {
                request_id: request.request_id,
                idempotency_key: request.idempotency_key,
                rejection_reason: Some(e.to_string()),
                status: ExecutionStatus::Failed,
                order_id: None,
                filled_qty: 0.0,
                avg_price: 0.0,
                fees: 0.0,
                platform: request.platform,
                market_id: request.market_id,
                contract_team: request.contract_team,
                game_id: request.game_id,
                sport: request.sport,
                signal_id: request.signal_id,
                signal_type: request.signal_type,
                edge_pct: request.edge_pct,
                side: request.side,
                requested_at: request.created_at,
                executed_at,
                latency_ms,
            }
        }
    };

    // Track P&L for filled orders (simplified - assumes cost is price * qty)
    if result.status == ExecutionStatus::Filled {
        // For now, just track fees as immediate loss
        // Full P&L tracking would require position tracking
        pnl_tracker.record_pnl(-result.fees).await;
    }

    // Publish trade result to ZMQ (for zmq_listener to observe)
    if let Err(e) = trade_publisher.publish_trade(&result, stats).await {
        warn!("Failed to publish trade to ZMQ: {}", e);
    }

    // Publish notification on execution failure
    if result.status == ExecutionStatus::Failed {
        let event = NotificationEvent {
            event_type: NotificationType::Error,
            priority: NotificationPriority::Error,
            data: serde_json::json!({
                "service": "execution_service_rust",
                "request_id": result.request_id,
                "message": result.rejection_reason.clone().unwrap_or_else(|| "execution_failed".to_string()),
            }),
            ts: Some(Utc::now()),
        };
        if let Err(e) = redis.publish(channels::NOTIFICATION_EVENTS, &event).await {
            warn!("Failed to publish notification event: {}", e);
        }
    }

    // Publish execution result to Redis (for backward compatibility)
    if let Err(e) = redis.publish(channels::EXECUTION_RESULTS, &result).await {
        error!("Failed to publish execution result: {}", e);
    }
}

/// Periodic stats logging
async fn stats_logging_loop(stats: Arc<ZmqStats>) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        interval.tick().await;

        let received = stats.messages_received.load(Ordering::Relaxed);
        let processed = stats.messages_processed.load(Ordering::Relaxed);
        let parse_errors = stats.parse_errors.load(Ordering::Relaxed);
        let trades_published = stats.trades_published.load(Ordering::Relaxed);

        info!(
            "Execution Service ZMQ stats: received={}, processed={}, trades_published={}, parse_errors={}",
            received, processed, trades_published, parse_errors
        );
    }
}
