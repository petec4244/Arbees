//! ZMQ Listener Service - Observes all ZMQ traffic for logging, analytics, and persistence
//!
//! This service subscribes to ALL ZMQ publishers in the system and provides:
//! 1. Structured console logging for debugging
//! 2. Redis streams for historical queries and slow-path consumers
//! 3. Optional file logging for replay/backtesting
//! 4. Latency tracking and metrics
//!
//! Architecture:
//! ```
//! kalshi_monitor (PUB :5555) ────────┐
//! polymarket_monitor (PUB :5556) ────┼──> zmq_listener (SUB all)
//! game_shard (PUB :5558) ────────────┤         │
//! signal_processor (PUB :5559) ──────┤         ├──> Console logs
//! execution_service (PUB :5560) ─────┘         ├──> Redis streams
//!                                              └──> File logs (optional)
//! ```
//!
//! Modes:
//! - observer: Subscribe and log/persist only (default for zmq_only transport)
//! - bridge: Subscribe and forward to Redis pub/sub (legacy, for "both" transport)
//! - disabled: Exit immediately

use anyhow::Result;
use arbees_rust_core::models::TransportMode;
use chrono::{DateTime, Utc};
use dotenv::dotenv;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zeromq::{Socket, SocketRecv, SubSocket};

/// Listener operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenerMode {
    Observer, // Log and persist to streams only (no pub/sub forwarding)
    Bridge,   // Forward to Redis pub/sub (legacy behavior)
    Disabled, // Exit immediately
}

impl ListenerMode {
    fn from_env() -> Self {
        match env::var("ZMQ_LISTENER_MODE").ok().as_deref() {
            Some("observer") => ListenerMode::Observer,
            Some("bridge") => ListenerMode::Bridge,
            Some("disabled") => ListenerMode::Disabled,
            None => {
                // Infer from transport mode
                match TransportMode::from_env() {
                    TransportMode::ZmqOnly => ListenerMode::Observer,
                    TransportMode::Both => ListenerMode::Bridge,
                    TransportMode::RedisOnly => ListenerMode::Disabled,
                }
            }
            _ => ListenerMode::Observer,
        }
    }
}

/// ZMQ message envelope format
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ZmqEnvelope {
    seq: u64,
    timestamp_ms: i64,
    source: Option<String>,
    payload: serde_json::Value,
}

/// Parsed message for logging
#[derive(Debug, Clone, Serialize)]
struct ParsedMessage {
    recv_ts: i64,
    topic: String,
    source: String,
    seq: u64,
    msg_ts: i64,
    latency_ms: i64,
    msg_type: MessageType,
    summary: String,
    payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum MessageType {
    Price,
    Signal,
    Execution,
    Trade,
    Game,
    Unknown,
}

/// Listener statistics
#[derive(Debug, Default)]
struct ListenerStats {
    messages_received: AtomicU64,
    messages_logged: AtomicU64,
    messages_to_streams: AtomicU64,
    parse_errors: AtomicU64,
    redis_errors: AtomicU64,
    sequence_gaps: AtomicU64,
    last_seq_by_source: RwLock<HashMap<String, u64>>,
}

impl ListenerStats {
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_logged: self.messages_logged.load(Ordering::Relaxed),
            messages_to_streams: self.messages_to_streams.load(Ordering::Relaxed),
            parse_errors: self.parse_errors.load(Ordering::Relaxed),
            redis_errors: self.redis_errors.load(Ordering::Relaxed),
            sequence_gaps: self.sequence_gaps.load(Ordering::Relaxed),
        }
    }

    async fn check_sequence(&self, source: &str, seq: u64) -> bool {
        let mut seqs = self.last_seq_by_source.write().await;
        if let Some(last_seq) = seqs.get(source) {
            if seq > *last_seq + 1 {
                let gap = seq - *last_seq - 1;
                self.sequence_gaps.fetch_add(gap, Ordering::Relaxed);
                warn!(
                    "Sequence gap detected from {}: expected {}, got {} (gap={})",
                    source,
                    *last_seq + 1,
                    seq,
                    gap
                );
                seqs.insert(source.to_string(), seq);
                return false;
            }
        }
        seqs.insert(source.to_string(), seq);
        true
    }
}

