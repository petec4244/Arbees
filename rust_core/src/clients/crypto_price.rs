//! Crypto Price Provider Trait
//!
//! Defines a common interface for cryptocurrency price data providers.
//! Implementations include Binance, Coinbase, and CoinGecko.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unified cryptocurrency price data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoPrice {
    /// Provider-specific coin identifier (e.g., "bitcoin" for CoinGecko, "BTC" for Binance)
    pub id: String,
    /// Ticker symbol (e.g., "BTC", "ETH")
    pub symbol: String,
    /// Human-readable name (e.g., "Bitcoin", "Ethereum")
    pub name: String,
    /// Current price in USD
    pub current_price: f64,
    /// 24-hour high price
    pub high_24h: f64,
    /// 24-hour low price
    pub low_24h: f64,
    /// Absolute price change in 24h
    pub price_change_24h: f64,
    /// Percentage price change in 24h
    pub price_change_percentage_24h: f64,
    /// 24-hour trading volume in USD
    pub volume_24h: f64,
    /// Market capitalization (if available)
    pub market_cap: Option<f64>,
    /// When this price was last updated
    pub last_updated: DateTime<Utc>,
    /// Source provider name
    pub source: String,
}

/// Provider health/availability status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderStatus {
    /// Provider is operational and responding normally
    Healthy,
    /// Provider is rate limited, requests should be delayed
    RateLimited,
    /// Provider returned an error, may be temporarily down
    Error,
    /// Provider is not available (configuration issue, network problem)
    Unavailable,
}

impl Default for ProviderStatus {
    fn default() -> Self {
        Self::Healthy
    }
}

/// Result of volatility calculation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityResult {
    /// Coin identifier
    pub coin_id: String,
    /// Annualized volatility (standard deviation of log returns * sqrt(365))
    pub annualized_volatility: f64,
    /// Daily volatility (standard deviation of daily log returns)
    pub daily_volatility: f64,
    /// Number of days/periods used in calculation
    pub periods_used: u32,
    /// When this was calculated
    pub calculated_at: DateTime<Utc>,
}

/// Common trait for cryptocurrency price data providers
///
/// Implementations must be Send + Sync for use in async contexts.
/// All methods are async and return Results to handle network/API errors.
#[async_trait]
pub trait CryptoPriceProvider: Send + Sync {
    /// Get the provider's display name (e.g., "Binance", "Coinbase", "CoinGecko")
    fn provider_name(&self) -> &str;

    /// Get the current status of the provider
    fn status(&self) -> ProviderStatus;

    /// Convert a common symbol (e.g., "BTC") to this provider's identifier format
    ///
    /// Examples:
    /// - Binance: "BTC" -> "BTCUSDT"
    /// - Coinbase: "BTC" -> "BTC-USD"
    /// - CoinGecko: "BTC" -> "bitcoin"
    fn symbol_to_id(&self, symbol: &str) -> String;

    /// Get current price for a single coin
    ///
    /// # Arguments
    /// * `coin_id` - Can be a symbol (BTC) or provider-specific ID
    ///
    /// # Returns
    /// * `Ok(CryptoPrice)` - Current price data
    /// * `Err` - If the coin is not found or API error
    async fn get_price(&self, coin_id: &str) -> Result<CryptoPrice>;

    /// Get prices for multiple coins at once
    ///
    /// More efficient than multiple get_price calls for providers that support batch requests.
    ///
    /// # Arguments
    /// * `coin_ids` - Slice of symbols or provider-specific IDs
    ///
    /// # Returns
    /// * `Ok(Vec<CryptoPrice>)` - Prices for all found coins (missing coins excluded)
    /// * `Err` - If API error
    async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CryptoPrice>>;

    /// Get historical price data for volatility calculation
    ///
    /// # Arguments
    /// * `coin_id` - Symbol or provider-specific ID
    /// * `days` - Number of days of historical data to fetch
    ///
    /// # Returns
    /// * `Ok(Vec<(DateTime<Utc>, f64)>)` - List of (timestamp, price) pairs
    /// * `Err` - If the coin is not found or API error
    async fn get_historical_prices(
        &self,
        coin_id: &str,
        days: u32,
    ) -> Result<Vec<(DateTime<Utc>, f64)>>;

    /// Calculate historical volatility for a coin
    ///
    /// Default implementation uses get_historical_prices and computes log returns.
    ///
    /// # Arguments
    /// * `coin_id` - Symbol or provider-specific ID
    /// * `days` - Number of days to use for calculation
    ///
    /// # Returns
    /// * `Ok(VolatilityResult)` - Volatility data
    /// * `Err` - If insufficient data or API error
    async fn calculate_volatility(&self, coin_id: &str, days: u32) -> Result<VolatilityResult> {
        let prices = self.get_historical_prices(coin_id, days).await?;

        if prices.len() < 2 {
            anyhow::bail!("Insufficient price data for volatility calculation");
        }

        // Calculate log returns
        let mut log_returns: Vec<f64> = Vec::with_capacity(prices.len() - 1);
        for i in 1..prices.len() {
            let prev_price = prices[i - 1].1;
            let curr_price = prices[i].1;
            if prev_price > 0.0 && curr_price > 0.0 {
                log_returns.push((curr_price / prev_price).ln());
            }
        }

        if log_returns.is_empty() {
            anyhow::bail!("No valid returns for volatility calculation");
        }

        // Calculate mean
        let mean: f64 = log_returns.iter().sum::<f64>() / log_returns.len() as f64;

        // Calculate variance
        let variance: f64 = log_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / log_returns.len() as f64;

        let std_dev = variance.sqrt();

        // Estimate periods per day based on data density
        let first_ts = prices.first().unwrap().0;
        let last_ts = prices.last().unwrap().0;
        let duration_days = last_ts
            .signed_duration_since(first_ts)
            .num_hours() as f64
            / 24.0;
        let periods_per_day = if duration_days > 0.0 {
            prices.len() as f64 / duration_days
        } else {
            1.0
        };

        // Annualize: multiply by sqrt(periods per year)
        // For hourly data: sqrt(24 * 365) ≈ 93.6
        // For daily data: sqrt(365) ≈ 19.1
        let annualized_volatility = std_dev * (periods_per_day * 365.0).sqrt();

        // Daily volatility: adjust from per-period to per-day
        let daily_volatility = std_dev * periods_per_day.sqrt();

        Ok(VolatilityResult {
            coin_id: self.symbol_to_id(coin_id),
            annualized_volatility,
            daily_volatility,
            periods_used: log_returns.len() as u32,
            calculated_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_status_default() {
        let status = ProviderStatus::default();
        assert_eq!(status, ProviderStatus::Healthy);
    }

    #[test]
    fn test_crypto_price_serialization() {
        let price = CryptoPrice {
            id: "bitcoin".to_string(),
            symbol: "BTC".to_string(),
            name: "Bitcoin".to_string(),
            current_price: 50000.0,
            high_24h: 51000.0,
            low_24h: 49000.0,
            price_change_24h: 1000.0,
            price_change_percentage_24h: 2.0,
            volume_24h: 1_000_000_000.0,
            market_cap: Some(1_000_000_000_000.0),
            last_updated: Utc::now(),
            source: "test".to_string(),
        };

        let json = serde_json::to_string(&price).unwrap();
        assert!(json.contains("bitcoin"));
        assert!(json.contains("50000"));
    }
}
