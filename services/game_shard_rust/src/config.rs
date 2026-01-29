//! Configuration constants and environment loading for GameShard
//!
//! This module manages all runtime configuration:
//! - Edge calculation thresholds
//! - Polling and monitoring intervals
//! - Circuit breaker settings
//! - ZMQ transport configuration
//! - Database connection parameters

use anyhow::Result;
use arbees_rust_core::circuit_breaker::ApiCircuitBreakerConfig;
use std::env;
use std::time::Duration;

/// Default minimum edge percentage to generate a signal (can be overridden via MIN_EDGE_PCT env var)
/// Data shows: 5-10% edge = 36% win rate, 15%+ edge = 87.5% win rate
pub const DEFAULT_MIN_EDGE_PCT: f64 = 15.0;

/// Maximum probability to buy (avoid buying near-certain outcomes)
pub const MAX_BUY_PROB: f64 = 0.95;

/// Minimum probability to buy (avoid buying very unlikely outcomes)
pub const MIN_BUY_PROB: f64 = 0.05;

/// Default database URL for PostgreSQL/TimescaleDB
pub const DEFAULT_DATABASE_URL: &str = "postgresql://arbees:arbees@localhost:5432/arbees";

/// Default polling interval in seconds
pub const DEFAULT_POLL_INTERVAL_SECS: f64 = 0.5;

/// Default heartbeat interval in seconds
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Default maximum concurrent games per shard
pub const DEFAULT_MAX_GAMES: usize = 20;

/// Default ZMQ PUB port for signals
pub const DEFAULT_ZMQ_PUB_PORT: u16 = 5558;

/// Default ZMQ SUB endpoints for price ingestion
pub const DEFAULT_ZMQ_SUB_ENDPOINTS: &str =
    "tcp://kalshi_monitor:5555,tcp://polymarket_monitor:5556";

/// Configuration for game polling and monitoring
#[derive(Debug, Clone)]
pub struct GameMonitorConfig {
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub max_games: usize,
    pub min_edge_pct: f64,
}

impl GameMonitorConfig {
    /// Load configuration from environment variables with sensible defaults
    pub fn from_env() -> Self {
        let poll_interval_secs = env::var("POLL_INTERVAL")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
            .clamp(0.1, 5.0);

        let heartbeat_interval = Duration::from_secs(
            env::var("HEARTBEAT_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_SECS),
        );

        let max_games = env::var("MAX_GAMES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_GAMES);

        let min_edge_pct = env::var("MIN_EDGE_PCT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_MIN_EDGE_PCT);

        Self {
            poll_interval: Duration::from_secs_f64(poll_interval_secs),
            heartbeat_interval,
            max_games,
            min_edge_pct,
        }
    }
}

/// Load ESPN circuit breaker configuration from environment
pub fn load_espn_circuit_breaker_config() -> ApiCircuitBreakerConfig {
    ApiCircuitBreakerConfig {
        failure_threshold: env::var("ESPN_CB_FAILURE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
        recovery_timeout: Duration::from_secs(
            env::var("ESPN_CB_RECOVERY_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        ),
        success_threshold: 2,
    }
}

/// Load database URL from environment or use default
pub fn load_database_url() -> String {
    env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string())
}

/// Load ZMQ subscriber endpoints from environment
pub fn load_zmq_sub_endpoints() -> Vec<String> {
    env::var("ZMQ_SUB_ENDPOINTS")
        .unwrap_or_else(|_| DEFAULT_ZMQ_SUB_ENDPOINTS.to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Load ZMQ publisher port from environment
pub fn load_zmq_pub_port() -> u16 {
    env::var("ZMQ_PUB_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ZMQ_PUB_PORT)
}
