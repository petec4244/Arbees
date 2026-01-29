//! Binance API Client
//!
//! Provides cryptocurrency price data from Binance public API.
//! No API key required for public endpoints.
//!
//! Rate limits: 1200 requests/minute (IP-based)

use super::crypto_price::{
    default_calculate_volatility, CryptoPrice, CryptoPriceProvider, ProviderStatus, VolatilityResult,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

const BASE_URL: &str = "https://api.binance.com/api/v3";
const CACHE_TTL_SECS: i64 = 30;
const RATE_LIMIT_PER_MINUTE: u64 = 1200;

/// Binance API client implementing CryptoPriceProvider
pub struct BinanceClient {
    client: Client,
    /// Cache: symbol -> (price_data, fetched_at)
    cache: Arc<RwLock<HashMap<String, (CryptoPrice, DateTime<Utc>)>>>,
    /// Rate limiting: requests this minute
    requests_this_minute: Arc<AtomicU64>,
    /// When the current rate limit window started
    rate_limit_window_start: Arc<RwLock<DateTime<Utc>>>,
    /// Current provider status (uses std::sync for sync trait method)
    status: Arc<std::sync::RwLock<ProviderStatus>>,
}

impl BinanceClient {
    /// Create a new Binance client
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
            requests_this_minute: Arc::new(AtomicU64::new(0)),
            rate_limit_window_start: Arc::new(RwLock::new(Utc::now())),
            status: Arc::new(std::sync::RwLock::new(ProviderStatus::Healthy)),
        }
    }

    /// Check and update rate limiting
    async fn check_rate_limit(&self) -> bool {
        let now = Utc::now();

        // Check if we need to reset the window
        {
            let mut window_start = self.rate_limit_window_start.write().await;
            if now.signed_duration_since(*window_start).num_seconds() >= 60 {
                *window_start = now;
                self.requests_this_minute.store(0, Ordering::SeqCst);
            }
        }

        let current = self.requests_this_minute.fetch_add(1, Ordering::SeqCst);
        if current >= RATE_LIMIT_PER_MINUTE {
            *self.status.write().unwrap() = ProviderStatus::RateLimited;
            return false;
        }
        true
    }

    /// Convert symbol to Binance trading pair (e.g., BTC -> BTCUSDT)
    fn to_trading_pair(symbol: &str) -> String {
        let sym = symbol.to_uppercase();
        // Handle already-formatted pairs
        if sym.ends_with("USDT") || sym.ends_with("USD") {
            sym
        } else {
            format!("{}USDT", sym)
        }
    }

    /// Get cached price if still valid
    async fn get_cached(&self, symbol: &str) -> Option<CryptoPrice> {
        let cache = self.cache.read().await;
        if let Some((price, fetched_at)) = cache.get(symbol) {
            let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
            if age < CACHE_TTL_SECS {
                debug!("Binance cache hit for {}", symbol);
                return Some(price.clone());
            }
        }
        None
    }

    /// Update cache with new price
    async fn update_cache(&self, symbol: &str, price: CryptoPrice) {
        let mut cache = self.cache.write().await;
        cache.insert(symbol.to_string(), (price, Utc::now()));
    }
}

impl Default for BinanceClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CryptoPriceProvider for BinanceClient {
    fn provider_name(&self) -> &str {
        "Binance"
    }

    fn status(&self) -> ProviderStatus {
        *self.status.read().unwrap()
    }

    fn symbol_to_id(&self, symbol: &str) -> String {
        Self::to_trading_pair(symbol)
    }

