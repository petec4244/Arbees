//! Unified ZMQ price listener for low-latency price ingestion
//!
//! Connects to multiple ZMQ endpoints (e.g., Kalshi and Polymarket monitors)
//! and processes incoming price messages. Replaces the old Redis price listener.

//! Unified ZMQ price listener for low-latency price ingestion
//!
//! Connects to multiple ZMQ endpoints (e.g., Kalshi and Polymarket monitors)
//! and processes incoming price messages. Replaces the old Redis price listener.

use crate::price::data::{IncomingMarketPrice, MarketPriceData};
use crate::types::{ZmqEnvelope, PriceListenerStats};
use anyhow::Result;
use chrono::Utc;
use log::{debug, info, warn};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zeromq::{Socket, SocketRecv, SubSocket};
use std::collections::HashMap;

/// Unified ZMQ price listener for multiple endpoints
pub struct PriceListener {
    /// ZMQ endpoints to connect to (e.g., ["tcp://kalshi:5555", "tcp://polymarket:5556"])
    pub zmq_sub_endpoints: Vec<String>,

    /// Shared market prices map - outer key is game_id, inner key is "{team}|{platform}"
    /// Stores MarketPriceData (processed format used by game monitoring)
    pub market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,

    /// Statistics tracking
    pub stats: Arc<PriceListenerStats>,
}

impl PriceListener {
    /// Create a new price listener
    pub fn new(
        zmq_sub_endpoints: Vec<String>,
        market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
        stats: Arc<PriceListenerStats>,
    ) -> Self {
        Self {
            zmq_sub_endpoints,
            market_prices,
            stats,
        }
    }

    /// Start the unified ZMQ price listener loop
    pub async fn start(&self) -> Result<()> {
        info!("Starting unified ZMQ price listener...");

        // Create ZMQ SUB socket
        let mut socket = SubSocket::new();

        // Connect to all endpoints
        for endpoint in &self.zmq_sub_endpoints {
            match socket.connect(endpoint).await {
                Ok(_) => info!("ZMQ price listener connected to {}", endpoint),
                Err(e) => {
                    warn!("Failed to connect to ZMQ endpoint {}: {}", endpoint, e);
                }
            }
        }

        // Subscribe to price topics
        socket.subscribe("prices.").await?;
        info!("ZMQ price listener subscribed to prices.*");

        // Track last stats log time
        let mut last_stats_log = Instant::now();
        let stats_log_interval = Duration::from_secs(60);
        let mut zmq_messages_received: u64 = 0;

        loop {
            // Receive with timeout
            let recv_result = tokio::time::timeout(
                Duration::from_secs(30),
                socket.recv(),
            ).await;

            match recv_result {
                Ok(Ok(msg)) => {
                    zmq_messages_received += 1;
                    self.stats.messages_received.fetch_add(1, Ordering::Relaxed);

                    // ZMQ multipart: [topic, payload]
                    let parts: Vec<_> = msg.iter().collect();
                    if parts.len() < 2 {
                        continue;
                    }

                    let topic = String::from_utf8_lossy(parts[0].as_ref());
                    let payload_bytes = parts[1].as_ref();

                    // Parse envelope
                    let envelope: ZmqEnvelope = match serde_json::from_slice(payload_bytes) {
                        Ok(e) => e,
                        Err(e) => {
                            self.stats.parse_failures.fetch_add(1, Ordering::Relaxed);
                            debug!("Failed to parse ZMQ price message: {}", e);
                            continue;
                        }
                    };

                    // Extract price data from payload
                    let price: IncomingMarketPrice = match serde_json::from_value(envelope.payload) {
                        Ok(p) => p,
                        Err(e) => {
                            self.stats.parse_failures.fetch_add(1, Ordering::Relaxed);
                            debug!("Failed to parse ZMQ price payload: {}", e);
                            continue;
                        }
                    };

                    // Process the price
                    self.process_incoming_price(price).await;

                    // Log ZMQ latency periodically
                    if zmq_messages_received % 1000 == 0 {
                        let now_ms = Utc::now().timestamp_millis();
                        let latency_ms = now_ms - envelope.timestamp_ms;
                        debug!("ZMQ price #{}: latency={}ms topic={}", zmq_messages_received, latency_ms, topic);
                    }
                }
                Ok(Err(e)) => {
                    warn!("ZMQ receive error: {}. Reconnecting...", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    // Reconnect
                    for endpoint in &self.zmq_sub_endpoints {
                        let _ = socket.connect(endpoint).await;
                    }
                }
                Err(_) => {
                    // Timeout - normal for low-traffic periods
                    debug!("No ZMQ price message in 30s");
                }
            }

            // Periodic stats logging
            if last_stats_log.elapsed() >= stats_log_interval {
                let stats = self.stats.snapshot();
                info!(
                    "ZMQ price listener: received={} messages, processed={}, parse_failures={}",
                    stats.messages_received, stats.messages_processed, stats.parse_failures
                );
                last_stats_log = Instant::now();
            }
        }
    }

    /// Process an incoming price message
    async fn process_incoming_price(&self, price: IncomingMarketPrice) {
        let game_id = price.game_id.clone();

        // Check if contract_team is present
        let team = match &price.contract_team {
            Some(t) => t,
            None => {
                self.stats.no_team_skipped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        // Check if we have liquidity data
        if price.yes_ask_size.is_none()
            && price.yes_bid_size.is_none()
            && price.liquidity.is_none()
        {
            self.stats.no_liquidity_skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.stats.messages_processed.fetch_add(1, Ordering::Relaxed);

        // Convert IncomingMarketPrice to MarketPriceData for storage
        let market_price = MarketPriceData {
            market_id: price.market_id.clone(),
            platform: price.platform.clone(),
            contract_team: team.clone(),
            yes_bid: price.yes_bid,
            yes_ask: price.yes_ask,
            mid_price: price.mid_price.unwrap_or((price.yes_bid + price.yes_ask) / 2.0),
            yes_bid_size: price.yes_bid_size,
            yes_ask_size: price.yes_ask_size,
            total_liquidity: price.liquidity,
            timestamp: Utc::now(),
        };

        // Store price in map: outer key is game_id, inner key is "{team}|{platform}"
        let key = format!("{}|{}", team, price.platform);
        let mut prices = self.market_prices.write().await;
        prices.entry(game_id).or_insert_with(HashMap::new).insert(key, market_price);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[tokio::test]
    async fn test_price_listener_creation() {
        let endpoints = vec!["tcp://localhost:5555".to_string()];
        let market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let stats = Arc::new(PriceListenerStats {
            messages_received: AtomicU64::new(0),
            messages_processed: AtomicU64::new(0),
            parse_failures: AtomicU64::new(0),
            no_liquidity_skipped: AtomicU64::new(0),
            no_team_skipped: AtomicU64::new(0),
        });

        let listener = PriceListener::new(endpoints, market_prices, stats);
        assert_eq!(listener.zmq_sub_endpoints.len(), 1);
    }
}
