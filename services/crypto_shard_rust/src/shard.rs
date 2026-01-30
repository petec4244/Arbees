//! CryptoShard: Self-contained crypto event monitoring and trading
//!
//! Main orchestrator that coordinates:
//! - ZMQ price listening
//! - Event monitoring
//! - Arbitrage detection
//! - Probability-based signal generation
//! - Inline risk management
//! - Direct execution request emission

use crate::config::CryptoShardConfig;
use crate::price::listener::{CryptoPriceListener, ListenerConfig};
use crate::signals::arbitrage::CryptoArbitrageDetector;
use crate::signals::probability::CryptoProbabilityDetector;
use crate::signals::risk::CryptoRiskChecker;
use crate::types::{CryptoEventContext, CryptoExecutionRequest, ZmqEnvelope};
use anyhow::Result;
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use futures_util::stream::StreamExt;
use log::{debug, error, info, warn};
use serde_json::json;
use sqlx::PgPool;
use std::env;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::time::interval;
use zeromq::{PubSocket, Socket, SocketSend, ZmqMessage};

/// Main CryptoShard service
pub struct CryptoShard {
    pub config: CryptoShardConfig,
    pub db_pool: PgPool,
    pub redis: RedisBus,

    // Events being monitored
    pub events: Arc<RwLock<HashMap<String, CryptoEventContext>>>,

    // Price cache: asset|platform -> price
    pub price_listener: CryptoPriceListener,

    // Signal detectors
    pub arb_detector: CryptoArbitrageDetector,
    pub prob_detector: CryptoProbabilityDetector,
    pub risk_checker: CryptoRiskChecker,

    // ZMQ publisher for ExecutionRequests (wrapped for async mutex access)
    pub execution_pub: Option<Arc<Mutex<PubSocket>>>,
    pub execution_seq: Arc<AtomicU64>,

    // Statistics
    pub stats: Arc<ShardStats>,

    // Service metadata
    pub heartbeat_interval: Duration,
}

#[derive(Debug, Clone)]
pub struct ShardStats {
    pub events_monitored: Arc<AtomicU64>,
    pub prices_received: Arc<AtomicU64>,
    pub arbitrage_signals: Arc<AtomicU64>,
    pub model_signals: Arc<AtomicU64>,
    pub execution_requests_sent: Arc<AtomicU64>,
    pub risk_blocks: Arc<AtomicU64>,
}

