mod engine;
mod polymarket_executor;

use anyhow::Result;
use arbees_rust_core::models::{
    channels, ExecutionRequest, ExecutionResult, ExecutionStatus, NotificationEvent,
    NotificationPriority, NotificationType, TransportMode,
};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use dotenv::dotenv;
use engine::ExecutionEngine;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use zeromq::{Socket, SocketRecv, SubSocket};

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
}

impl ZmqStats {
    fn new() -> Self {
        Self {
            messages_received: AtomicU64::new(0),
            messages_processed: AtomicU64::new(0),
            parse_errors: AtomicU64::new(0),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting ExecutionService Rust Service...");

    let paper_trading_val = env::var("PAPER_TRADING").unwrap_or_else(|_| "1".to_string());
    let paper_trading = matches!(paper_trading_val.to_lowercase().as_str(), "1" | "true" | "yes");
    let engine = ExecutionEngine::new(paper_trading).await;
    let redis = RedisBus::new().await?;

    // Transport mode configuration
    let transport_mode = TransportMode::from_env();

    let zmq_endpoint = env::var("ZMQ_SUB_ENDPOINT")
        .unwrap_or_else(|_| "tcp://signal_processor:5559".to_string());

    info!(
        "Execution Service ready (Paper Trading: {}, Kalshi Live: {}, Polymarket Live: {}, Transport: {:?})",
        paper_trading,
        engine.kalshi_live_enabled(),
        engine.polymarket_live_enabled(),
        transport_mode
    );

    match transport_mode {
        TransportMode::ZmqOnly => {
            // ZMQ only mode
            run_zmq_only(engine, redis, zmq_endpoint).await
        }
        TransportMode::Both => {
            // Run with both ZMQ and Redis listeners
            run_with_zmq(engine, redis, zmq_endpoint).await
        }
        TransportMode::RedisOnly => {
            // Run with Redis only (backward compatible)
            run_redis_only(engine, redis).await
        }
    }
}

/// Run with Redis pub/sub only (backward compatible mode)
async fn run_redis_only(engine: ExecutionEngine, redis: RedisBus) -> Result<()> {
    info!("Running in Redis-only mode");

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

        process_request(&engine, &redis, request).await;
    }

    Ok(())
}

/// Run with ZMQ only (lowest latency mode)
async fn run_zmq_only(engine: ExecutionEngine, redis: RedisBus, zmq_endpoint: String) -> Result<()> {
    info!("Running in ZMQ-only mode");

    let engine = Arc::new(engine);
    let redis = Arc::new(redis);
    let zmq_stats = Arc::new(ZmqStats::new());

    // Run ZMQ listener
    zmq_listener_loop(engine, redis, zmq_endpoint, zmq_stats).await
}

/// Run with both ZMQ (primary, low-latency) and Redis (fallback) listeners
async fn run_with_zmq(engine: ExecutionEngine, redis: RedisBus, zmq_endpoint: String) -> Result<()> {
    info!("Running in ZMQ + Redis hybrid mode");

    let engine = Arc::new(engine);
    let redis = Arc::new(redis);
    let zmq_stats = Arc::new(ZmqStats::new());

    // Spawn ZMQ listener task
    let zmq_engine = engine.clone();
    let zmq_redis = redis.clone();
    let zmq_stats_clone = zmq_stats.clone();
    let zmq_handle = tokio::spawn(async move {
        zmq_listener_loop(zmq_engine, zmq_redis, zmq_endpoint, zmq_stats_clone).await
    });

    // Spawn Redis listener task (fallback)
    let redis_engine = engine.clone();
    let redis_bus = redis.clone();
    let redis_handle = tokio::spawn(async move {
        redis_listener_loop(redis_engine, redis_bus).await
    });

    // Spawn stats logging task
    let stats_clone = zmq_stats.clone();
    let stats_handle = tokio::spawn(async move {
        stats_logging_loop(stats_clone).await
    });

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
) -> Result<()> {
    info!("Starting ZMQ listener for signals from {}", endpoint);

    loop {
        // Create ZMQ SUB socket
        let mut socket = SubSocket::new();

        // Connect with retry
        match socket.connect(&endpoint).await {
            Ok(_) => info!("ZMQ connected to {}", endpoint),
            Err(e) => {
                warn!("Failed to connect to ZMQ {}: {}. Retrying in 5s...", endpoint, e);
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
            let recv_result = tokio::time::timeout(
                Duration::from_secs(30),
                socket.recv(),
            ).await;

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
                    debug!("ZMQ signal received: topic={} latency={}ms", topic, latency_ms);

                    // Parse execution request from signal payload
                    let request: ExecutionRequest = match serde_json::from_value(envelope.payload) {
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

                    process_request(&engine, &redis, request).await;
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

        process_request(&engine, &redis, request).await;
    }

    Ok(())
}

/// Process an execution request
async fn process_request(engine: &ExecutionEngine, redis: &RedisBus, request: ExecutionRequest) {
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

        info!(
            "Execution Service ZMQ stats: received={}, processed={}, parse_errors={}",
            received, processed, parse_errors
        );
    }
}
