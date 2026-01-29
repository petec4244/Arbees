//! Multi-Market Manager
//!
//! Handles event discovery and routing for non-sports markets:
//! - Crypto (price targets, protocol events)
//! - Economics (indicator thresholds, Fed decisions)
//! - Politics (elections, confirmations, policy votes)

use crate::config::Config;
use crate::managers::service_registry::ServiceRegistry;
use crate::managers::shard_manager::ShardManager;
use arbees_rust_core::clients::coingecko::CoinGeckoClient;
use arbees_rust_core::models::MarketType;
use arbees_rust_core::providers::crypto::CryptoEventProvider;
use arbees_rust_core::providers::economics::EconomicsEventProvider;
use arbees_rust_core::providers::politics::PoliticsEventProvider;
use arbees_rust_core::providers::{EventInfo, EventProvider, EventStatus};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Event assignment for non-sports markets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventAssignment {
    pub event_id: String,
    pub market_type: MarketType,
    pub entity_a: String,
    pub entity_b: Option<String>,
    pub shard_id: String,
    pub kalshi_market_id: Option<String>,
    pub polymarket_market_id: Option<String>,
    pub assigned_at: DateTime<Utc>,
}

/// Multi-market manager for Crypto, Economics, and Politics
pub struct MultiMarketManager {
    redis: redis::Client,
    config: Config,
    shard_manager: Arc<ShardManager>,
    service_registry: Arc<ServiceRegistry>,
    /// Active event assignments
    assignments: Arc<RwLock<HashMap<String, EventAssignment>>>,
    /// Crypto event provider
    crypto_provider: Option<CryptoEventProvider>,
    /// Economics event provider
    economics_provider: Option<EconomicsEventProvider>,
    /// Politics event provider
    politics_provider: Option<PoliticsEventProvider>,
}

impl MultiMarketManager {
    /// Create a new multi-market manager
    pub fn new(
        redis_client: redis::Client,
        shard_manager: Arc<ShardManager>,
        service_registry: Arc<ServiceRegistry>,
        config: Config,
    ) -> Self {
        // Initialize providers based on configuration
        let crypto_provider = if config.enable_crypto_markets {
            let coingecko = Arc::new(CoinGeckoClient::new());
            Some(CryptoEventProvider::with_coingecko(coingecko))
        } else {
            None
        };

        let economics_provider = if config.enable_economics_markets {
            Some(EconomicsEventProvider::new())
        } else {
            None
        };

        let politics_provider = if config.enable_politics_markets {
            Some(PoliticsEventProvider::new())
        } else {
            None
        };

        Self {
            redis: redis_client,
            config,
            shard_manager,
            service_registry,
            assignments: Arc::new(RwLock::new(HashMap::new())),
            crypto_provider,
            economics_provider,
            politics_provider,
        }
    }

    /// Get reference to assignments for fault tolerance
    pub fn get_assignments(&self) -> Arc<RwLock<HashMap<String, EventAssignment>>> {
        self.assignments.clone()
    }

    /// Run discovery cycle for all enabled market types
    pub async fn run_discovery_cycle(&self) {
        info!("Starting multi-market discovery cycle");

        let mut all_events = Vec::new();

        // Discover crypto events
        if let Some(provider) = &self.crypto_provider {
            match provider.get_live_events().await {
                Ok(events) => {
                    info!("Found {} live crypto events", events.len());
                    all_events.extend(events);
                }
                Err(e) => error!("Error discovering crypto events: {}", e),
            }
        }

        // Discover economics events
        if let Some(provider) = &self.economics_provider {
            match provider.get_live_events().await {
                Ok(events) => {
                    info!("Found {} live economics events", events.len());
                    all_events.extend(events);
                }
                Err(e) => error!("Error discovering economics events: {}", e),
            }
        }

        // Discover politics events
        if let Some(provider) = &self.politics_provider {
            match provider.get_live_events().await {
                Ok(events) => {
                    info!("Found {} live politics events", events.len());
                    all_events.extend(events);
                }
                Err(e) => error!("Error discovering politics events: {}", e),
            }
        }

        // Filter for new events not yet assigned
        let assignments = self.assignments.read().await;
        let new_events: Vec<EventInfo> = all_events
            .into_iter()
            .filter(|e| !assignments.contains_key(&e.event_id))
            .filter(|e| e.status == EventStatus::Live)
            .collect();
        drop(assignments);

        info!("Processing {} new multi-market events", new_events.len());

        for event in new_events {
            self.process_new_event(event).await;
        }
    }