impl ShardStats {
    pub fn new() -> Self {
        Self {
            events_monitored: Arc::new(AtomicU64::new(0)),
            prices_received: Arc::new(AtomicU64::new(0)),
            arbitrage_signals: Arc::new(AtomicU64::new(0)),
            model_signals: Arc::new(AtomicU64::new(0)),
            execution_requests_sent: Arc::new(AtomicU64::new(0)),
            risk_blocks: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn snapshot(&self) -> ShardStatsSnapshot {
        ShardStatsSnapshot {
            events_monitored: self.events_monitored.load(Ordering::Relaxed),
            prices_received: self.prices_received.load(Ordering::Relaxed),
            arbitrage_signals: self.arbitrage_signals.load(Ordering::Relaxed),
            model_signals: self.model_signals.load(Ordering::Relaxed),
            execution_requests_sent: self.execution_requests_sent.load(Ordering::Relaxed),
            risk_blocks: self.risk_blocks.load(Ordering::Relaxed),
        }
    }
}

impl Default for ShardStats {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ShardStatsSnapshot {
    pub events_monitored: u64,
    pub prices_received: u64,
    pub arbitrage_signals: u64,
    pub model_signals: u64,
    pub execution_requests_sent: u64,
    pub risk_blocks: u64,
}

impl CryptoShard {
    pub async fn new(config: CryptoShardConfig) -> Result<Self> {
        info!(
            "Initializing CryptoShard {} on endpoints: {:?}",
            config.shard_id, config.price_sub_endpoints
        );

        // Connect to database
        let db_pool = PgPool::connect(&config.database_url).await?;
        sqlx::query("SELECT 1").execute(&db_pool).await?;
        info!("Connected to database");

        // Connect to Redis (for heartbeat and commands)
        // RedisBus uses REDIS_URL environment variable
        let redis = RedisBus::new().await?;
        info!("Connected to Redis");

        // Create ZMQ publisher for execution requests
        info!("Creating ZMQ PubSocket...");
        let mut execution_pub = PubSocket::new();
        info!("PubSocket created successfully");

        info!("Binding PubSocket to {}", config.execution_pub_endpoint);
        match execution_pub.bind(&config.execution_pub_endpoint).await {
            Ok(_) => {
                info!(
                    "CryptoShard publishing ExecutionRequests to {}",
                    config.execution_pub_endpoint
                );
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to bind PubSocket to {}: {}", config.execution_pub_endpoint, e));
            }
        }
        let execution_pub = Some(Arc::new(Mutex::new(execution_pub)));

        // Create price listener
        let prices = Arc::new(RwLock::new(HashMap::new()));
        let prices_received = Arc::new(AtomicU64::new(0));

        let mut price_listener = CryptoPriceListener::new(
            config.price_sub_endpoints.clone(),
            prices.clone(),
            prices_received.clone(),
        );

        // Configure price listener
        let listener_config = ListenerConfig {
            price_staleness: Duration::from_secs(config.price_staleness_secs),
            latency_log_interval: 100,
            zmq_receive_timeout: Duration::from_secs(30),
            reconnect_delay: Duration::from_secs(5),
        };
        price_listener = price_listener.with_config(listener_config);

        // Create signal detectors
        let arb_detector = CryptoArbitrageDetector::new(config.min_edge_pct);
        let prob_detector = CryptoProbabilityDetector::new(
            config.min_edge_pct,
            config.model_min_confidence,
        );

        // Create risk checker
        let risk_checker = CryptoRiskChecker::new(
            db_pool.clone(),
            config.min_edge_pct,
            config.max_position_size,
            config.max_asset_exposure,
            config.max_total_exposure,
            config.volatility_scaling,
            config.min_liquidity,
        );

        let stats = Arc::new(ShardStats::new());

        Ok(Self {
            config: config.clone(),
            db_pool,
            redis,
            events: Arc::new(RwLock::new(HashMap::new())),
            price_listener,
            arb_detector,
            prob_detector,
            risk_checker,
            execution_pub,
            execution_seq: Arc::new(AtomicU64::new(0)),
            stats,
            heartbeat_interval: Duration::from_secs(config.heartbeat_interval_secs),
        })
    }

    /// Start the CryptoShard service
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting CryptoShard {}", self.config.shard_id);

        // Set up price update channel if event-driven evaluation is enabled
        let price_update_rx = if self.config.event_driven_evaluation {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            self.price_listener.set_price_update_notifier(tx).await;
            info!(
                "[{}] Event-driven evaluation enabled - price updates will trigger signal monitoring",
                self.config.shard_id
            );
            Some(rx)
        } else {
            info!(
                "[{}] Polling-based evaluation enabled - signals evaluated every {}s",
                self.config.shard_id, self.config.poll_interval_secs
            );
            None
        };

        // Spawn price listener as background task
        let price_listener = self.price_listener.clone();
        let listener_shard_id = self.config.shard_id.clone();
        tokio::spawn(async move {
            info!("[{}] Price listener task starting", listener_shard_id);
            match price_listener.start().await {
                Ok(_) => {
                    info!("[{}] Price listener completed normally", listener_shard_id);
                }
                Err(e) => {
                    error!("[{}] Price listener error: {}", listener_shard_id, e);
                }
            }
        });

        // Spawn command listener as background task
        let cmd_shard_id = self.config.shard_id.clone();
        let cmd_redis = self.redis.clone();
        let cmd_events = self.events.clone();

        tokio::spawn(async move {
            info!("[{}] Command listener task starting", cmd_shard_id);
            Self::command_listener(cmd_shard_id.clone(), cmd_redis, cmd_events).await;
            info!("[{}] Command listener task completed", cmd_shard_id);
        });

        // Spawn heartbeat loop for orchestrator service discovery
        let heartbeat_shard_id = self.config.shard_id.clone();
        let heartbeat_events = self.events.clone();
        let heartbeat_stats = self.stats.clone();
        let heartbeat_interval = self.heartbeat_interval;
        let heartbeat_redis = self.redis.clone();
        let heartbeat_execution_pub = self.execution_pub.clone();
        let heartbeat_price_listener = self.price_listener.clone();

        tokio::spawn(async move {
            info!("[{}] Heartbeat task starting", heartbeat_shard_id);
            Self::heartbeat_loop(
                heartbeat_shard_id.clone(),
                heartbeat_events,
                heartbeat_stats,
                heartbeat_interval,
                heartbeat_redis,
                heartbeat_execution_pub,
                heartbeat_price_listener,
            )
            .await;
            info!("[{}] Heartbeat task completed", heartbeat_shard_id);
        });

        // Main monitoring loop - supports both event-driven and polling modes
        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));
        let event_driven = self.config.event_driven_evaluation;
        let shard_id = self.config.shard_id.clone();

        info!(
            "CryptoShard {} initialized (poll_interval: {}s, price_staleness: {}s, event_driven: {})",
            shard_id,
            self.config.poll_interval_secs,
            self.config.price_staleness_secs,
            event_driven
        );

        info!("CryptoShard {} running", shard_id);

        let mut stats_log_counter = 0u64;
        let mut price_update_rx = price_update_rx;

        loop {
            if event_driven {
                // Event-driven mode: trigger on price updates OR periodic backup
                if let Some(ref mut rx) = price_update_rx {
                    tokio::select! {
                        // Trigger on new price (no timeout - immediate)
                        Some(update) = rx.recv() => {
                            if update.count % 100 == 0 {
                                debug!("Event-driven trigger: {} price update #{}", update.asset, update.count);
                            }
                            if let Err(e) = self.monitor_events().await {
                                warn!("Error in event-driven monitoring: {}", e);
                            }
                        }

                        // Fallback periodic ticker (every N seconds as backup)
                        _ = ticker.tick() => {
                            debug!("Periodic backup monitoring triggered (event-driven backup cycle)");
                            if let Err(e) = self.monitor_events().await {
                                warn!("Error in backup monitoring: {}", e);
                            }
                        }
                    }
                } else {
                    // Shouldn't reach here, but handle gracefully
                    ticker.tick().await;
                    if let Err(e) = self.monitor_events().await {
                        warn!("Error in monitoring: {}", e);
                    }
                }
            } else {
                // Polling mode: traditional fixed-interval monitoring
                ticker.tick().await;
                if let Err(e) = self.monitor_events().await {
                    warn!("Error in polling-based monitoring: {}", e);
                }
            }

            // Log statistics periodically
            stats_log_counter += 1;
            if stats_log_counter % 12 == 0 {  // Log every ~60s (if poll_interval_secs=5)
                let snapshot = self.stats.snapshot();
                info!(
                    "CryptoShard stats: events={}, arb_signals={}, model_signals={}, exec_sent={}, risk_blocks={}",
                    snapshot.events_monitored,
                    snapshot.arbitrage_signals,
                    snapshot.model_signals,
                    snapshot.execution_requests_sent,
                    snapshot.risk_blocks
                );
            }
        }
    }