    fn get_price<'a>(
        &'a self,
        coin_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<CryptoPrice>> + Send + 'a>> {
        Box::pin(async move {
            let symbol = coin_id.to_uppercase();

            // Check cache
            if let Some(cached) = self.get_cached(&symbol).await {
                return Ok(cached);
            }

            // Check rate limit
            if !self.check_rate_limit().await {
                return Err(anyhow!("Binance rate limit exceeded"));
            }

            let trading_pair = Self::to_trading_pair(&symbol);
            let url = format!("{}/ticker/24hr?symbol={}", BASE_URL, trading_pair);

            debug!("Fetching {} from Binance", trading_pair);

            let response = self
                .client
                .get(&url)
                .send()
                .await
                .context("Failed to fetch from Binance")?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();

                if status.as_u16() == 429 {
                    *self.status.write().unwrap() = ProviderStatus::RateLimited;
                } else {
                    *self.status.write().unwrap() = ProviderStatus::Error;
                }

                return Err(anyhow!("Binance API error: {} - {}", status, body));
            }

            let ticker: Binance24hrTicker = response
                .json()
                .await
                .context("Failed to parse Binance response")?;

            // Update status to healthy on success
            *self.status.write().unwrap() = ProviderStatus::Healthy;

            let price = CryptoPrice {
                id: trading_pair.clone(),
                symbol: symbol.clone(),
                name: symbol.clone(), // Binance doesn't provide full names
                current_price: ticker.last_price.parse().unwrap_or(0.0),
                high_24h: ticker.high_price.parse().unwrap_or(0.0),
                low_24h: ticker.low_price.parse().unwrap_or(0.0),
                price_change_24h: ticker.price_change.parse().unwrap_or(0.0),
                price_change_percentage_24h: ticker.price_change_percent.parse().unwrap_or(0.0),
                volume_24h: ticker.quote_volume.parse().unwrap_or(0.0),
                market_cap: None, // Binance doesn't provide market cap
                last_updated: Utc::now(),
                source: "Binance".to_string(),
            };

            self.update_cache(&symbol, price.clone()).await;
            Ok(price)
        })
    }

    async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CryptoPrice>> {
        // Binance doesn't have a batch endpoint for specific symbols,
        // so we fetch individually (with caching)
        let mut prices = Vec::with_capacity(coin_ids.len());

        for coin_id in coin_ids {
            match self.get_price(coin_id).await {
                Ok(price) => prices.push(price),
                Err(e) => {
                    warn!("Failed to fetch {} from Binance: {}", coin_id, e);
                    // Continue with other coins
                }
            }
        }

        Ok(prices)
    }

    async fn get_historical_prices(
        &self,
        coin_id: &str,
        days: u32,
    ) -> Result<Vec<(DateTime<Utc>, f64)>> {
        // Check rate limit
        if !self.check_rate_limit().await {
            return Err(anyhow!("Binance rate limit exceeded"));
        }

        let trading_pair = Self::to_trading_pair(coin_id);

        // Use 1h klines for precision
        // Max 1000 candles per request, so for 30 days we need ~720 hourly candles
        let limit = (days * 24).min(1000);

        let url = format!(
            "{}/klines?symbol={}&interval=1h&limit={}",
            BASE_URL, trading_pair, limit
        );

        debug!(
            "Fetching {} historical prices from Binance ({} candles)",
            trading_pair, limit
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch klines from Binance")?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 429 {
                *self.status.write().unwrap() = ProviderStatus::RateLimited;
            }
            return Err(anyhow!("Binance klines API error: {}", status));
        }

        // Binance klines format: [[open_time, open, high, low, close, volume, close_time, ...], ...]
        let klines: Vec<Vec<serde_json::Value>> = response
            .json()
            .await
            .context("Failed to parse Binance klines")?;

        let prices: Vec<(DateTime<Utc>, f64)> = klines
            .into_iter()
            .filter_map(|candle| {
                if candle.len() < 5 {
                    return None;
                }
                let timestamp_ms = candle[0].as_i64()?;
                let close_price = candle[4].as_str()?.parse::<f64>().ok()?;
                let dt = Utc.timestamp_millis_opt(timestamp_ms).single()?;
                Some((dt, close_price))
            })
            .collect();

        Ok(prices)
    }

    fn calculate_volatility<'a>(
        &'a self,
        coin_id: &'a str,
        days: u32,
    ) -> Pin<Box<dyn Future<Output = Result<VolatilityResult>> + Send + 'a>> {
        default_calculate_volatility(self, coin_id, days)
    }
}

/// Binance 24hr ticker response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Binance24hrTicker {
    symbol: String,
    price_change: String,
    price_change_percent: String,
    #[allow(dead_code)]
    weighted_avg_price: String,
    #[allow(dead_code)]
    prev_close_price: String,
    last_price: String,
    #[allow(dead_code)]
    last_qty: String,
    #[allow(dead_code)]
    bid_price: String,
    #[allow(dead_code)]
    bid_qty: String,
    #[allow(dead_code)]
    ask_price: String,
    #[allow(dead_code)]
    ask_qty: String,
    #[allow(dead_code)]
    open_price: String,
    high_price: String,
    low_price: String,
    #[allow(dead_code)]
    volume: String,
    quote_volume: String,
    #[allow(dead_code)]
    open_time: i64,
    #[allow(dead_code)]
    close_time: i64,
    #[allow(dead_code)]
    first_id: i64,
    #[allow(dead_code)]
    last_id: i64,
    #[allow(dead_code)]
    count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        assert_eq!(BinanceClient::to_trading_pair("BTC"), "BTCUSDT");
        assert_eq!(BinanceClient::to_trading_pair("eth"), "ETHUSDT");
        assert_eq!(BinanceClient::to_trading_pair("BTCUSDT"), "BTCUSDT");
    }

    #[tokio::test]
    async fn test_client_creation() {
        let client = BinanceClient::new();
        assert_eq!(client.provider_name(), "Binance");
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_get_price() {
        let client = BinanceClient::new();
        let price = client.get_price("BTC").await.unwrap();
        assert_eq!(price.symbol, "BTC");
        assert!(price.current_price > 0.0);
        assert_eq!(price.source, "Binance");
    }
}
