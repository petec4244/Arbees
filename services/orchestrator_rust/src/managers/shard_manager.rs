use crate::config::Config;
use crate::managers::service_registry::ServiceRegistry;
use crate::state::{ServiceType, ShardInfo};
use arbees_rust_core::models::MarketType;
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, info};

/// ShardManager acts as a facade/adapter over ServiceRegistry for backward compatibility.
/// It provides shard-specific methods while delegating to the comprehensive ServiceRegistry.
pub struct ShardManager {
    service_registry: Arc<ServiceRegistry>,
    config: Config,
}

impl ShardManager {
    pub fn new(service_registry: Arc<ServiceRegistry>, config: Config) -> Self {
        Self {
            service_registry,
            config,
        }
    }

    /// Handle shard heartbeat by delegating to ServiceRegistry
    pub async fn handle_heartbeat(&self, payload: serde_json::Value) {
        if let Err(e) = self.service_registry.handle_heartbeat(payload).await {
            tracing::warn!("Failed to handle shard heartbeat: {}", e);
        }
    }

    /// Get the best shard for assignment (GameShard only)
    /// Filters by: GameShard type, Healthy status, Circuit Closed, Has Capacity
    pub async fn get_best_shard(&self) -> Option<ShardInfo> {
        let healthy_shards = self
            .service_registry
            .get_healthy_shards_by_type(ServiceType::GameShard)
            .await;
        self.select_best_from_shards(healthy_shards)
    }

    /// Get the best shard for a specific market type
    ///
    /// Routes markets to appropriate shard types:
    /// - Sports -> GameShard
    /// - Crypto/Economics/Politics -> CryptoShard (with GameShard fallback)
    pub async fn get_best_shard_for_type(&self, market_type: &MarketType) -> Option<ShardInfo> {
        let preferred_shard_type = match market_type {
            MarketType::Sport { .. } => ServiceType::GameShard,
            MarketType::Crypto { .. } => ServiceType::CryptoShard,
            MarketType::Economics { .. } => ServiceType::CryptoShard,
            MarketType::Politics { .. } => ServiceType::CryptoShard,
            MarketType::Entertainment { .. } => ServiceType::CryptoShard,
        };

        debug!(
            "Looking for {:?} shard for market type {:?}",
            preferred_shard_type, market_type
        );

        // Try to get shards of the preferred type
        let preferred_shards = self
            .service_registry
            .get_healthy_shards_by_type(preferred_shard_type.clone())
            .await;

        if let Some(shard) = self.select_best_from_shards(preferred_shards) {
            debug!(
                "Found preferred shard {} for {:?}",
                shard.shard_id, market_type
            );
            return Some(shard);
        }

        // Fallback: for non-sports, try GameShard
        if !matches!(market_type, MarketType::Sport { .. }) {
            debug!("No CryptoShards available, falling back to GameShard");
            let fallback_shards = self
                .service_registry
                .get_healthy_shards_by_type(ServiceType::GameShard)
                .await;

            if let Some(shard) = self.select_best_from_shards(fallback_shards) {
                debug!("Using GameShard fallback: {}", shard.shard_id);
                return Some(shard);
            }
        }

        // Last resort: any healthy shard
        debug!("No typed shards available, trying any healthy shard");
        self.get_best_shard().await
    }

    /// Select the best shard from a list based on available capacity
    fn select_best_from_shards(&self, shards: Vec<crate::state::ServiceState>) -> Option<ShardInfo> {
        if shards.is_empty() {
            return None;
        }

        // Find shard with most capacity
        let best_state = shards
            .iter()
            .filter(|s| s.available_capacity() > 0)
            .max_by_key(|s| s.available_capacity())?;

        // Extract game count and max_games from metrics
        let max_games = best_state
            .metrics
            .get("max_games")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;

        let game_count = best_state.assigned_games.len();

        Some(ShardInfo {
            shard_id: best_state.instance_id.clone(),
            game_count,
            max_games,
            games: best_state.assigned_games.iter().cloned().collect(),
            last_heartbeat: best_state.last_heartbeat,
            is_healthy: true, // Already filtered by healthy status
        })
    }

    /// Check health of all services (delegated to ServiceRegistry)
    pub async fn check_health(&self) {
        self.service_registry.check_health().await;
    }

    /// Track a game/event assignment to a shard
    /// This prevents zombie detection from removing legitimately assigned games
    pub async fn track_assignment(&self, shard_id: &str, game_id: &str) {
        self.service_registry
            .track_assignment(shard_id, game_id)
            .await;
    }

    /// Get snapshot of all shards for monitoring/debugging
    pub async fn get_shards_snapshot(&self) -> Vec<ShardInfo> {
        let shards = self.service_registry.get_healthy_shards().await;

        shards
            .iter()
            .map(|state| {
                let max_games = state
                    .metrics
                    .get("max_games")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;

                let game_count = state.assigned_games.len();

                ShardInfo {
                    shard_id: state.instance_id.clone(),
                    game_count,
                    max_games,
                    games: state.assigned_games.iter().cloned().collect(),
                    last_heartbeat: state.last_heartbeat,
                    is_healthy: true,
                }
            })
            .collect()
    }
}
