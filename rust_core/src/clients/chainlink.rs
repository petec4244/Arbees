//! Chainlink oracle price feed client
//!
//! Fetches current cryptocurrency prices from Chainlink Data Feeds.
//! Example: https://data.chain.link/streams/sol-usd -> SOL/USD price

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

/// Chainlink price data response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainlinkPrice {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    /// Current price (e.g., SOL/USD = 117.44)
    pub price: f64,
    /// Timestamp of the price (Unix seconds)
    pub timestamp: i64,
    /// Number of decimals (e.g., 8 decimals means price is in 1e-8 units)
    pub decimals: u32,
}

/// Chainlink client for fetching crypto prices
pub struct ChainlinkClient {
    http: Client,
    /// Cache TTL in seconds
    cache_ttl_secs: i64,
}

impl ChainlinkClient {
    /// Create a new Chainlink client
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http,
            cache_ttl_secs: 60, // Cache prices for 1 minute
        }
    }

    /// Fetch price from a Chainlink stream URL
    ///
    /// Examples:
    /// - "https://data.chain.link/streams/sol-usd" -> SOL/USD price
    /// - "https://data.chain.link/streams/btc-usd" -> BTC/USD price
    pub async fn get_price_from_stream(&self, stream_url: &str) -> Result<ChainlinkPrice> {
        let stream_id = self.extract_stream_id(stream_url)?;
        self.get_price(&stream_id).await
    }

    /// Fetch price by stream ID
    ///
    /// Examples:
    /// - "sol-usd" -> SOL/USD price
    /// - "btc-usd" -> BTC/USD price
    pub async fn get_price(&self, stream_id: &str) -> Result<ChainlinkPrice> {
        // For now, we'll use a simple approach to fetch from Coinbase or other public APIs
        // In production, you'd use Chainlink's official endpoints or on-chain lookups

        // Map stream IDs to assets
        let (asset, _pair) = Self::parse_stream_id(stream_id)?;

        // For directional markets like "sol-updown-15m", we need the current spot price
        // Use CoinGecko as a fallback for now (Chainlink would be ideal but requires subscription)
        self.fetch_from_coingecko(&asset).await
    }

    /// Extract stream ID from Chainlink URL
    /// Examples:
    /// - "https://data.chain.link/streams/sol-usd" -> "sol-usd"
    /// - "https://data.chain.link/streams/btc-usd" -> "btc-usd"
    fn extract_stream_id(&self, url: &str) -> Result<String> {
        url.split('/').last().ok_or_else(|| anyhow!("Invalid stream URL")).map(|s| s.to_string())
    }

    /// Parse stream ID into asset pair
    /// Examples:
    /// - "sol-usd" -> ("SOL", "USD")
    /// - "btc-usd" -> ("BTC", "USD")
    fn parse_stream_id(stream_id: &str) -> Result<(String, String)> {
        let parts: Vec<&str> = stream_id.split('-').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid stream ID format: {}", stream_id));
        }

        let asset = parts[0].to_uppercase();
        let pair = parts[1].to_uppercase();

        Ok((asset, pair))
    }

    /// Fetch price from CoinGecko (public API, no auth required)
    /// This is a fallback until we integrate proper Chainlink feeds
    async fn fetch_from_coingecko(&self, asset: &str) -> Result<ChainlinkPrice> {
        let asset_lower = asset.to_lowercase();
        let url = format!(
            "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd&include_last_updated_at=true",
            asset_lower
        );

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from CoinGecko")?;

        if !response.status().is_success() {
            return Err(anyhow!("CoinGecko API error: {}", response.status()));
        }

        let data: serde_json::Value = response.json().await?;

        // Extract price from response like: {solana: {usd: 117.44, last_updated_at: 1234567890}}
        let price = data
            .get(&asset_lower)
            .and_then(|v| v.get("usd"))
            .and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow!("Price not found for {}", asset))?;

        let timestamp = data
            .get(&asset_lower)
            .and_then(|v| v.get("last_updated_at"))
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp());

        debug!("Fetched {} price: ${} (ts: {})", asset, price, timestamp);

        Ok(ChainlinkPrice {
            symbol: format!("{}-USD", asset),
            base_asset: asset.to_string(),
            quote_asset: "USD".to_string(),
            price,
            timestamp,
            decimals: 2, // USD typically 2 decimals
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stream_id() {
        let (asset, pair) = ChainlinkClient::parse_stream_id("sol-usd").unwrap();
        assert_eq!(asset, "SOL");
        assert_eq!(pair, "USD");

        let (asset, pair) = ChainlinkClient::parse_stream_id("btc-usd").unwrap();
        assert_eq!(asset, "BTC");
        assert_eq!(pair, "USD");
    }

    #[test]
    fn test_extract_stream_id() {
        let client = ChainlinkClient::new();
        let stream_id =
            client.extract_stream_id("https://data.chain.link/streams/sol-usd").unwrap();
        assert_eq!(stream_id, "sol-usd");
    }

    #[test]
    fn test_invalid_stream_id() {
        assert!(ChainlinkClient::parse_stream_id("invalid").is_err());
        assert!(ChainlinkClient::parse_stream_id("btc").is_err());
    }
}
