//! ZMQ price listener for crypto prices
//!
//! Subscribes to multiple ZMQ endpoints for crypto price updates.
//! Maintains an in-memory price cache with stale checking.
//! Measures end-to-end latency from monitor publication to processing.

use crate::price::data::{CryptoPriceData, IncomingCryptoPrice};
use crate::types::ZmqEnvelope;
use anyhow::Result;
use chrono::Utc;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
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

    /// Rate limiter for high latency warnings (log every 1000)
    high_latency_warnings: Arc<AtomicU64>,

    /// Channel for notifying when prices are updated (for event-driven evaluation)
    pub price_update_tx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<PriceUpdate>>>>,

    /// Configuration
    pub config: ListenerConfig,
}

/// Signal sent when prices are updated
#[derive(Debug, Clone)]
pub struct PriceUpdate {
    pub asset: String,
    pub platform: String,
    pub count: u64,
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
            high_latency_warnings: Arc::new(AtomicU64::new(0)),
            price_update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            config: ListenerConfig::default(),
        }
    }

    /// Set the price update notification channel
    pub async fn set_price_update_notifier(&self, tx: mpsc::UnboundedSender<PriceUpdate>) {
        let mut channel = self.price_update_tx.lock().await;
        *channel = Some(tx);
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

        // Create socket
        let mut socket = SubSocket::new();

        // Connect to all endpoints with error handling
        // Note: ZMQ will retry connection asynchronously, so this is best-effort
        let mut successful_connections = 0;
        let total_endpoints = self.zmq_sub_endpoints.len();

        for endpoint in &self.zmq_sub_endpoints {
            // Try to connect, but don't fail if it doesn't work immediately
            // ZMQ handles reconnection in the background
            match tokio::time::timeout(Duration::from_secs(5), socket.connect(endpoint)).await {
                Ok(Ok(_)) => {
                    info!("Price listener connected to {}", endpoint);
                    successful_connections += 1;
                }
                Ok(Err(e)) => {
                    warn!("Failed to connect to {} (will retry in background): {}", endpoint, e);
                }
                Err(_) => {
                    warn!("Connection attempt to {} timed out (will retry in background)", endpoint);
                }
            }
        }

        if total_endpoints == 0 {
            warn!("No endpoints configured, listener will be inactive");
        } else if successful_connections == 0 {
            warn!("No successful connections established ({} endpoints); listener will wait for publishers to connect", total_endpoints);
        }

        // Subscribe to price topics from all sources
        // Note: These subscriptions will work even if no publishers are connected yet
        // ZMQ will buffer/match messages once publishers connect
        if let Err(e) = socket.subscribe("prices.kalshi").await {
            error!("[INSTRUMENTATION] Failed to subscribe to prices.kalshi: {}", e);
        } else {
            info!("[INSTRUMENTATION] Successfully subscribed to prices.kalshi");
        }
        if let Err(e) = socket.subscribe("prices.poly").await {
            error!("[INSTRUMENTATION] Failed to subscribe to prices.poly: {}", e);
        } else {
            info!("[INSTRUMENTATION] Successfully subscribed to prices.poly");
        }
        if let Err(e) = socket.subscribe("crypto.prices").await {
            error!("[INSTRUMENTATION] Failed to subscribe to crypto.prices: {}", e);
        } else {
            info!("[INSTRUMENTATION] Successfully subscribed to crypto.prices");
        }
        info!("Subscription setup complete (will receive prices.kalshi.*, prices.poly.*, crypto.prices.*)");

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

        let topic = String::from_utf8_lossy(parts[0].as_ref());
        let payload_bytes = parts[1].as_ref();

        // Log EVERY topic for 5 seconds after startup to diagnose delivery
        let count_before = self.prices_received.load(Ordering::Relaxed);
        if count_before < 500 {
            info!("[INSTRUMENTATION] Message #{}: topic='{}' ({} bytes payload)", count_before, topic, payload_bytes.len());
        } else if count_before % 100 == 99 {
            debug!("[INSTRUMENTATION] Received message on topic: {}", topic);
        }

        // Parse envelope
        let count_before = self.prices_received.load(Ordering::Relaxed);
        let envelope: ZmqEnvelope<IncomingCryptoPrice> = match serde_json::from_slice(payload_bytes)
        {
            Ok(e) => e,
            Err(e) => {
                // Log parse errors at INFO to diagnose issues
                if count_before < 20 {
                    let json_str = String::from_utf8_lossy(payload_bytes);
                    let limit = json_str.len().min(2000); // Show up to 2000 chars
                    info!("[INSTRUMENTATION] Parse error on message #{}: topic='{}' ({}B)\n  Error: {}\n  JSON: {}{}",
                        count_before, topic, payload_bytes.len(), e,
                        &json_str[..limit],
                        if json_str.len() > 2000 { "... (truncated)" } else { "" }
                    );
                } else if (topic.to_string().starts_with("prices.kalshi") || topic.to_string().starts_with("prices.poly")) && count_before % 200 == 0 {
                    info!("[INSTRUMENTATION] Kalshi/Poly parse error (msg #{}): {}", count_before, e);
                } else {
                    debug!("Failed to parse incoming crypto price: {}", e);
                }
                return;
            }
        };

        let incoming = envelope.payload;

        // Log successful parse for first few Kalshi messages
        if topic.to_string().contains("kalshi") && count_before < 10 {
            let asset_display = incoming.asset.as_deref().unwrap_or("UNKNOWN");
            info!("[INSTRUMENTATION] ✓ Parsed Kalshi message #{}: asset={} platform={}", count_before, asset_display, incoming.platform);
        }

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

        // ALWAYS log for first 5 messages regardless of source
        if count < 5 {
            info!("[INSTRUMENTATION] ✓ Price #{}: {} | {} (${:.4}/${:.4}) from {}",
                count, price_data.asset, price_data.platform, price_data.yes_bid, price_data.yes_ask,
                topic.split('.').nth(1).unwrap_or("unknown"));
        }

        // Log every 100 prices to track flow (shows aggregated prices by asset)
        if count % 100 == 0 {
            info!(
                "[INSTRUMENTATION] Received {} prices total. Latest: {} | {} (${:.4}/${:.4})",
                count, price_data.asset, price_data.platform, price_data.yes_bid, price_data.yes_ask
            );
        }

        // Log latency periodically
        if count % self.config.latency_log_interval == 0 {
            debug!(
                "Crypto price latency: {}ms (seq={}, asset={}, platform={})",
                latency_ms, envelope.seq, price_data.asset, price_data.platform
            );
        }

        // Warn if latency is excessive (rate limited: every 1000 warnings)
        if latency_ms > 1000 {
            let count = self.high_latency_warnings.fetch_add(1, Ordering::Relaxed) + 1;
            if count % 1000 == 0 {
                warn!(
                    "High crypto price latency: {}ms for {} on {} (1000th warning)",
                    latency_ms, price_data.asset, price_data.platform
                );
            }
        }

        // Notify event-driven monitor if registered
        let channel = self.price_update_tx.lock().await;
        if let Some(ref tx) = *channel {
            let _ = tx.send(PriceUpdate {
                asset: price_data.asset.clone(),
                platform: price_data.platform.clone(),
                count: count + 1,
            });
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
