//! Standardized database connection pool configuration
//!
//! Provides consistent pool settings across all services for reliability.

use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

/// Database pool configuration
#[derive(Clone, Debug)]
pub struct DbPoolConfig {
    /// Maximum number of connections in the pool
    pub max_connections: u32,
    /// Minimum number of idle connections to maintain
    pub min_connections: u32,
    /// Maximum lifetime of a connection (prevents stale connections)
    pub max_lifetime: Duration,
    /// Maximum idle time before a connection is closed
    pub idle_timeout: Duration,
    /// Connection timeout
    pub acquire_timeout: Duration,
    /// Enable statement logging
    pub enable_logging: bool,
}

impl Default for DbPoolConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl DbPoolConfig {
    /// Load configuration from environment variables with sensible defaults
    pub fn from_env() -> Self {
        Self {
            max_connections: std::env::var("DB_POOL_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            min_connections: std::env::var("DB_POOL_MIN_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            max_lifetime: Duration::from_secs(
                std::env::var("DB_POOL_MAX_LIFETIME_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1800), // 30 minutes
            ),
            idle_timeout: Duration::from_secs(
                std::env::var("DB_POOL_IDLE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(600), // 10 minutes
            ),
            acquire_timeout: Duration::from_secs(
                std::env::var("DB_POOL_ACQUIRE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
            enable_logging: std::env::var("DB_POOL_ENABLE_LOGGING")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),
        }
    }

    /// Create configuration optimized for high-throughput services
    pub fn high_throughput() -> Self {
        Self {
            max_connections: 50,
            min_connections: 10,
            max_lifetime: Duration::from_secs(1800),
            idle_timeout: Duration::from_secs(300),
            acquire_timeout: Duration::from_secs(30),
            enable_logging: false,
        }
    }

    /// Create configuration optimized for low-latency services
    pub fn low_latency() -> Self {
        Self {
            max_connections: 10,
            min_connections: 5,
            max_lifetime: Duration::from_secs(1800),
            idle_timeout: Duration::from_secs(600),
            acquire_timeout: Duration::from_secs(5),
            enable_logging: false,
        }
    }
}

/// Create a PostgreSQL connection pool with standardized configuration
pub async fn create_pool(database_url: &str, config: DbPoolConfig) -> Result<PgPool> {
    info!(
        "Creating database pool: max={}, min={}, max_lifetime={:?}, idle_timeout={:?}",
        config.max_connections, config.min_connections, config.max_lifetime, config.idle_timeout
    );

    // Parse connection options
    let connect_opts = PgConnectOptions::from_str(database_url)
        .context("Failed to parse database URL")?;

    // Build pool with configuration
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .max_lifetime(config.max_lifetime)
        .idle_timeout(config.idle_timeout)
        .acquire_timeout(config.acquire_timeout)
        .connect_with(connect_opts)
        .await
        .context("Failed to create database pool")?;

    info!("Database pool created successfully");
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DbPoolConfig::default();
        assert!(config.max_connections > 0);
        assert!(config.min_connections > 0);
        assert!(config.min_connections <= config.max_connections);
    }

    #[test]
    fn test_high_throughput_config() {
        let config = DbPoolConfig::high_throughput();
        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 10);
    }

    #[test]
    fn test_low_latency_config() {
        let config = DbPoolConfig::low_latency();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 5);
    }
}
