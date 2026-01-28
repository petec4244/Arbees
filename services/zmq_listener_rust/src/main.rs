//! ZMQ Listener Service - Bridges ZMQ messages to Redis for backward compatibility
//!
//! This service subscribes to ZMQ publishers (kalshi_monitor, polymarket_monitor, game_shard)
//! and mirrors messages to Redis streams/pub-sub for services that don't use ZMQ.
//!
//! Architecture:
//! ```
//! kalshi_monitor (ZMQ PUB :5555) ──┐
//! polymarket_monitor (ZMQ PUB :5556) ──┼──> zmq_listener ──> Redis
//! game_shard (ZMQ PUB :5558) ──────┘
//! ```

use anyhow::Result;
use arbees_rust_core::models::TransportMode;
use chrono::Utc;
use dotenv::dotenv;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zeromq::{Socket, SocketRecv, SubSocket};

/// ZMQ message envelope format
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ZmqEnvelope {
    seq: u64,
    timestamp_ms: i64,
    source: Option<String>,
    payload: serde_json::Value,
}

/// Listener statistics for monitoring
#[derive(Debug, Default)]
struct ListenerStats {
    messages_received: AtomicU64,
    messages_forwarded: AtomicU64,
    parse_errors: AtomicU64,
    redis_errors: AtomicU64,
}

