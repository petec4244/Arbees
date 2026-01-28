//! Universal event monitor for all market types
//!
//! This module provides monitoring capabilities for non-sports events
//! (crypto, economics, politics). It follows a similar pattern to monitor_game
//! but uses the universal EventProvider abstraction.

use crate::shard::MarketPriceData;
use arbees_rust_core::models::{
    channels, MarketType, Platform, SignalDirection, SignalType, Sport, TradingSignal,
    TransportMode,
};
use arbees_rust_core::probability::ProbabilityModelRegistry;
use arbees_rust_core::providers::{EventProviderRegistry, EventState, EventStatus};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use log::{debug, error, info, warn};
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use zeromq::{PubSocket, ZmqMessage, SocketSend};

/// Configuration for event monitoring
#[derive(Debug, Clone)]
pub struct EventMonitorConfig {
    /// Polling interval for event state updates
    pub poll_interval: Duration,
    /// Minimum edge percentage to generate a signal
    pub min_edge_pct: f64,
    /// Debounce time between signals (in seconds)
    pub signal_debounce_secs: u64,
    /// Price staleness threshold (in seconds)
    pub price_staleness_secs: i64,
}

impl Default for EventMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            min_edge_pct: 15.0,
            signal_debounce_secs: 30,
            price_staleness_secs: 30,
        }
    }
}

impl EventMonitorConfig {
    /// Create config from environment variables with fallback to defaults
    pub fn from_env() -> Self {
        Self {
            poll_interval: Duration::from_secs(
                env::var("EVENT_POLL_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5),
            ),
            min_edge_pct: env::var("MIN_EDGE_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15.0),
            signal_debounce_secs: env::var("SIGNAL_DEBOUNCE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            price_staleness_secs: env::var("PRICE_STALENESS_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        }
    }
}

/// Monitor a universal event (crypto, economics, politics)
///
/// This function runs in a loop, fetching event state from the appropriate
/// provider and emitting trading signals when edges are detected.
pub async fn monitor_event(
    redis: RedisBus,
    db_pool: PgPool,
    event_id: String,
    market_type: MarketType,
    entity_a: String,
    entity_b: Option<String>,
    config: EventMonitorConfig,
    provider_registry: Arc<EventProviderRegistry>,
    probability_registry: Arc<ProbabilityModelRegistry>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,
    transport_mode: TransportMode,
) {
    info!(
        "Starting monitor_event: {} ({:?}) entity_a={}, entity_b={:?}",
        event_id,
        market_type.type_name(),
        entity_a,
        entity_b
    );

    // Get the provider for this market type
    let provider = match provider_registry.get_provider(&market_type) {
        Some(p) => p,
        None => {
            error!("No provider for market type: {:?}", market_type);
            return;
        }
    };

    // Signal debouncing: (entity, direction) -> last_signal_time
    let mut last_signal_times: HashMap<(String, String), Instant> = HashMap::new();

    // Track last probability for change detection
    let mut last_probability: Option<f64> = None;

    loop {
        // Fetch event state from provider
        match provider.get_event_state(&event_id).await {
            Ok(state) => {
                // Check if event is completed
                if state.status == EventStatus::Completed {
                    info!("Event {} completed, stopping monitor", event_id);
                    return;
                }

                // Skip if event not yet live
                if state.status == EventStatus::Scheduled {
                    debug!("Event {} not yet live, waiting...", event_id);
                    tokio::time::sleep(config.poll_interval).await;
                    continue;
                }

                // Calculate probability using the appropriate model
                let probability = match probability_registry
                    .calculate_probability(&state, true)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to calculate probability for {}: {}", event_id, e);
                        tokio::time::sleep(config.poll_interval).await;
                        continue;
                    }
                };

                // Publish event state to Redis
                publish_event_state(&redis, &event_id, &state, probability).await;

                // Check for trading signals
                let prices = market_prices.read().await;
                if let Some(event_prices) = prices.get(&event_id) {
                    // Find prices for entity_a (primary entity)
                    if let Some(price) = find_entity_price(event_prices, &entity_a, config.price_staleness_secs) {
                        // Calculate edge
                        let market_mid = price.mid_price;
                        let edge_pct = (probability - market_mid).abs() * 100.0;

                        if edge_pct >= config.min_edge_pct {
                            let direction = if probability > market_mid {
                                SignalDirection::Buy
                            } else {
                                SignalDirection::Sell
                            };

                            let signal_key = (entity_a.clone(), format!("{:?}", direction).to_lowercase());

                            // Check debounce
                            let should_emit = match last_signal_times.get(&signal_key) {
                                Some(last_time) => {
                                    last_time.elapsed().as_secs() >= config.signal_debounce_secs
                                }
                                None => true,
                            };

                            if should_emit {
                                info!(
                                    "EDGE: {} {} model={:.1}% market={:.1}% edge={:.1}%",
                                    event_id, entity_a, probability * 100.0, market_mid * 100.0, edge_pct
                                );

                                if emit_event_signal(
                                    &redis,
                                    &event_id,
                                    &market_type,
                                    &entity_a,
                                    direction,
                                    &price,
                                    probability,
                                    edge_pct,
                                    &zmq_pub,
                                    &zmq_seq,
                                    transport_mode,
                                )
                                .await
                                {
                                    last_signal_times.insert(signal_key, Instant::now());
                                }
                            }
                        }
                    }
                }

                // Update last probability
                last_probability = Some(probability);
            }
            Err(e) => {
                warn!("Failed to fetch event state for {}: {}", event_id, e);
            }
        }

        tokio::time::sleep(config.poll_interval).await;
    }
}

/// Publish event state to Redis for other services
async fn publish_event_state(
    redis: &RedisBus,
    event_id: &str,
    state: &EventState,
    probability: f64,
) {
    let state_channel = format!("event:{}:state", event_id);
    let state_json = json!({
        "event_id": state.event_id,
        "market_type": state.market_type.type_name(),
        "entity_a": state.entity_a,
        "entity_b": state.entity_b,
        "status": format!("{:?}", state.status),
        "probability": probability,
        "fetched_at": state.fetched_at.to_rfc3339(),
        "timestamp": Utc::now().to_rfc3339(),
    });

    if let Err(e) = redis.publish(&state_channel, &state_json).await {
        warn!("Event state publish error: {}", e);
    }
}

/// Find price for a specific entity
fn find_entity_price<'a>(
    prices: &'a HashMap<String, MarketPriceData>,
    entity: &str,
    staleness_secs: i64,
) -> Option<&'a MarketPriceData> {
    let now = Utc::now();

    // Try exact match first
    if let Some(price) = prices.get(entity) {
        let age = (now - price.timestamp).num_seconds();
        if age <= staleness_secs {
            return Some(price);
        }
    }

    // Try lowercase match
    let entity_lower = entity.to_lowercase();
    for (key, price) in prices {
        if key.to_lowercase().contains(&entity_lower) {
            let age = (now - price.timestamp).num_seconds();
            if age <= staleness_secs {
                return Some(price);
            }
        }
    }

    None
}

