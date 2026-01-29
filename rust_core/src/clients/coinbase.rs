//! Coinbase Exchange API Client
//!
//! Provides cryptocurrency price data from Coinbase Exchange public API.
//! No API key required for public endpoints.
//!
//! Rate limits: 10 requests/second (IP-based)

use super::crypto_price::{CryptoPrice, CryptoPriceProvider, ProviderStatus, VolatilityResult};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

const BASE_URL: &str = "https://api.exchange.coinbase.com";
const CACHE_TTL_SECS: i64 = 30;
const RATE_LIMIT_PER_SECOND: u64 = 10;

/// Coinbase Exchange API client implementing CryptoPriceProvider
pub struct CoinbaseClient {
    client: Client,
    /// Cache: symbol -> (price_data, fetched_at)
    cache: Arc<RwLock<HashMap<String, (CryptoPrice, DateTime<Utc>)>>>,
    /// Rate limiting: requests this second
    requests_this_second: Arc<AtomicU64>,
    /// When the current rate limit window started
    rate_limit_window_start: Arc<RwLock<DateTime<Utc>>>,
    /// Current provider status (uses std::sync for sync trait method)
    status: Arc<std::sync::RwLock<ProviderStatus>>,
}

impl CoinbaseClient {
    /// Create a new Coinbase client
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
            requests_this_second: Arc::new(AtomicU64::new(0)),
            rate_limit_window_start: Arc::new(RwLock::new(Utc::now())),
            status: Arc::new(std::sync::RwLock::new(ProviderStatus::Healthy)),
        }
    }

    /// Check and update rate limiting
    async fn check_rate_limit(&self) -> bool {
        let now = Utc::now();

        // Check if we need to reset the window (1 second)
        {
            let mut window_start = self.rate_limit_window_start.write().await;
            if now.signed_duration_since(*window_start).num_milliseconds() >= 1000 {
                *window_start = now;
                self.requests_this_second.store(0, Ordering::SeqCst);
            }
        }

        let current = self.requests_this_second.fetch_add(1, Ordering::SeqCst);
        if current >= RATE_LIMIT_PER_SECOND {
            *self.status.write().unwrap() = ProviderStatus::RateLimited;
            return false;
        }
        true
    }

    /// Convert symbol to Coinbase product ID (e.g., BTC -> BTC-USD)
    fn to_product_id(symbol: &str) -> String {
        let sym = symbol.to_uppercase();
        // Handle already-formatted pairs
        if sym.contains('-') {
            sym
        } else {
            format!("{}-USD", sym)
        }
    }

    /// Get cached price if still valid
    async fn get_cached(&self, symbol: &str) -> Option<CryptoPrice> {
        let cache = self.cache.read().await;
        if let Some((price, fetched_at)) = cache.get(symbol) {
            let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
            if age < CACHE_TTL_SECS {
                debug!("Coinbase cache hit for {}", symbol);
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

    /// Fetch product stats (24h high/low/volume)
    async fn get_stats(&self, product_id: &str) -> Result<CoinbaseStats> {
        let url = format!("{}/products/{}/stats", BASE_URL, product_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch stats from Coinbase")?;

        if !response.status().is_success() {
            return Err(anyhow!("Coinbase stats API error: {}", response.status()));
        }

        response
            .json()
            .await
            .context("Failed to parse Coinbase stats")
    }
}

impl Default for CoinbaseClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CryptoPriceProvider for CoinbaseClient {
    fn provider_name(&self) -> &str {
        "Coinbase"
    }

    fn status(&self) -> ProviderStatus {
        *self.status.read().unwrap()
    }

    fn symbol_to_id(&self, symbol: &str) -> String {
        Self::to_product_id(symbol)
    }

    async fn get_price(&self, coin_id: &str) -> Result<CryptoPrice> {
        let symbol = coin_id.to_uppercase();

        // Check cache
        if let Some(cached) = self.get_cached(&symbol).await {
            return Ok(cached);
        }

        // Check rate limit
        if !self.check_rate_limit().await {
            return Err(anyhow!("Coinbase rate limit exceeded"));
        }

        let product_id = Self::to_product_id(&symbol);

        // Fetch ticker for current price
        let ticker_url = format!("{}/products/{}/ticker", BASE_URL, product_id);

        debug!("Fetching {} from Coinbase", product_id);

        let response = self
            .client
            .get(&ticker_url)
            .send()
            .await
            .context("Failed to fetch from Coinbase")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 429 {
                *self.status.write().unwrap() = ProviderStatus::RateLimited;
            } else {
                *self.status.write().unwrap() = ProviderStatus::Error;
            }

            return Err(anyhow!("Coinbase API error: {} - {}", status, body));
        }

        let ticker: CoinbaseTicker = response
            .json()
            .await
            .context("Failed to parse Coinbase ticker")?;

        // Fetch stats for 24h data (separate request)
        let stats = self.get_stats(&product_id).await.ok();

        // Update status to healthy on success
        *self.status.write().unwrap() = ProviderStatus::Healthy;

        let current_price: f64 = ticker.price.parse().unwrap_or(0.0);

        let (high_24h, low_24h, volume_24h) = if let Some(ref s) = stats {
            (
                s.high.parse().unwrap_or(current_price),
                s.low.parse().unwrap_or(current_price),
                s.volume.parse().unwrap_or(0.0) * current_price, // Convert to USD
            )
        } else {
            (current_price, current_price, 0.0)
        };

        // Calculate price change from 24h open
        let open_24h: f64 = stats
            .as_ref()
            .map(|s| s.open.parse().unwrap_or(current_price))
            .unwrap_or(current_price);
        let price_change_24h = current_price - open_24h;
        let price_change_percentage_24h = if open_24h > 0.0 {
            (price_change_24h / open_24h) * 100.0
        } else {
            0.0
        };

        let price = CryptoPrice {
            id: product_id.clone(),
            symbol: symbol.clone(),
            name: symbol.clone(), // Coinbase ticker doesn't provide full names
            current_price,
            high_24h,
            low_24h,
            price_change_24h,
            price_change_percentage_24h,
            volume_24h,
            market_cap: None, // Coinbase doesn't provide market cap
            last_updated: Utc::now(),
            source: "Coinbase".to_string(),
        };

        self.update_cache(&symbol, price.clone()).await;
        Ok(price)
    }

    async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CryptoPrice>> {
        let mut prices = Vec::with_capacity(coin_ids.len());

        for coin_id in coin_ids {
            match self.get_price(coin_id).await {
                Ok(price) => prices.push(price),
                Err(e) => {
                    warn!("Failed to fetch {} from Coinbase: {}", coin_id, e);
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
            return Err(anyhow!("Coinbase rate limit exceeded"));
        }

        let product_id = Self::to_product_id(coin_id);

        // Use 1-hour granularity (3600 seconds)
        // Max 300 candles per request
        let granularity = 3600;
        let limit = (days * 24).min(300);

        // Calculate start and end times
        let end = Utc::now();
        let start = end - chrono::Duration::hours(limit as i64);

        let url = format!(
            "{}/products/{}/candles?granularity={}&start={}&end={}",
            BASE_URL,
            product_id,
            granularity,
            start.to_rfc3339(),
            end.to_rfc3339()
        );

        debug!(
            "Fetching {} historical prices from Coinbase ({} candles)",
            product_id, limit
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch candles from Coinbase")?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 429 {
                *self.status.write().unwrap() = ProviderStatus::RateLimited;
            }
            return Err(anyhow!("Coinbase candles API error: {}", status));
        }

        // Coinbase candles format: [[time, low, high, open, close, volume], ...]
        let candles: Vec<Vec<f64>> = response
            .json()
            .await
            .context("Failed to parse Coinbase candles")?;

        let prices: Vec<(DateTime<Utc>, f64)> = candles
            .into_iter()
            .filter_map(|candle| {
                if candle.len() < 5 {
                    return None;
                }
                let timestamp = candle[0] as i64;
                let close_price = candle[4];
                let dt = Utc.timestamp_opt(timestamp, 0).single()?;
                Some((dt, close_price))
            })
            // Coinbase returns newest first, reverse for chronological order
            .rev()
            .collect();

        Ok(prices)
    }

    async fn calculate_volatility(&self, coin_id: &str, days: u32) -> Result<VolatilityResult> {
        // Use default implementation from trait
        <Self as CryptoPriceProvider>::calculate_volatility(self, coin_id, days).await
    }
}

/// Coinbase ticker response
#[derive(Debug, Deserialize)]
struct CoinbaseTicker {
    #[allow(dead_code)]
    trade_id: Option<i64>,
    price: String,
    #[allow(dead_code)]
    size: Option<String>,
    #[allow(dead_code)]
    bid: Option<String>,
    #[allow(dead_code)]
    ask: Option<String>,
    #[allow(dead_code)]
    volume: Option<String>,
    #[allow(dead_code)]
    time: Option<String>,
}

/// Coinbase 24hr stats response
#[derive(Debug, Deserialize)]
struct CoinbaseStats {
    open: String,
    high: String,
    low: String,
    volume: String,
    #[allow(dead_code)]
    last: String,
    #[allow(dead_code)]
    volume_30day: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_product_id_conversion() {
        assert_eq!(CoinbaseClient::to_product_id("BTC"), "BTC-USD");
        assert_eq!(CoinbaseClient::to_product_id("eth"), "ETH-USD");
        assert_eq!(CoinbaseClient::to_product_id("BTC-USD"), "BTC-USD");
    }

    #[tokio::test]
    async fn test_client_creation() {
        let client = CoinbaseClient::new();
        assert_eq!(client.provider_name(), "Coinbase");
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_get_price() {
        let client = CoinbaseClient::new();
        let price = client.get_price("BTC").await.unwrap();
        assert_eq!(price.symbol, "BTC");
        assert!(price.current_price > 0.0);
        assert_eq!(price.source, "Coinbase");
    }
}