impl ListenerStats {
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_forwarded: self.messages_forwarded.load(Ordering::Relaxed),
            parse_errors: self.parse_errors.load(Ordering::Relaxed),
            redis_errors: self.redis_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
struct StatsSnapshot {
    messages_received: u64,
    messages_forwarded: u64,
    parse_errors: u64,
    redis_errors: u64,
}

/// ZMQ endpoint configuration
struct ZmqEndpoint {
    name: String,
    address: String,
    subscriptions: Vec<String>,
}

/// ZMQ Listener that bridges ZMQ -> Redis
struct ZmqListener {
    redis_client: redis::Client,
    endpoints: Vec<ZmqEndpoint>,
    stats: Arc<ListenerStats>,
    running: Arc<RwLock<bool>>,
}

impl ZmqListener {
    async fn new() -> Result<Self> {
        // Connect to Redis
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379".to_string());
        let redis_client = redis::Client::open(redis_url)?;

        // Configure ZMQ endpoints
        let kalshi_endpoint = env::var("ZMQ_KALSHI_ENDPOINT")
            .unwrap_or_else(|_| "tcp://kalshi_monitor:5555".to_string());
        let polymarket_endpoint = env::var("ZMQ_POLYMARKET_ENDPOINT")
            .unwrap_or_else(|_| "tcp://polymarket_monitor:5556".to_string());
        let game_shard_endpoint = env::var("ZMQ_GAME_SHARD_ENDPOINT")
            .unwrap_or_else(|_| "tcp://game_shard:5558".to_string());

        let endpoints = vec![
            ZmqEndpoint {
                name: "kalshi".to_string(),
                address: kalshi_endpoint,
                subscriptions: vec!["prices.kalshi.".to_string()],
            },
            ZmqEndpoint {
                name: "polymarket".to_string(),
                address: polymarket_endpoint,
                subscriptions: vec!["prices.poly.".to_string()],
            },
            ZmqEndpoint {
                name: "game_shard".to_string(),
                address: game_shard_endpoint,
                subscriptions: vec![
                    "signals.trade.".to_string(),
                    "games.".to_string(),
                ],
            },
        ];

        Ok(Self {
            redis_client,
            endpoints,
            stats: Arc::new(ListenerStats::default()),
            running: Arc::new(RwLock::new(true)),
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting ZMQ Listener Service...");

        // Test Redis connection
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let _: String = redis::cmd("PING").query_async(&mut conn).await?;
        info!("Redis connection verified");

        // Spawn listener tasks for each endpoint
        let mut handles = Vec::new();

        for endpoint in &self.endpoints {
            let endpoint_name = endpoint.name.clone();
            let endpoint_addr = endpoint.address.clone();
            let subscriptions = endpoint.subscriptions.clone();
            let redis_client = self.redis_client.clone();
            let stats = self.stats.clone();
            let running = self.running.clone();

            let handle = tokio::spawn(async move {
                Self::listen_to_endpoint(
                    endpoint_name,
                    endpoint_addr,
                    subscriptions,
                    redis_client,
                    stats,
                    running,
                )
                .await
            });

            handles.push(handle);
        }

        // Spawn health monitoring task
        let stats = self.stats.clone();
        let running = self.running.clone();
        let redis_client = self.redis_client.clone();
        handles.push(tokio::spawn(async move {
            Self::health_monitor_loop(stats, running, redis_client).await
        }));

        // Wait for all tasks
        for handle in handles {
            if let Err(e) = handle.await {
                error!("Task error: {}", e);
            }
        }

        Ok(())
    }

    async fn listen_to_endpoint(
        name: String,
        address: String,
        subscriptions: Vec<String>,
        redis_client: redis::Client,
        stats: Arc<ListenerStats>,
        running: Arc<RwLock<bool>>,
    ) -> Result<()> {
        loop {
            // Check if we should keep running
            if !*running.read().await {
                info!("Stopping listener for {}", name);
                break;
            }

            info!(
                "Connecting to ZMQ endpoint: {} at {}",
                name, address
            );

            // Create ZMQ SUB socket
            let mut socket = SubSocket::new();

            // Attempt connection with retry
            match socket.connect(&address).await {
                Ok(_) => info!("Connected to ZMQ: {}", name),
                Err(e) => {
                    warn!("Failed to connect to {}: {}. Retrying in 5s...", name, e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }

            // Subscribe to topics
            for sub in &subscriptions {
                if let Err(e) = socket.subscribe(sub).await {
                    warn!("Failed to subscribe to '{}' on {}: {}", sub, name, e);
                }
            }

            // Also subscribe to empty prefix to get all messages if no specific subs
            if subscriptions.is_empty() {
                if let Err(e) = socket.subscribe("").await {
                    warn!("Failed to subscribe to all on {}: {}", name, e);
                }
            }

            info!(
                "Subscribed to {} topics on {}: {:?}",
                subscriptions.len(),
                name,
                subscriptions
            );

            // Get Redis connection
            let mut redis_conn = match redis_client.get_multiplexed_async_connection().await {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to get Redis connection: {}. Retrying...", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            // Message processing loop
            loop {
                if !*running.read().await {
                    break;
                }

                // Receive with timeout
                let recv_result = tokio::time::timeout(
                    Duration::from_secs(30),
                    socket.recv(),
                )
                .await;

                match recv_result {
                    Ok(Ok(msg)) => {
                        stats.messages_received.fetch_add(1, Ordering::Relaxed);

                        // ZMQ multipart: [topic, payload]
                        let parts: Vec<_> = msg.iter().collect();
                        if parts.len() < 2 {
                            debug!("Received message with {} parts, expected 2+", parts.len());
                            continue;
                        }

                        let topic = String::from_utf8_lossy(parts[0].as_ref());
                        let payload_bytes = parts[1].as_ref();

                        // Parse envelope
                        let envelope: ZmqEnvelope = match serde_json::from_slice(payload_bytes) {
                            Ok(e) => e,
                            Err(e) => {
                                stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                                debug!("Failed to parse ZMQ message from {}: {}", name, e);
                                continue;
                            }
                        };

                        // Forward to Redis based on topic
                        if let Err(e) = Self::forward_to_redis(
                            &mut redis_conn,
                            &topic,
                            &envelope,
                        )
                        .await
                        {
                            stats.redis_errors.fetch_add(1, Ordering::Relaxed);
                            warn!("Failed to forward to Redis: {}", e);
                        } else {
                            stats.messages_forwarded.fetch_add(1, Ordering::Relaxed);
                            debug!(
                                "Forwarded {} -> Redis (seq={})",
                                topic, envelope.seq
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("ZMQ receive error on {}: {}. Reconnecting...", name, e);
                        break;
                    }
                    Err(_) => {
                        // Timeout - just continue, this is normal for low-traffic periods
                        debug!("No message from {} in 30s", name);
                    }
                }
            }

            // Reconnection delay
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        Ok(())
    }

    async fn forward_to_redis(
        conn: &mut redis::aio::MultiplexedConnection,
        topic: &str,
        envelope: &ZmqEnvelope,
    ) -> Result<()> {
        // Determine Redis channel/stream based on topic prefix
        let payload_str = serde_json::to_string(&envelope.payload)?;

        if topic.starts_with("prices.") {
            // Price messages go to both:
            // 1. Redis pub/sub for real-time consumers (game:*:price pattern)
            // 2. Redis stream for persistence/replay

            // Extract game_id from payload if available
            if let Some(game_id) = envelope.payload.get("game_id").and_then(|v| v.as_str()) {
                let channel = format!("game:{}:price", game_id);
                let _: () = redis::cmd("PUBLISH")
                    .arg(&channel)
                    .arg(&payload_str)
                    .query_async(conn)
                    .await?;
            }

            // Also add to stream
            let stream_key = if topic.starts_with("prices.kalshi.") {
                "prices:kalshi"
            } else if topic.starts_with("prices.poly.") {
                "prices:polymarket"
            } else {
                "prices:unknown"
            };

            let _: String = redis::cmd("XADD")
                .arg(stream_key)
                .arg("MAXLEN")
                .arg("~")
                .arg("10000") // Keep last ~10k messages
                .arg("*") // Auto-generate ID
                .arg("topic")
                .arg(topic)
                .arg("payload")
                .arg(&payload_str)
                .arg("ts")
                .arg(envelope.timestamp_ms)
                .query_async(conn)
                .await?;

        } else if topic.starts_with("signals.") {
            // Signal messages go to signals:new pub/sub channel
            let _: () = redis::cmd("PUBLISH")
                .arg("signals:new")
                .arg(&payload_str)
                .query_async(conn)
                .await?;

            // Also add to stream for persistence
            let _: String = redis::cmd("XADD")
                .arg("signals:stream")
                .arg("MAXLEN")
                .arg("~")
                .arg("1000")
                .arg("*")
                .arg("topic")
                .arg(topic)
                .arg("payload")
                .arg(&payload_str)
                .arg("ts")
                .arg(envelope.timestamp_ms)
                .query_async(conn)
                .await?;

        } else if topic.starts_with("games.") {
            // Game state messages go to game:{game_id}:state pub/sub
            if let Some(game_id) = envelope.payload.get("game_id").and_then(|v| v.as_str()) {
                let channel = format!("game:{}:state", game_id);
                let _: () = redis::cmd("PUBLISH")
                    .arg(&channel)
                    .arg(&payload_str)
                    .query_async(conn)
                    .await?;
            }
        }

        Ok(())
    }

    async fn health_monitor_loop(
        stats: Arc<ListenerStats>,
        running: Arc<RwLock<bool>>,
        redis_client: redis::Client,
    ) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            if !*running.read().await {
                break;
            }

            let snapshot = stats.snapshot();

            info!(
                "ZMQ Listener stats: received={}, forwarded={}, parse_errors={}, redis_errors={}",
                snapshot.messages_received,
                snapshot.messages_forwarded,
                snapshot.parse_errors,
                snapshot.redis_errors
            );

            // Publish health status to Redis
            if let Ok(mut conn) = redis_client.get_multiplexed_async_connection().await {
                let health = serde_json::json!({
                    "service": "zmq_listener_rust",
                    "healthy": true,
                    "messages_received": snapshot.messages_received,
                    "messages_forwarded": snapshot.messages_forwarded,
                    "parse_errors": snapshot.parse_errors,
                    "redis_errors": snapshot.redis_errors,
                    "timestamp": Utc::now().to_rfc3339(),
                });

                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("health:heartbeats")
                    .arg(health.to_string())
                    .query_async(&mut conn)
                    .await;
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting ZMQ Listener Rust Service...");

    // Check transport mode - zmq_listener only needed for bridging ZMQ to Redis
    let transport_mode = TransportMode::from_env();
    info!("Transport mode: {:?}", transport_mode);

    if transport_mode == TransportMode::ZmqOnly {
        info!("ZMQ_TRANSPORT_MODE=zmq_only, zmq_listener not needed (no bridging required)");
        info!("Services communicate directly via ZMQ - zmq_listener exiting gracefully");
        return Ok(());
    }

    if transport_mode == TransportMode::RedisOnly {
        info!("ZMQ_TRANSPORT_MODE=redis_only, zmq_listener not needed (ZMQ disabled)");
        return Ok(());
    }

    // transport_mode == Both: Bridge ZMQ messages to Redis
    info!("ZMQ_TRANSPORT_MODE=both, starting ZMQ→Redis bridge");
    let listener = ZmqListener::new().await?;
    listener.start().await?;

    Ok(())
}