    /// Listen for commands from orchestrator via Redis
    async fn command_listener(
        shard_id: String,
        redis: RedisBus,
        events: Arc<RwLock<HashMap<String, CryptoEventContext>>>,
    ) {
        let channel = format!("shard:{}:command", shard_id);
        info!(
            "Command listener started for shard {}, listening on {}",
            shard_id, channel
        );

        // Use reconnecting pub/sub for automatic reconnection handling
        let reconnecting_pubsub = redis.subscribe_with_reconnect(vec![channel]);
        let mut message_stream = reconnecting_pubsub.into_message_stream();

        while let Some(msg) = message_stream.next().await {
            // Parse the incoming message
            match serde_json::from_str::<serde_json::Value>(&msg.get_payload::<String>().unwrap_or_default()) {
                Ok(cmd_json) => {
                    if let Some(cmd_type) = cmd_json.get("type").and_then(|v| v.as_str()) {
                        match cmd_type {
                            "add_event" => {
                                // Parse event from command
                                match Self::parse_add_event_command(&cmd_json) {
                                    Ok(event) => {
                                        let event_id = event.event_id.clone();
                                        let asset = event.asset.clone();
                                        let target_price = event.target_price.unwrap_or(0.0);
                                        let target_date = event.target_date.format("%Y-%m-%d");

                                        info!(
                                            "Adding crypto event: {} ({}) -> ${} by {}",
                                            event_id, asset, target_price, target_date
                                        );
                                        events.write().await.insert(event_id, event);
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse add_event command: {}", e);
                                    }
                                }
                            }
                            "remove_event" => {
                                if let Some(event_id) = cmd_json.get("event_id").and_then(|v| v.as_str()) {
                                    info!("Removing crypto event: {}", event_id);
                                    events.write().await.remove(event_id);
                                }
                            }
                            "shutdown" => {
                                info!("Shutdown command received");
                                return;
                            }
                            _ => {
                                warn!("Unknown command type: {}", cmd_type);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to parse command JSON: {}", e);
                }
            }
        }
    }

    /// Parse add_event command JSON into CryptoEventContext
    fn parse_add_event_command(cmd: &serde_json::Value) -> Result<CryptoEventContext> {
        let event_id = cmd.get("event_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing event_id"))?
            .to_string();

        // Extract asset from top-level field or from nested market_type.asset
        let asset = if let Some(a) = cmd.get("asset").and_then(|v| v.as_str()) {
            a.to_string()
        } else if let Some(market_type) = cmd.get("market_type") {
            // Try to extract from market_type.asset (flattened enum format)
            market_type.get("asset")
                .and_then(|a| a.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing asset in market_type.asset or top-level"))?
                .to_string()
        } else {
            return Err(anyhow::anyhow!("Missing asset - not in top-level or market_type"));
        };

        let target_price = cmd.get("target_price").and_then(|v| v.as_f64());

        // Try to get target_date, or default to 24 hours from now
        let target_date = if let Some(target_date_str) = cmd.get("target_date").and_then(|v| v.as_str()) {
            chrono::DateTime::parse_from_rfc3339(target_date_str)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .ok_or_else(|| anyhow::anyhow!("Invalid target_date format"))?
        } else {
            // Default: 24 hours from now
            Utc::now() + chrono::Duration::hours(24)
        };

        let description = cmd.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("Crypto event")
            .to_string();

        Ok(CryptoEventContext {
            event_id,
            asset,
            event_type: crate::types::CryptoEventType::PriceTarget,
            target_price,
            target_date,
            description,
            created_at: Utc::now(),
        })
    }

    /// Publish heartbeat for orchestrator service discovery
    async fn heartbeat_loop(
        shard_id: String,
        events: Arc<RwLock<HashMap<String, CryptoEventContext>>>,
        stats: Arc<ShardStats>,
        heartbeat_interval: Duration,
        redis: RedisBus,
        execution_pub: Option<Arc<Mutex<PubSocket>>>,
        price_listener: CryptoPriceListener,
    ) {
        let channel = format!("shard:{}:heartbeat", shard_id);

        loop {
            let events_lock = events.read().await;
            let event_count = events_lock.len();
            // Collect event IDs for orchestrator zombie detection
            let event_ids: Vec<String> = events_lock.keys().cloned().collect();
            drop(events_lock);

            let snapshot = stats.snapshot();

            // Shard type for orchestrator routing
            let shard_type = env::var("SHARD_TYPE").unwrap_or_else(|_| "crypto".to_string());

            // Check component health
            // Redis OK will be verified implicitly by successful publish
            let redis_ok = true;
            // ZMQ is OK if the execution publisher is initialized
            let zmq_ok = execution_pub.is_some();
            // Price listener is OK if it has received prices recently
            let listener_stats = price_listener.stats();
            let price_listener_ok = listener_stats.total_prices_received > 0;

            let payload = json!({
                "shard_id": shard_id,
                "shard_type": shard_type,
                "event_count": event_count,
                // Include event IDs for orchestrator zombie detection
                // Service registry checks both "events" and "games" fields
                "events": event_ids,
                "max_games": 1000, // Crypto shards have high capacity
                "timestamp": Utc::now().to_rfc3339(),
                "status": "healthy",
                "checks": {
                    "redis_ok": redis_ok,
                    "zmq_ok": zmq_ok,
                    "price_listener_ok": price_listener_ok,
                },
                "metrics": {
                    "events_monitored": snapshot.events_monitored,
                    "prices_received": snapshot.prices_received,
                    "arbitrage_signals": snapshot.arbitrage_signals,
                    "model_signals": snapshot.model_signals,
                    "execution_requests_sent": snapshot.execution_requests_sent,
                    "risk_blocks": snapshot.risk_blocks,
                },
            });

            if let Err(e) = redis.publish(&channel, &payload).await {
                warn!("Heartbeat publish error: {}", e);
            }

            tokio::time::sleep(heartbeat_interval).await;
        }
    }

    /// Main monitoring loop that detects and emits signals
    async fn monitor_events(&mut self) -> Result<()> {
        let events = self.events.read().await.clone();

        if events.is_empty() {
            return Ok(());
        }

        self.stats
            .events_monitored
            .fetch_add(1, Ordering::Relaxed);

        for (event_id, event) in events.iter() {
            // Get prices for this asset
            let asset_prices = self.price_listener.get_asset_prices(&event.asset).await;

            if asset_prices.is_empty() {
                continue; // No prices yet
            }

            // Build price cache: asset|platform -> price
            let mut price_cache = HashMap::new();
            for price in &asset_prices {
                let key = format!("{}|{}", price.asset, price.platform);
                price_cache.insert(key, price.clone());
            }

            // Check for stale prices
            let price_staleness = Duration::from_secs(self.config.price_staleness_secs);
            if asset_prices.iter().all(|p| p.is_stale(price_staleness)) {
                warn!("Stale prices for event {}", event_id);
                continue;
            }

            // Volatility factor (simplified for Phase 5)
            let volatility_factor = 1.0;

            // 1. Arbitrage detection
            if let Ok(Some(request)) = self
                .arb_detector
                .detect_and_emit(event_id, &event.asset, &price_cache, &self.risk_checker, volatility_factor)
                .await
            {
                self.emit_execution_request(request).await?;
                self.stats
                    .arbitrage_signals
                    .fetch_add(1, Ordering::Relaxed);
            }

            // 2. Probability-based detection
            // Note: In production, would get actual spot price from price feed
            if let Some(best_price) = asset_prices.first() {
                if let Ok(Some(request)) = self
                    .prob_detector
                    .detect_and_emit(&event, &price_cache, best_price.mid_price, &self.risk_checker)
                    .await
                {
                    self.emit_execution_request(request).await?;
                    self.stats
                        .model_signals
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        Ok(())
    }

    /// Emit an execution request via ZMQ
    async fn emit_execution_request(&mut self, request: CryptoExecutionRequest) -> Result<()> {
        if let Some(ref pub_socket) = self.execution_pub {
            let topic = format!("crypto.execution.{}", request.request_id);
            let seq = self.execution_seq.fetch_add(1, Ordering::SeqCst);

            let envelope = ZmqEnvelope {
                seq,
                timestamp_ms: Utc::now().timestamp_millis(),
                source: format!("crypto_shard:{}", self.config.shard_id),
                payload: request.clone(),
            };

            let payload = serde_json::to_vec(&envelope)?;

            // Create and send multi-part ZMQ message: [topic, payload]
            let mut msg = ZmqMessage::from(topic.as_bytes().to_vec());
            msg.push_back(payload.into());

            let mut socket = pub_socket.lock().await;
            if let Err(e) = socket.send(msg).await {
                warn!("Failed to publish execution request via ZMQ: {}", e);
                return Err(anyhow::anyhow!("ZMQ send failed: {}", e));
            }

            self.stats
                .execution_requests_sent
                .fetch_add(1, Ordering::Relaxed);

            info!(
                "Emitted ExecutionRequest: {} {} ${:.2} (edge: {:.2}%, signal: {:?})",
                request.asset, request.platform, request.suggested_size, request.edge_pct, request.signal_type
            );
        } else {
            warn!("No ZMQ publisher configured for execution requests");
        }

        Ok(())
    }
}