#[derive(Debug, Clone, Serialize)]
struct StatsSnapshot {
    messages_received: u64,
    messages_logged: u64,
    messages_to_streams: u64,
    parse_errors: u64,
    redis_errors: u64,
    sequence_gaps: u64,
}

/// ZMQ endpoint configuration
#[derive(Clone)]
struct ZmqEndpoint {
    name: String,
    address: String,
    subscriptions: Vec<String>,
}

/// Observer configuration
struct ObserverConfig {
    console_log: bool,
    redis_streams: bool,
    file_log: Option<PathBuf>,
    log_format: LogFormat,
}

#[derive(Clone, Copy)]
enum LogFormat {
    Pretty,  // Human-readable one-liner
    Json,    // Full JSON
    Compact, // Compact JSON (no pretty print)
}

impl ObserverConfig {
    fn from_env() -> Self {
        let console_log = env::var("ZMQ_LISTENER_CONSOLE_LOG")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);

        let redis_streams = env::var("ZMQ_LISTENER_REDIS_STREAMS")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);

        let file_log = env::var("ZMQ_LISTENER_LOG_FILE").ok().map(PathBuf::from);

        let log_format = match env::var("ZMQ_LISTENER_LOG_FORMAT").ok().as_deref() {
            Some("json") => LogFormat::Json,
            Some("compact") => LogFormat::Compact,
            _ => LogFormat::Pretty,
        };

        Self {
            console_log,
            redis_streams,
            file_log,
            log_format,
        }
    }
}

/// ZMQ Listener - Observer and Bridge
struct ZmqListener {
    mode: ListenerMode,
    config: ObserverConfig,
    redis_client: redis::Client,
    endpoints: Vec<ZmqEndpoint>,
    stats: Arc<ListenerStats>,
    running: Arc<RwLock<bool>>,
    file_writer: Option<Arc<RwLock<File>>>,
}

impl ZmqListener {
    async fn new(mode: ListenerMode) -> Result<Self> {
        let config = ObserverConfig::from_env();

        // Connect to Redis
        let redis_url =
            env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379".to_string());
        let redis_client = redis::Client::open(redis_url)?;

        // Configure ZMQ endpoints
        let kalshi_endpoint = env::var("ZMQ_KALSHI_ENDPOINT")
            .unwrap_or_else(|_| "tcp://kalshi_monitor:5555".to_string());
        let polymarket_endpoint = env::var("ZMQ_POLYMARKET_ENDPOINT")
            .unwrap_or_else(|_| "tcp://polymarket_monitor:5556".to_string());
        let game_shard_endpoint = env::var("ZMQ_GAME_SHARD_ENDPOINT")
            .unwrap_or_else(|_| "tcp://game_shard:5558".to_string());
        let signal_processor_endpoint = env::var("ZMQ_SIGNAL_PROCESSOR_ENDPOINT")
            .unwrap_or_else(|_| "tcp://signal_processor:5559".to_string());
        let execution_endpoint = env::var("ZMQ_EXECUTION_ENDPOINT")
            .unwrap_or_else(|_| "tcp://execution_service:5560".to_string());

        let endpoints = vec![
            ZmqEndpoint {
                name: "kalshi_monitor".to_string(),
                address: kalshi_endpoint,
                subscriptions: vec!["prices.kalshi.".to_string()],
            },
            ZmqEndpoint {
                name: "polymarket_monitor".to_string(),
                address: polymarket_endpoint,
                subscriptions: vec!["prices.poly.".to_string()],
            },
            ZmqEndpoint {
                name: "game_shard".to_string(),
                address: game_shard_endpoint,
                subscriptions: vec!["signals.trade.".to_string(), "games.".to_string()],
            },
            ZmqEndpoint {
                name: "signal_processor".to_string(),
                address: signal_processor_endpoint,
                subscriptions: vec!["execution.".to_string()],
            },
            ZmqEndpoint {
                name: "execution_service".to_string(),
                address: execution_endpoint,
                subscriptions: vec!["trades.".to_string()],
            },
        ];

        // Open file writer if configured
        let file_writer = if let Some(ref path) = config.file_log {
            // Create parent directories if needed
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            Some(Arc::new(RwLock::new(file)))
        } else {
            None
        };

        Ok(Self {
            mode,
            config,
            redis_client,
            endpoints,
            stats: Arc::new(ListenerStats::default()),
            running: Arc::new(RwLock::new(true)),
            file_writer,
        })
    }