    /// Process a newly discovered event
    async fn process_new_event(&self, event: EventInfo) {
        debug!(
            "Processing new event: {} ({:?})",
            event.event_id, event.market_type
        );

        // Extract market IDs from metadata
        let kalshi_id = event
            .metadata
            .get("kalshi_ticker")
            .and_then(|v| v.as_str())
            .map(String::from);

        let polymarket_id = event
            .metadata
            .get("polymarket_condition_id")
            .or(event.metadata.get("condition_id"))
            .and_then(|v| v.as_str())
            .map(String::from);

        // Get best shard
        let shard = match self.shard_manager.get_best_shard().await {
            Some(s) => s,
            None => {
                warn!(
                    "No healthy shards available for event {}",
                    event.event_id
                );
                return;
            }
        };

        // Create assignment
        let assignment = EventAssignment {
            event_id: event.event_id.clone(),
            market_type: event.market_type.clone(),
            entity_a: event.entity_a.clone(),
            entity_b: event.entity_b.clone(),
            shard_id: shard.shard_id.clone(),
            kalshi_market_id: kalshi_id.clone(),
            polymarket_market_id: polymarket_id.clone(),
            assigned_at: Utc::now(),
        };

        // Construct market IDs by type
        let mut market_ids_by_type = HashMap::new();
        let mut moneyline = HashMap::new();
        if let Some(pid) = &polymarket_id {
            moneyline.insert("polymarket".to_string(), pid.clone());
        }
        if let Some(kid) = &kalshi_id {
            moneyline.insert("kalshi".to_string(), kid.clone());
        }
        if !moneyline.is_empty() {
            market_ids_by_type.insert("moneyline".to_string(), moneyline);
        }

        // Send command to shard
        let command = serde_json::json!({
            "type": "add_event",
            "event_id": event.event_id,
            "market_type": event.market_type,
            "entity_a": event.entity_a,
            "entity_b": event.entity_b,
            "kalshi_market_id": kalshi_id,
            "polymarket_market_id": polymarket_id,
            "market_ids_by_type": market_ids_by_type,
            "metadata": event.metadata,
        });

        if let Ok(mut conn) = self.redis.get_async_connection().await {
            let channel = format!("shard:{}:command", shard.shard_id);
            let _ = conn.publish::<_, _, ()>(channel, command.to_string()).await;

            // Publish market assignments for monitors
            if let Some(pid) = &polymarket_id {
                let _ = conn
                    .publish::<_, _, ()>(
                        "orchestrator:market_assignments",
                        serde_json::json!({
                            "type": "polymarket_assign",
                            "event_id": event.event_id,
                            "market_type": event.market_type.type_name(),
                            "markets": vec![serde_json::json!({
                                "market_type": "outcome",
                                "condition_id": pid
                            })]
                        })
                        .to_string(),
                    )
                    .await;
            }

            if let Some(kid) = &kalshi_id {
                let _ = conn
                    .publish::<_, _, ()>(
                        "orchestrator:market_assignments",
                        serde_json::json!({
                            "type": "kalshi_assign",
                            "event_id": event.event_id,
                            "market_type": event.market_type.type_name(),
                            "markets": vec![serde_json::json!({
                                "market_type": "outcome",
                                "ticker": kid
                            })]
                        })
                        .to_string(),
                    )
                    .await;
            }

            info!(
                "Assigned {} event {} to shard {}",
                event.market_type.type_name(),
                event.event_id,
                shard.shard_id
            );

            // Track assignment in service registry to prevent zombie detection
            self.service_registry
                .track_assignment(&shard.shard_id, &event.event_id)
                .await;

            let mut assign_lock = self.assignments.write().await;
            assign_lock.insert(event.event_id.clone(), assignment);
        } else {
            error!("Failed to connect to Redis to assign event");
        }
    }

    /// Handle shard heartbeat to detect missing events
    pub async fn handle_shard_heartbeat(&self, payload: serde_json::Value) {
        let shard_id = match payload.get("shard_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };

        // Extract reported events (may be reported as "games" or "events")
        let reported_events: std::collections::HashSet<String> = payload
            .get("events")
            .or(payload.get("games"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut assignments = self.assignments.write().await;
        let mut events_to_remove = Vec::new();

        for (event_id, assignment) in assignments.iter() {
            if assignment.shard_id == shard_id && !reported_events.contains(event_id) {
                warn!(
                    "Event {} missing from shard {} report, clearing assignment",
                    event_id, shard_id
                );
                events_to_remove.push(event_id.clone());
            }
        }

        for event_id in events_to_remove {
            assignments.remove(&event_id);
        }
    }

    /// Get statistics about current assignments
    pub async fn get_stats(&self) -> MultiMarketStats {
        let assignments = self.assignments.read().await;

        let mut stats = MultiMarketStats::default();

        for assignment in assignments.values() {
            match &assignment.market_type {
                MarketType::Crypto { .. } => stats.crypto_count += 1,
                MarketType::Economics { .. } => stats.economics_count += 1,
                MarketType::Politics { .. } => stats.politics_count += 1,
                _ => stats.other_count += 1,
            }
        }

        stats.total_count = assignments.len();
        stats
    }
}

/// Statistics about multi-market assignments
#[derive(Debug, Clone, Default, Serialize)]
pub struct MultiMarketStats {
    pub total_count: usize,
    pub crypto_count: usize,
    pub economics_count: usize,
    pub politics_count: usize,
    pub other_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_assignment_serialization() {
        let assignment = EventAssignment {
            event_id: "test-123".to_string(),
            market_type: MarketType::Crypto {
                asset: "BTC".to_string(),
                prediction_type: arbees_rust_core::models::CryptoPredictionType::PriceTarget,
            },
            entity_a: "bitcoin".to_string(),
            entity_b: None,
            shard_id: "shard-1".to_string(),
            kalshi_market_id: Some("KBTC-100K".to_string()),
            polymarket_market_id: None,
            assigned_at: Utc::now(),
        };

        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("crypto"));
    }
}
