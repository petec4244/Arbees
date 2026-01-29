//! ZMQ price listener for crypto prices
//!
//! Subscribes to multiple ZMQ endpoints for crypto price updates.
//! Maintains an in-memory price cache with stale checking.
//! Measures end-to-end latency from monitor publication to processing.

use crate::price::data::{CryptoPriceData, IncomingCryptoPrice};
use crate::types::ZmqEnvelope;
use anyhow::Result;
use chrono::Utc;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zeromq::{Socket, SocketRecv, SubSocket};

/// Listens to ZMQ price feeds and maintains in-memory cache
#[derive(Clone)]
pub struct CryptoPriceListener {
    /// ZMQ endpoints to subscribe to
    pub zmq_sub_endpoints: Vec<String>,

    /// In-memory price cache: asset -> platform -> price
    pub prices: Arc<RwLock<HashMap<String, CryptoPriceData>>>,

    /// Statistics: prices received
    pub prices_received: Arc<AtomicU64>,

    /// Configuration
    pub config: ListenerConfig,
}

#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Max age before price is considered stale
    pub price_staleness: Duration,

    /// Log latency every N prices
    pub latency_log_interval: u64,

    /// Timeout for ZMQ receive operations
    pub zmq_receive_timeout: Duration,

    /// Reconnection delay on error
    pub reconnect_delay: Duration,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            price_staleness: Duration::from_secs(60),
            latency_log_interval: 100,
            zmq_receive_timeout: Duration::from_secs(30),
            reconnect_delay: Duration::from_secs(5),
        }
    }
}

impl CryptoPriceListener {
    pub fn new(
        zmq_sub_endpoints: Vec<String>,
        prices: Arc<RwLock<HashMap<String, CryptoPriceData>>>,
        prices_received: Arc<AtomicU64>,
    ) -> Self {
        Self {
            zmq_sub_endpoints,
            prices,
            prices_received,
            config: ListenerConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ListenerConfig) -> Self {
        self.config = config;
        self
    }

    /// Start listening for prices on configured ZMQ endpoints
    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting crypto ZMQ price listener on endpoints: {:?}",
            self.zmq_sub_endpoints
        );

        let mut socket = SubSocket::new();

        // Connect to all endpoints
        for endpoint in &self.zmq_sub_endpoints {
            match socket.connect(endpoint).await {
                Ok(_) => info!("Price listener connected to {}", endpoint),
                Err(e) => {
                    warn!("Failed to connect to {}: {}", endpoint, e);
                }
            }
        }

        // Subscribe to price topics from all sources
        socket.subscribe("prices.kalshi").await?;  // Kalshi prediction market prices
        socket.subscribe("prices.poly").await?;    // Polymarket prediction market prices
        socket.subscribe("crypto.prices").await?;  // Crypto spot prices
        info!("Subscribed to prices.kalshi.*, prices.poly.*, crypto.prices.*");

        let mut last_stats_log = Instant::now();
        let stats_log_interval = Duration::from_secs(60);