/// Emit a trading signal for a non-sports event
async fn emit_event_signal(
    redis: &RedisBus,
    event_id: &str,
    market_type: &MarketType,
    entity: &str,
    direction: SignalDirection,
    price: &MarketPriceData,
    model_prob: f64,
    edge_pct: f64,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
    transport_mode: TransportMode,
) -> bool {
    let signal_type = match direction {
        SignalDirection::Buy => SignalType::ModelEdgeYes,
        SignalDirection::Sell => SignalType::ModelEdgeNo,
        SignalDirection::Hold => return false,
    };

    let signal = TradingSignal {
        signal_id: format!("sig-{}", Uuid::new_v4()),
        signal_type,
        game_id: event_id.to_string(),
        sport: Sport::NBA, // Placeholder for non-sports (backward compat)
        team: entity.to_string(),
        direction,
        model_prob,
        market_prob: Some(price.mid_price),
        edge_pct,
        confidence: (edge_pct / 20.0).min(1.0), // Scale edge to confidence
        platform_buy: Some(Platform::Polymarket), // TODO: Use actual platform
        platform_sell: None,
        buy_price: Some(price.yes_ask),
        sell_price: None,
        liquidity_available: price.total_liquidity.unwrap_or(1000.0),
        reason: format!(
            "{:?} {} model={:.1}% market={:.1}%",
            market_type.type_name(),
            entity,
            model_prob * 100.0,
            price.mid_price * 100.0
        ),
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::minutes(5)),
        play_id: None,
        // Universal fields (new)
        event_id: Some(event_id.to_string()),
        market_type: Some(market_type.clone()),
        entity: Some(entity.to_string()),
    };

    let signal_json = match serde_json::to_value(&signal) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to serialize signal: {}", e);
            return false;
        }
    };

    // Publish via ZMQ if enabled
    if transport_mode.use_zmq() {
        if let Some(zmq) = zmq_pub {
            let seq = zmq_seq.fetch_add(1, Ordering::SeqCst);
            let envelope = json!({
                "seq": seq,
                "timestamp_ms": Utc::now().timestamp_millis(),
                "source": "game_shard",
                "payload": signal_json,
            });

            if let Ok(envelope_bytes) = serde_json::to_vec(&envelope) {
                let zmq_msg = ZmqMessage::from(envelope_bytes);
                if let Ok(mut socket) = zmq.try_lock() {
                    if let Err(e) = socket.send(zmq_msg).await {
                        warn!("ZMQ signal send error: {}", e);
                    } else {
                        debug!("Signal sent via ZMQ: seq={}", seq);
                    }
                }
            }
        }
    }

    // Publish via Redis if enabled
    if transport_mode.use_redis() {
        if let Err(e) = redis.publish(channels::SIGNALS_NEW, &signal_json).await {
            warn!("Redis signal publish error: {}", e);
            return false;
        }
    }

    info!(
        "SIGNAL: {} {} {:?} edge={:.1}%",
        event_id, entity, direction, edge_pct
    );

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = EventMonitorConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(5));
        assert_eq!(config.min_edge_pct, 15.0);
        assert_eq!(config.signal_debounce_secs, 30);
    }

    #[test]
    fn test_find_entity_price() {
        let mut prices = HashMap::new();
        prices.insert(
            "bitcoin".to_string(),
            MarketPriceData {
                market_id: "btc-market".to_string(),
                platform: "polymarket".to_string(),
                contract_team: "bitcoin".to_string(),
                yes_bid: 0.55,
                yes_ask: 0.57,
                mid_price: 0.56,
                timestamp: Utc::now(),
                yes_bid_size: Some(1000.0),
                yes_ask_size: Some(1000.0),
                total_liquidity: Some(5000.0),
            },
        );

        // Exact match
        let found = find_entity_price(&prices, "bitcoin", 30);
        assert!(found.is_some());
        assert_eq!(found.unwrap().market_id, "btc-market");

        // Case-insensitive match
        let found = find_entity_price(&prices, "Bitcoin", 30);
        assert!(found.is_some());

        // No match
        let found = find_entity_price(&prices, "ethereum", 30);
        assert!(found.is_none());
    }
}
