use crate::config::Config;
use crate::state::ShardInfo;
use chrono::{Utc, DateTime};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

pub struct ShardManager {
    shards: Arc<RwLock<HashMap<String, ShardInfo>>>,
    config: Config,
}

impl ShardManager {
    pub fn new(config: Config) -> Self {
        Self {
            shards: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub async fn handle_heartbeat(&self, payload: serde_json::Value) {
        // Expected payload: { "shard_id": "...", "game_count": 5, "max_games": 20, "games": [...] }
        let shard_id = match payload.get("shard_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };

        let game_count = payload.get("game_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let max_games = payload.get("max_games").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let games: Vec<String> = payload.get("games")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let mut shards = self.shards.write().await;
        
        let should_log_discovery = !shards.contains_key(&shard_id);
        
        shards.insert(shard_id.clone(), ShardInfo {
            shard_id: shard_id.clone(),
            game_count,
            max_games,
            games,
            last_heartbeat: Utc::now(),
            is_healthy: true,
        });

        if should_log_discovery {
            info!("Discovered new shard: {}", shard_id);
        }
    }

    pub async fn get_best_shard(&self) -> Option<ShardInfo> {
        let shards = self.shards.read().await;
        let now = Utc::now();
        let timeout_secs = self.config.shard_timeout_secs as i64;
        
        let healthy_shards: Vec<&ShardInfo> = shards.values()
            .filter(|s| {
                let age = now.signed_duration_since(s.last_heartbeat).num_seconds();
                age < timeout_secs && s.available_capacity() > 0
            })
            .collect();
            
        if healthy_shards.is_empty() {
            return None;
        }
        
        // Return shard with most capacity
        healthy_shards.into_iter()
            .max_by_key(|s| s.available_capacity())
            .map(|s| s.clone())
    }

    pub async fn check_health(&self) {
        let mut shards = self.shards.write().await;
        let now = Utc::now();
        let timeout_secs = self.config.shard_timeout_secs as i64;
        
        for (id, shard) in shards.iter_mut() {
            let age = now.signed_duration_since(shard.last_heartbeat).num_seconds();
            if age >= timeout_secs && shard.is_healthy {
                warn!("Shard {} is unhealthy (last heartbeat {}s ago)", id, age);
                shard.is_healthy = false;
            } else if age < timeout_secs && !shard.is_healthy {
                info!("Shard {} recovered", id);
                shard.is_healthy = true;
            }
        }
    }
    
    pub async fn get_shards_snapshot(&self) -> Vec<ShardInfo> {
        self.shards.read().await.values().cloned().collect()
    }
}
