//! Database connection pooling and configuration.
//!
//! This module provides standardized connection pool creation with:
//! - Consistent timeout and connection settings across services
//! - Configurable pool sizes based on service requirements
//! - Health check and idle timeout management

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::env;
use std::time::Duration;

/// Database pool configuration
#[derive(Debug, Clone)]
pub struct DbPoolConfig {
    /// Maximum number of connections in the pool
    pub max_connections: u32,
    /// Minimum number of connections to maintain
    pub min_connections: u32,
    /// Timeout for acquiring a connection
    pub acquire_timeout: Duration,
    /// How long idle connections are kept alive
    pub idle_timeout: Duration,
    /// Maximum lifetime of a connection
    pub max_lifetime: Duration,
}

impl Default for DbPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),   // 5 minutes
            max_lifetime: Duration::from_secs(1800),  // 30 minutes
        }
    }
}

impl DbPoolConfig {
    /// Configuration for services with high concurrency (e.g., signal processor)
    pub fn high_concurrency() -> Self {
        Self {
            max_connections: 15,
            min_connections: 3,
            ..Default::default()
        }
    }

    /// Configuration for services with low concurrency (e.g., game shards)
    pub fn low_concurrency() -> Self {
        Self {
            max_connections: 5,
            min_connections: 1,
            ..Default::default()
        }
    }

    /// Create config from environment variables with fallback to provided defaults
    pub fn from_env_with_defaults(defaults: Self) -> Self {
        Self {
            max_connections: env::var("DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.max_connections),
            min_connections: env::var("DB_MIN_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.min_connections),
            acquire_timeout: env::var("DB_ACQUIRE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.acquire_timeout),
            idle_timeout: env::var("DB_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.idle_timeout),
            max_lifetime: env::var("DB_MAX_LIFETIME_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.max_lifetime),
        }
    }
}

/// Create a database connection pool with the given configuration.
///
/// # Arguments
/// * `database_url` - PostgreSQL connection URL
/// * `config` - Pool configuration settings
///
/// # Example
/// ```ignore
/// let config = DbPoolConfig::high_concurrency();
/// let pool = create_pool(&database_url, &config).await?;
/// ```
pub async fn create_pool(database_url: &str, config: &DbPoolConfig) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.acquire_timeout)
        .idle_timeout(config.idle_timeout)
        .max_lifetime(config.max_lifetime)
        .connect(database_url)
        .await
        .context("Failed to create database connection pool")?;

    tracing::info!(
        "Database pool created: max={}, min={}, acquire_timeout={}s",
        config.max_connections,
        config.min_connections,
        config.acquire_timeout.as_secs()
    );

    Ok(pool)
}

/// Create a database connection pool with default configuration.
///
/// Uses DATABASE_URL environment variable.
pub async fn create_default_pool() -> Result<PgPool> {
    let database_url = env::var("DATABASE_URL")
        .context("DATABASE_URL environment variable must be set")?;

    let config = DbPoolConfig::from_env_with_defaults(DbPoolConfig::default());
    create_pool(&database_url, &config).await
}

/// Create a high-concurrency pool for services like signal processor.
pub async fn create_high_concurrency_pool() -> Result<PgPool> {
    let database_url = env::var("DATABASE_URL")
        .context("DATABASE_URL environment variable must be set")?;

    let config = DbPoolConfig::from_env_with_defaults(DbPoolConfig::high_concurrency());
    create_pool(&database_url, &config).await
}

/// Create a low-concurrency pool for services like game shards.
pub async fn create_low_concurrency_pool() -> Result<PgPool> {
    let database_url = env::var("DATABASE_URL")
        .context("DATABASE_URL environment variable must be set")?;

    let config = DbPoolConfig::from_env_with_defaults(DbPoolConfig::low_concurrency());
    create_pool(&database_url, &config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DbPoolConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.acquire_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_high_concurrency_config() {
        let config = DbPoolConfig::high_concurrency();
        assert_eq!(config.max_connections, 15);
        assert_eq!(config.min_connections, 3);
    }

    #[test]
    fn test_low_concurrency_config() {
        let config = DbPoolConfig::low_concurrency();
        assert_eq!(config.max_connections, 5);
        assert_eq!(config.min_connections, 1);
    }
}
