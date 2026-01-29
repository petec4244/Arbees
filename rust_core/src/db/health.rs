//! Database connection health monitoring
//!
//! Provides health checks and monitoring for database connection pools.

use anyhow::{Context, Result};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Check if database pool is healthy
pub async fn check_pool_health(pool: &PgPool) -> Result<()> {
    sqlx::query("SELECT 1")
        .fetch_one(pool)
        .await
        .context("Database health check failed")?;
    Ok(())
}

/// Configuration for pool health monitoring
#[derive(Clone, Debug)]
pub struct PoolHealthConfig {
    /// Interval between health checks
    pub check_interval: Duration,
    /// Number of consecutive failures before alerting
    pub alert_threshold: u32,
    /// Whether to enable health monitoring
    pub enabled: bool,
}

impl Default for PoolHealthConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl PoolHealthConfig {
    pub fn from_env() -> Self {
        Self {
            check_interval: Duration::from_secs(
                std::env::var("DB_HEALTH_CHECK_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
            alert_threshold: std::env::var("DB_HEALTH_ALERT_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            enabled: std::env::var("DB_HEALTH_CHECK_ENABLED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(true),
        }
    }
}

/// Monitor that continuously checks database pool health
pub struct PoolHealthMonitor {
    pool: PgPool,
    config: PoolHealthConfig,
}

impl PoolHealthMonitor {
    /// Create new pool health monitor
    pub fn new(pool: PgPool, config: PoolHealthConfig) -> Self {
        Self { pool, config }
    }

    /// Start monitoring loop (runs forever)
    pub async fn start_monitoring(self) {
        if !self.config.enabled {
            info!("Database health monitoring is disabled");
            return;
        }

        info!(
            "Starting database health monitoring (interval: {:?}, alert threshold: {})",
            self.config.check_interval, self.config.alert_threshold
        );

        let mut consecutive_failures = 0u32;

        loop {
            match check_pool_health(&self.pool).await {
                Ok(_) => {
                    if consecutive_failures > 0 {
                        info!(
                            "Database connection recovered after {} failures",
                            consecutive_failures
                        );
                        consecutive_failures = 0;
                    }
                }
                Err(e) => {
                    consecutive_failures += 1;
                    error!(
                        "Database health check failed (attempt {}/{}): {}",
                        consecutive_failures, self.config.alert_threshold, e
                    );

                    if consecutive_failures >= self.config.alert_threshold {
                        error!(
                            "CRITICAL: Database health check failed {} times in a row!",
                            consecutive_failures
                        );
                        // Alert would be sent via CriticalAlertClient in production
                        // For now, just log the critical error
                    }
                }
            }

            tokio::time::sleep(self.config.check_interval).await;
        }
    }

    /// Start monitoring in background task
    pub fn start_background(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.start_monitoring().await;
        })
    }
}

/// Get database pool statistics
pub async fn get_pool_stats(pool: &PgPool) -> PoolStats {
    PoolStats {
        size: pool.size(),
        idle: pool.num_idle(),
    }
}

/// Database pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total number of connections in the pool
    pub size: u32,
    /// Number of idle connections
    pub idle: usize,
}

impl PoolStats {
    pub fn active(&self) -> u32 {
        self.size.saturating_sub(self.idle as u32)
    }
}
