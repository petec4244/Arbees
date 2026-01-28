use crate::config::Config;
use crate::managers::service_registry::ServiceRegistry;
use crate::state::ShardInfo;
use chrono::Utc;
use std::sync::Arc;
use tracing::info;

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

    /// Get the best shard for assignment
    /// Filters by: GameShard type, Healthy status, Circuit Closed, Has Capacity
    pub async fn get_best_shard(&self) -> Option<ShardInfo> {
        let healthy_shards = self.service_registry.get_healthy_shards().await;

        if healthy_shards.is_empty() {
            return None;
        }

        // Convert ServiceState to ShardInfo and find shard with most capacity
        let best_state = healthy_shards
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
