//! System-wide health monitoring and critical alerting
//!
//! Monitors critical system components and sends alerts when
//! catastrophic failures occur that require immediate attention.

use crate::managers::service_registry::ServiceRegistry;
use crate::state::ServiceStatus;
use arbees_rust_core::alerts::{CriticalAlert, CriticalAlertClient};
use arbees_rust_core::redis::RedisBus;
use chrono::Utc;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{error, info, warn};

/// System monitor that performs health checks and sends critical alerts
pub struct SystemMonitor {
    service_registry: Arc<ServiceRegistry>,
    redis_bus: Arc<RedisBus>,
    db_pool: PgPool,
    alert_client: CriticalAlertClient,
}

impl SystemMonitor {
    /// Create new system monitor
    pub fn new(
        service_registry: Arc<ServiceRegistry>,
        redis_bus: Arc<RedisBus>,
        db_pool: PgPool,
        alert_client: CriticalAlertClient,
    ) -> Self {
        Self {
            service_registry,
            redis_bus,
            db_pool,
            alert_client,
        }
    }

    /// Perform all system health checks and send alerts if needed
    pub async fn check_system_health(&self) {
        // 1. Check if all game shards are unhealthy
        self.check_shard_health().await;

        // 2. Check Redis connectivity
        self.check_redis_health().await;

        // 3. Check database connectivity
        self.check_database_health().await;

        // 4. Check market discovery services
        self.check_market_discovery_health().await;
    }

    /// Check if all game shards are unhealthy
    async fn check_shard_health(&self) {
        let services = self.service_registry.get_services().await;

        // Filter for game shards
        let game_shards: Vec<_> = services
            .iter()
            .filter(|(_, state)| matches!(state.service_type, crate::state::ServiceType::GameShard))
            .collect();

        if game_shards.is_empty() {
            // No shards registered yet, don't alert
            return;
        }

        // Check if any shards are healthy
        let healthy_shards = game_shards
            .iter()
            .filter(|(_, state)| matches!(state.status, ServiceStatus::Healthy))
            .count();

        if healthy_shards == 0 {
            warn!(
                "CRITICAL: All {} game shards are unhealthy!",
                game_shards.len()
            );

            self.alert_client
                .send_critical_alert(CriticalAlert::AllShardsUnhealthy {
                    total_shards: game_shards.len(),
                    timestamp: Utc::now(),
                })
                .await;
        }
    }

    /// Check Redis connectivity
    async fn check_redis_health(&self) {
        match self.redis_bus.health_check_result().await {
            Ok(_) => {
                // Redis is healthy, no action needed
            }
            Err(e) => {
                error!("CRITICAL: Redis health check failed: {}", e);

                self.alert_client
                    .send_critical_alert(CriticalAlert::RedisConnectivityIssue {
                        reason: e.to_string(),
                        timestamp: Utc::now(),
                    })
                    .await;
            }
        }
    }

    /// Check database connectivity
    async fn check_database_health(&self) {
        match sqlx::query("SELECT 1").fetch_one(&self.db_pool).await {
            Ok(_) => {
                // Database is healthy, no action needed
            }
            Err(e) => {
                error!("CRITICAL: Database health check failed: {}", e);

                self.alert_client
                    .send_critical_alert(CriticalAlert::DatabaseConnectivityIssue {
                        reason: e.to_string(),
                        timestamp: Utc::now(),
                    })
                    .await;
            }
        }
    }

    /// Check if market discovery services are available
    async fn check_market_discovery_health(&self) {
        let services = self.service_registry.get_services().await;

        // Check for any healthy market discovery services
        let has_healthy_discovery = services.iter().any(|(_, state)| {
            matches!(
                state.service_type,
                crate::state::ServiceType::PolymarketMonitor | crate::state::ServiceType::KalshiMonitor
            ) && matches!(state.status, ServiceStatus::Healthy)
        });

        if !has_healthy_discovery {
            warn!("CRITICAL: No healthy market discovery services available");

            self.alert_client
                .send_critical_alert(CriticalAlert::NoMarketDiscoveryServices {
                    timestamp: Utc::now(),
                })
                .await;
        }
    }

    /// Get system health summary
    pub async fn get_health_summary(&self) -> SystemHealthSummary {
        let services = self.service_registry.get_services().await;

        let total_services = services.len();
        let healthy_services = services
            .iter()
            .filter(|(_, state)| matches!(state.status, ServiceStatus::Healthy))
            .count();

        let game_shards = services
            .iter()
            .filter(|(_, state)| matches!(state.service_type, crate::state::ServiceType::GameShard))
            .count();
        let healthy_shards = services
            .iter()
            .filter(|(_, state)| {
                matches!(state.service_type, crate::state::ServiceType::GameShard)
                    && matches!(state.status, ServiceStatus::Healthy)
            })
            .count();

        let redis_healthy = self.redis_bus.health_check().await;
        let db_healthy = sqlx::query("SELECT 1")
            .fetch_one(&self.db_pool)
            .await
            .is_ok();

        SystemHealthSummary {
            total_services,
            healthy_services,
            game_shards,
            healthy_shards,
            redis_healthy,
            database_healthy: db_healthy,
        }
    }
}

/// Summary of system health status
#[derive(Debug, Clone)]
pub struct SystemHealthSummary {
    pub total_services: usize,
    pub healthy_services: usize,
    pub game_shards: usize,
    pub healthy_shards: usize,
    pub redis_healthy: bool,
    pub database_healthy: bool,
}

impl SystemHealthSummary {
    pub fn is_critical(&self) -> bool {
        !self.redis_healthy
            || !self.database_healthy
            || (self.game_shards > 0 && self.healthy_shards == 0)
    }
}