    async fn start(&self) -> Result<()> {
        info!(
            "Starting ZMQ Listener in {:?} mode...",
            self.mode
        );
        info!(
            "Config: console_log={}, redis_streams={}, file_log={:?}",
            self.config.console_log,
            self.config.redis_streams,
            self.config.file_log
        );

        // Test Redis connection
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let _: String = redis::cmd("PING").query_async(&mut conn).await?;
        info!("Redis connection verified");

        // Spawn listener tasks for each endpoint
        let mut handles = Vec::new();

        for endpoint in &self.endpoints {
            let endpoint_clone = endpoint.clone();
            let mode = self.mode;
            let config_console = self.config.console_log;
            let config_streams = self.config.redis_streams;
            let config_format = self.config.log_format;
            let redis_client = self.redis_client.clone();
            let stats = self.stats.clone();
            let running = self.running.clone();
            let file_writer = self.file_writer.clone();

            let handle = tokio::spawn(async move {
                Self::listen_to_endpoint(
                    endpoint_clone,
                    mode,
                    config_console,
                    config_streams,
                    config_format,
                    redis_client,
                    stats,
                    running,
                    file_writer,
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

        // Print startup banner
        println!();
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║           ZMQ LISTENER - OBSERVER MODE ACTIVE                ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║ Subscribed endpoints:                                        ║");
        for ep in &self.endpoints {
            println!("║   {} -> {}                    ", ep.name, ep.address);
        }
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║ Output: console={} streams={} file={:<20} ║",
            if self.config.console_log { "yes" } else { "no " },
            if self.config.redis_streams { "yes" } else { "no " },
            self.config.file_log.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".to_string())
        );
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();

        // Wait for all tasks
        for handle in handles {
            if let Err(e) = handle.await {
                error!("Task error: {}", e);
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn listen_to_endpoint(
        endpoint: ZmqEndpoint,
        mode: ListenerMode,
        console_log: bool,
        redis_streams: bool,
        log_format: LogFormat,
        redis_client: redis::Client,
        stats: Arc<ListenerStats>,
        running: Arc<RwLock<bool>>,
        file_writer: Option<Arc<RwLock<File>>>,
    ) -> Result<()> {
        loop {
            if !*running.read().await {
                info!("Stopping listener for {}", endpoint.name);
                break;
            }

            info!(
                "Connecting to ZMQ endpoint: {} at {}",
                endpoint.name, endpoint.address
            );

            // Create ZMQ SUB socket
            let mut socket = SubSocket::new();

            match socket.connect(&endpoint.address).await {
                Ok(_) => info!("Connected to ZMQ: {}", endpoint.name),
                Err(e) => {
                    warn!(
                        "Failed to connect to {}: {}. Retrying in 5s...",
                        endpoint.name, e
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }

            // Subscribe to topics
            for sub in &endpoint.subscriptions {
                if let Err(e) = socket.subscribe(sub).await {
                    warn!(
                        "Failed to subscribe to '{}' on {}: {}",
                        sub, endpoint.name, e
                    );
                }
            }

            info!(
                "Subscribed to topics on {}: {:?}",
                endpoint.name, endpoint.subscriptions
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

                let recv_result =
                    tokio::time::timeout(Duration::from_secs(30), socket.recv()).await;

                match recv_result {
                    Ok(Ok(msg)) => {
                        let recv_ts = Utc::now().timestamp_millis();
                        stats.messages_received.fetch_add(1, Ordering::Relaxed);

                        // Parse multipart message
                        let parts: Vec<_> = msg.iter().collect();
                        if parts.len() < 2 {
                            debug!(
                                "Received message with {} parts, expected 2+",
                                parts.len()
                            );
                            continue;
                        }

                        let topic = String::from_utf8_lossy(parts[0].as_ref()).to_string();
                        let payload_bytes = parts[1].as_ref();

                        // Parse envelope
                        let envelope: ZmqEnvelope = match serde_json::from_slice(payload_bytes) {
                            Ok(e) => e,
                            Err(e) => {
                                stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                                debug!(
                                    "Failed to parse ZMQ message from {}: {}",
                                    endpoint.name, e
                                );
                                continue;
                            }
                        };

                        // Check sequence
                        let source_key = format!(
                            "{}.{}",
                            endpoint.name,
                            envelope.source.as_deref().unwrap_or("unknown")
                        );
                        stats.check_sequence(&source_key, envelope.seq).await;

                        // Parse and format message
                        let parsed = Self::parse_message(
                            recv_ts,
                            &topic,
                            &endpoint.name,
                            &envelope,
                        );

                        // Console logging
                        if console_log {
                            Self::log_to_console(&parsed, log_format);
                            stats.messages_logged.fetch_add(1, Ordering::Relaxed);
                        }

                        // File logging
                        if let Some(ref writer) = file_writer {
                            if let Ok(mut file) = writer.write().await.try_write_all(
                                format!("{}\n", serde_json::to_string(&parsed).unwrap_or_default())
                                    .as_bytes(),
                            ) {
                                // Written
                            }
                        }

                        // Redis streams (observer mode)
                        if redis_streams {
                            if let Err(e) = Self::write_to_stream(
                                &mut redis_conn,
                                &topic,
                                &envelope,
                                recv_ts,
                            )
                            .await
                            {
                                stats.redis_errors.fetch_add(1, Ordering::Relaxed);
                                debug!("Failed to write to stream: {}", e);
                            } else {
                                stats.messages_to_streams.fetch_add(1, Ordering::Relaxed);
                            }
                        }

                        // Bridge mode: also forward to pub/sub
                        if mode == ListenerMode::Bridge {
                            if let Err(e) =
                                Self::forward_to_pubsub(&mut redis_conn, &topic, &envelope).await
                            {
                                stats.redis_errors.fetch_add(1, Ordering::Relaxed);
                                warn!("Failed to forward to pub/sub: {}", e);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        warn!(
                            "ZMQ receive error on {}: {}. Reconnecting...",
                            endpoint.name, e
                        );
                        break;
                    }
                    Err(_) => {
                        // Timeout - normal for low traffic
                        debug!("No message from {} in 30s", endpoint.name);
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        Ok(())
    }

    fn parse_message(
        recv_ts: i64,
        topic: &str,
        source: &str,
        envelope: &ZmqEnvelope,
    ) -> ParsedMessage {
        let msg_type = if topic.starts_with("prices.") {
            MessageType::Price
        } else if topic.starts_with("signals.") {
            MessageType::Signal
        } else if topic.starts_with("execution.") {
            MessageType::Execution
        } else if topic.starts_with("trades.") {
            MessageType::Trade
        } else if topic.starts_with("games.") {
            MessageType::Game
        } else {
            MessageType::Unknown
        };

        let latency_ms = recv_ts - envelope.timestamp_ms;

        // Build summary based on message type
        let summary = match msg_type {
            MessageType::Price => {
                let bid = envelope
                    .payload
                    .get("yes_bid")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let ask = envelope
                    .payload
                    .get("yes_ask")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let liq = envelope
                    .payload
                    .get("liquidity")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                format!("bid={:.3} ask={:.3} liq=${:.0}", bid, ask, liq)
            }
            MessageType::Signal => {
                let game_id = envelope
                    .payload
                    .get("game_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let edge = envelope
                    .payload
                    .get("edge_percentage")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                format!("game={} edge={:.2}%", game_id, edge)
            }
            MessageType::Execution => {
                let signal_id = envelope
                    .payload
                    .get("signal_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("signal={}", signal_id)
            }
            MessageType::Trade => {
                let status = envelope
                    .payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let platform = envelope
                    .payload
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("status={} platform={}", status, platform)
            }
            MessageType::Game => {
                let game_id = envelope
                    .payload
                    .get("game_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("game_id={}", game_id)
            }
            MessageType::Unknown => "unknown".to_string(),
        };

        ParsedMessage {
            recv_ts,
            topic: topic.to_string(),
            source: source.to_string(),
            seq: envelope.seq,
            msg_ts: envelope.timestamp_ms,
            latency_ms,
            msg_type,
            summary,
            payload: envelope.payload.clone(),
        }
    }

    fn log_to_console(msg: &ParsedMessage, format: LogFormat) {
        match format {
            LogFormat::Pretty => {
                let ts = DateTime::from_timestamp_millis(msg.recv_ts)
                    .map(|dt| dt.format("%H:%M:%S%.3f").to_string())
                    .unwrap_or_else(|| "?".to_string());

                let type_str = match msg.msg_type {
                    MessageType::Price => "PRICE",
                    MessageType::Signal => "SIGNAL",
                    MessageType::Execution => "EXEC",
                    MessageType::Trade => "TRADE",
                    MessageType::Game => "GAME",
                    MessageType::Unknown => "???",
                };

                // Color codes for terminal
                let color = match msg.msg_type {
                    MessageType::Price => "\x1b[36m",      // Cyan
                    MessageType::Signal => "\x1b[33m",    // Yellow
                    MessageType::Execution => "\x1b[35m", // Magenta
                    MessageType::Trade => "\x1b[32m",     // Green
                    MessageType::Game => "\x1b[34m",      // Blue
                    MessageType::Unknown => "\x1b[37m",   // White
                };
                let reset = "\x1b[0m";

                println!(
                    "[{}] {}{:<6}{} {:20} seq={:<8} lat={:>3}ms {}",
                    ts,
                    color,
                    type_str,
                    reset,
                    msg.source,
                    msg.seq,
                    msg.latency_ms,
                    msg.summary
                );
            }
            LogFormat::Json => {
                println!("{}", serde_json::to_string_pretty(msg).unwrap_or_default());
            }
            LogFormat::Compact => {
                println!("{}", serde_json::to_string(msg).unwrap_or_default());
            }
        }
    }

    async fn write_to_stream(
        conn: &mut redis::aio::MultiplexedConnection,
        topic: &str,
        envelope: &ZmqEnvelope,
        recv_ts: i64,
    ) -> Result<()> {
        let payload_str = serde_json::to_string(&envelope.payload)?;

        // Determine stream key based on topic
        let stream_key = if topic.starts_with("prices.kalshi.") {
            "stream:prices:kalshi"
        } else if topic.starts_with("prices.poly.") {
            "stream:prices:polymarket"
        } else if topic.starts_with("signals.") {
            "stream:signals"
        } else if topic.starts_with("execution.") {
            "stream:executions"
        } else if topic.starts_with("trades.") {
            "stream:trades"
        } else if topic.starts_with("games.") {
            "stream:games"
        } else {
            "stream:unknown"
        };

        // Determine MAXLEN based on stream type
        let maxlen = if stream_key.starts_with("stream:prices") {
            50000 // More price data
        } else {
            5000 // Less for signals/trades
        };

        let _: String = redis::cmd("XADD")
            .arg(stream_key)
            .arg("MAXLEN")
            .arg("~")
            .arg(maxlen)
            .arg("*")
            .arg("topic")
            .arg(topic)
            .arg("payload")
            .arg(&payload_str)
            .arg("zmq_seq")
            .arg(envelope.seq)
            .arg("zmq_ts")
            .arg(envelope.timestamp_ms)
            .arg("recv_ts")
            .arg(recv_ts)
            .arg("source")
            .arg(envelope.source.as_deref().unwrap_or("unknown"))
            .query_async(conn)
            .await?;

        Ok(())
    }

    async fn forward_to_pubsub(
        conn: &mut redis::aio::MultiplexedConnection,
        topic: &str,
        envelope: &ZmqEnvelope,
    ) -> Result<()> {
        let payload_str = serde_json::to_string(&envelope.payload)?;

        if topic.starts_with("prices.") {
            // Price messages go to game:{game_id}:price
            if let Some(game_id) = envelope.payload.get("game_id").and_then(|v| v.as_str()) {
                let channel = format!("game:{}:price", game_id);
                let _: () = redis::cmd("PUBLISH")
                    .arg(&channel)
                    .arg(&payload_str)
                    .query_async(conn)
                    .await?;
            }
        } else if topic.starts_with("signals.") {
            // Signal messages go to signals:new
            let _: () = redis::cmd("PUBLISH")
                .arg("signals:new")
                .arg(&payload_str)
                .query_async(conn)
                .await?;
        } else if topic.starts_with("games.") {
            // Game state messages
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
                "ZMQ Observer stats: received={} logged={} streams={} gaps={} errors=parse:{}/redis:{}",
                snapshot.messages_received,
                snapshot.messages_logged,
                snapshot.messages_to_streams,
                snapshot.sequence_gaps,
                snapshot.parse_errors,
                snapshot.redis_errors
            );

            // Publish health status
            if let Ok(mut conn) = redis_client.get_multiplexed_async_connection().await {
                let health = serde_json::json!({
                    "service": "zmq_listener_rust",
                    "mode": "observer",
                    "healthy": true,
                    "messages_received": snapshot.messages_received,
                    "messages_logged": snapshot.messages_logged,
                    "messages_to_streams": snapshot.messages_to_streams,
                    "sequence_gaps": snapshot.sequence_gaps,
                    "parse_errors": snapshot.parse_errors,
                    "redis_errors": snapshot.redis_errors,
                    "timestamp": Utc::now().to_rfc3339(),
                });

                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("health:heartbeats")
                    .arg(health.to_string())
                    .query_async(&mut conn)
                    .await;

                // Also set a key for easy status check
                let _: Result<(), _> = redis::cmd("SET")
                    .arg("service:zmq_listener:status")
                    .arg(health.to_string())
                    .arg("EX")
                    .arg(120) // Expire in 2 minutes
                    .query_async(&mut conn)
                    .await;
            }
        }

        Ok(())
    }
}

// Helper trait for file writing
trait TryWriteAll {
    fn try_write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
}

impl TryWriteAll for File {
    fn try_write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.write_all(buf)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting ZMQ Listener Rust Service...");

    let mode = ListenerMode::from_env();
    info!("Listener mode: {:?}", mode);

    match mode {
        ListenerMode::Disabled => {
            info!("ZMQ Listener is disabled (redis_only mode or explicit disable)");
            info!("Exiting gracefully");
            return Ok(());
        }
        ListenerMode::Observer => {
            info!("Starting in OBSERVER mode - logging and streaming only");
        }
        ListenerMode::Bridge => {
            info!("Starting in BRIDGE mode - forwarding to Redis pub/sub");
        }
    }

    let listener = ZmqListener::new(mode).await?;
    listener.start().await?;

    Ok(())
}