        loop {
            match tokio::time::timeout(self.config.zmq_receive_timeout, socket.recv()).await {
                Ok(Ok(msg)) => {
                    self.handle_price_message(msg).await;
                }
                Ok(Err(e)) => {
                    warn!("ZMQ receive error: {}. Reconnecting...", e);
                    // Attempt reconnection
                    for endpoint in &self.zmq_sub_endpoints {
                        let _ = socket.connect(endpoint).await;
                    }
                    tokio::time::sleep(self.config.reconnect_delay).await;
                }
                Err(_) => {
                    // Timeout is normal - just log stats periodically
                    if last_stats_log.elapsed() >= stats_log_interval {
                        let count = self.prices_received.load(Ordering::Relaxed);
                        info!("Crypto prices received (total): {}", count);
                        last_stats_log = Instant::now();
                    }
                }
            }
        }
    }

    /// Handle incoming price message
    async fn handle_price_message(&self, msg: zeromq::ZmqMessage) {
        let parts: Vec<_> = msg.iter().collect();

        if parts.len() < 2 {
            warn!("Invalid ZMQ message: expected at least 2 parts");
            return;
        }

        let _topic = String::from_utf8_lossy(parts[0].as_ref());
        let payload_bytes = parts[1].as_ref();

        // Parse envelope
        let envelope: ZmqEnvelope<IncomingCryptoPrice> = match serde_json::from_slice(payload_bytes)
        {
            Ok(e) => e,
            Err(e) => {
                debug!("Failed to parse incoming crypto price: {}", e);
                return;
            }
        };

        let incoming = envelope.payload;

        // Calculate latency
        let now_ms = Utc::now().timestamp_millis();
        let latency_ms = now_ms - envelope.timestamp_ms;

        // Store price
        let price_data: CryptoPriceData = incoming.into();
        let cache_key = format!("{}|{}", price_data.asset, price_data.platform);

        {
            let mut prices = self.prices.write().await;
            prices.insert(cache_key.clone(), price_data.clone());
        }

        // Update statistics
        let count = self.prices_received.fetch_add(1, Ordering::Relaxed);

        // Log latency periodically
        if count % self.config.latency_log_interval == 0 {
            debug!(
                "Crypto price latency: {}ms (seq={}, asset={}, platform={})",
                latency_ms, envelope.seq, price_data.asset, price_data.platform
            );
        }

        // Warn if latency is excessive
        if latency_ms > 1000 {
            warn!(
                "High crypto price latency: {}ms for {} on {}",
                latency_ms, price_data.asset, price_data.platform
            );
        }
    }

    /// Get current price for an asset on a specific platform
    pub async fn get_price(&self, asset: &str, platform: &str) -> Option<CryptoPriceData> {
        let prices = self.prices.read().await;
        let cache_key = format!("{}|{}", asset, platform);
        prices.get(&cache_key).cloned()
    }

    /// Get all prices for an asset across platforms
    pub async fn get_asset_prices(&self, asset: &str) -> Vec<CryptoPriceData> {
        let prices = self.prices.read().await;
        prices
            .values()
            .filter(|p| p.asset == asset)
            .cloned()
            .collect()
    }

    /// Check if we have recent prices for an asset
    pub async fn has_recent_price(&self, asset: &str, max_age: Duration) -> bool {
        self.get_asset_prices(asset)
            .await
            .iter()
            .any(|p| !p.is_stale(max_age))
    }

    /// Get statistics about received prices
    pub fn stats(&self) -> ListenerStats {
        ListenerStats {
            total_prices_received: self.prices_received.load(Ordering::Relaxed),
            cached_prices: {
                // Note: This would need to be awaited in real usage
                // For now, return a placeholder
                0
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenerStats {
    pub total_prices_received: u64,
    pub cached_prices: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_listener_creation() {
        let prices = Arc::new(RwLock::new(HashMap::new()));
        let received = Arc::new(AtomicU64::new(0));

        let listener = CryptoPriceListener::new(
            vec!["tcp://localhost:5560".to_string()],
            prices.clone(),
            received.clone(),
        );

        assert_eq!(listener.zmq_sub_endpoints.len(), 1);
        assert_eq!(listener.prices_received.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_get_price_empty_cache() {
        let prices = Arc::new(RwLock::new(HashMap::new()));
        let received = Arc::new(AtomicU64::new(0));

        let listener = CryptoPriceListener::new(vec![], prices, received);

        let price = listener.get_price("BTC", "kalshi").await;
        assert!(price.is_none());
    }

    #[tokio::test]
    async fn test_get_asset_prices() {
        let mut price_map = HashMap::new();

        let price1 = CryptoPriceData {
            market_id: "btc_1".to_string(),
            platform: "kalshi".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.45,
            yes_ask: 0.47,
            mid_price: 0.46,
            yes_bid_size: None,
            yes_ask_size: None,
            total_liquidity: None,
            timestamp: Utc::now(),
        };

        let price2 = CryptoPriceData {
            market_id: "btc_2".to_string(),
            platform: "polymarket".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.46,
            yes_ask: 0.48,
            mid_price: 0.47,
            yes_bid_size: None,
            yes_ask_size: None,
            total_liquidity: None,
            timestamp: Utc::now(),
        };

        price_map.insert("BTC|kalshi".to_string(), price1.clone());
        price_map.insert("BTC|polymarket".to_string(), price2.clone());

        let prices = Arc::new(RwLock::new(price_map));
        let received = Arc::new(AtomicU64::new(0));

        let listener = CryptoPriceListener::new(vec![], prices, received);

        let btc_prices = listener.get_asset_prices("BTC").await;
        assert_eq!(btc_prices.len(), 2);
    }

    #[tokio::test]
    async fn test_has_recent_price() {
        let mut price_map = HashMap::new();

        let price = CryptoPriceData {
            market_id: "btc".to_string(),
            platform: "kalshi".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.45,
            yes_ask: 0.47,
            mid_price: 0.46,
            yes_bid_size: None,
            yes_ask_size: None,
            total_liquidity: None,
            timestamp: Utc::now(),
        };

        price_map.insert("BTC|kalshi".to_string(), price);

        let prices = Arc::new(RwLock::new(price_map));
        let received = Arc::new(AtomicU64::new(0));

        let listener = CryptoPriceListener::new(vec![], prices, received);

        assert!(listener.has_recent_price("BTC", Duration::from_secs(60)).await);
        assert!(!listener.has_recent_price("BTC", Duration::from_secs(0)).await);
        assert!(!listener.has_recent_price("ETH", Duration::from_secs(60)).await);
    }
}
