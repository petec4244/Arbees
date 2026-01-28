//! CoinGecko API Client
//!
//! Provides cryptocurrency price data, market caps, and historical data
//! for crypto prediction market probability calculations.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::debug;

/// CoinGecko API client with caching
pub struct CoinGeckoClient {
    client: Client,
    base_url: String,
    /// Cache: coin_id -> (price_data, fetched_at)
    cache: Arc<RwLock<HashMap<String, (CoinPrice, DateTime<Utc>)>>>,
    /// Cache TTL in seconds
    cache_ttl_secs: i64,
}

/// Coin price data from CoinGecko
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinPrice {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub current_price: f64,
    pub market_cap: f64,
    pub total_volume: f64,
    pub high_24h: f64,
    pub low_24h: f64,
    pub price_change_24h: f64,
    pub price_change_percentage_24h: f64,
    pub ath: f64,
    pub ath_date: Option<String>,
    pub atl: f64,
    pub atl_date: Option<String>,
    pub last_updated: Option<String>,
}

/// Market data for multiple coins
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketData {
    pub prices: Vec<CoinPrice>,
    pub fetched_at: DateTime<Utc>,
}

/// Simplified price response from CoinGecko /simple/price endpoint
#[derive(Debug, Deserialize)]
struct SimplePriceResponse {
    #[serde(flatten)]
    prices: HashMap<String, SimplePriceData>,
}

#[derive(Debug, Deserialize)]
struct SimplePriceData {
    usd: f64,
    #[serde(default)]
    usd_market_cap: Option<f64>,
    #[serde(default)]
    usd_24h_vol: Option<f64>,
    #[serde(default)]
    usd_24h_change: Option<f64>,
}

/// Market chart data for historical prices
#[derive(Debug, Deserialize)]
pub struct MarketChartData {
    pub prices: Vec<(f64, f64)>, // (timestamp_ms, price)
}

/// Volatility calculation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityData {
    pub coin_id: String,
    /// Annualized volatility (standard deviation of log returns * sqrt(365))
    pub annualized_volatility: f64,
    /// Daily volatility
    pub daily_volatility: f64,
    /// Number of days used in calculation
    pub days_used: u32,
    pub calculated_at: DateTime<Utc>,
}

impl CoinGeckoClient {
    /// Create a new CoinGecko client
    pub fn new() -> Self {
        Self::with_cache_ttl(60) // Default 60 second cache
    }

    /// Create with custom cache TTL
    pub fn with_cache_ttl(cache_ttl_secs: i64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: "https://api.coingecko.com/api/v3".to_string(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_secs,
        }
    }

    /// Map common symbols to CoinGecko IDs
    pub fn symbol_to_id(symbol: &str) -> &str {
        match symbol.to_uppercase().as_str() {
            "BTC" | "BITCOIN" => "bitcoin",
            "ETH" | "ETHEREUM" => "ethereum",
            "SOL" | "SOLANA" => "solana",
            "XRP" | "RIPPLE" => "ripple",
            "DOGE" | "DOGECOIN" => "dogecoin",
            "ADA" | "CARDANO" => "cardano",
            "AVAX" | "AVALANCHE" => "avalanche-2",
            "DOT" | "POLKADOT" => "polkadot",
            "MATIC" | "POLYGON" => "matic-network",
            "LINK" | "CHAINLINK" => "chainlink",
            "UNI" | "UNISWAP" => "uniswap",
            "ATOM" | "COSMOS" => "cosmos",
            "LTC" | "LITECOIN" => "litecoin",
            "SHIB" | "SHIBAINU" => "shiba-inu",
            "TRX" | "TRON" => "tron",
            "XLM" | "STELLAR" => "stellar",
            "NEAR" => "near",
            "APT" | "APTOS" => "aptos",
            "ARB" | "ARBITRUM" => "arbitrum",
            "OP" | "OPTIMISM" => "optimism",
            _ => symbol, // Assume it's already a CoinGecko ID
        }
    }

    /// Get current price for a single coin
    pub async fn get_price(&self, coin_id: &str) -> Result<CoinPrice> {
        let coin_id = Self::symbol_to_id(coin_id).to_lowercase();

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some((price, fetched_at)) = cache.get(&coin_id) {
                let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
                if age < self.cache_ttl_secs {
                    debug!("Cache hit for {}", coin_id);
                    return Ok(price.clone());
                }
            }
        }

        // Fetch from API
        let url = format!(
            "{}/coins/markets?vs_currency=usd&ids={}&order=market_cap_desc&per_page=1&page=1&sparkline=false&price_change_percentage=24h",
            self.base_url, coin_id
        );

        debug!("Fetching price for {} from CoinGecko", coin_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from CoinGecko")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "CoinGecko API error: {} - {}",
                status,
                body
            ));
        }

        let coins: Vec<CoinGeckoMarketResponse> = response
            .json()
            .await
            .context("Failed to parse CoinGecko response")?;

        let coin = coins
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Coin not found: {}", coin_id))?;

        let price = CoinPrice {
            id: coin.id.clone(),
            symbol: coin.symbol,
            name: coin.name,
            current_price: coin.current_price,
            market_cap: coin.market_cap.unwrap_or(0.0),
            total_volume: coin.total_volume.unwrap_or(0.0),
            high_24h: coin.high_24h.unwrap_or(coin.current_price),
            low_24h: coin.low_24h.unwrap_or(coin.current_price),
            price_change_24h: coin.price_change_24h.unwrap_or(0.0),
            price_change_percentage_24h: coin.price_change_percentage_24h.unwrap_or(0.0),
            ath: coin.ath.unwrap_or(coin.current_price),
            ath_date: coin.ath_date,
            atl: coin.atl.unwrap_or(coin.current_price),
            atl_date: coin.atl_date,
            last_updated: coin.last_updated,
        };

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(coin_id, (price.clone(), Utc::now()));
        }

        Ok(price)
    }

    /// Get prices for multiple coins at once
    pub async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CoinPrice>> {
        let ids: Vec<String> = coin_ids
            .iter()
            .map(|id| Self::symbol_to_id(id).to_lowercase())
            .collect();

        let ids_str = ids.join(",");

        let url = format!(
            "{}/coins/markets?vs_currency=usd&ids={}&order=market_cap_desc&per_page=100&page=1&sparkline=false",
            self.base_url, ids_str
        );

        debug!("Fetching prices for {} coins from CoinGecko", ids.len());

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from CoinGecko")?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(anyhow!("CoinGecko API error: {}", status));
        }

        let coins: Vec<CoinGeckoMarketResponse> = response
            .json()
            .await
            .context("Failed to parse CoinGecko response")?;

        let prices: Vec<CoinPrice> = coins
            .into_iter()
            .map(|coin| CoinPrice {
                id: coin.id,
                symbol: coin.symbol,
                name: coin.name,
                current_price: coin.current_price,
                market_cap: coin.market_cap.unwrap_or(0.0),
                total_volume: coin.total_volume.unwrap_or(0.0),
                high_24h: coin.high_24h.unwrap_or(coin.current_price),
                low_24h: coin.low_24h.unwrap_or(coin.current_price),
                price_change_24h: coin.price_change_24h.unwrap_or(0.0),
                price_change_percentage_24h: coin.price_change_percentage_24h.unwrap_or(0.0),
                ath: coin.ath.unwrap_or(coin.current_price),
                ath_date: coin.ath_date,
                atl: coin.atl.unwrap_or(coin.current_price),
                atl_date: coin.atl_date,
                last_updated: coin.last_updated,
            })
            .collect();

        // Update cache for all fetched coins
        {
            let mut cache = self.cache.write().await;
            let now = Utc::now();
            for price in &prices {
                cache.insert(price.id.clone(), (price.clone(), now));
            }
        }

        Ok(prices)
    }

    /// Get historical price data for volatility calculation
    pub async fn get_market_chart(&self, coin_id: &str, days: u32) -> Result<MarketChartData> {
        let coin_id = Self::symbol_to_id(coin_id).to_lowercase();

        let url = format!(
            "{}/coins/{}/market_chart?vs_currency=usd&days={}",
            self.base_url, coin_id, days
        );

        debug!("Fetching {} day chart for {} from CoinGecko", days, coin_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch market chart")?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(anyhow!("CoinGecko API error: {}", status));
        }

        let chart: MarketChartData = response
            .json()
            .await
            .context("Failed to parse market chart")?;

        Ok(chart)
    }

    /// Calculate historical volatility
    pub async fn calculate_volatility(&self, coin_id: &str, days: u32) -> Result<VolatilityData> {
        let chart = self.get_market_chart(coin_id, days).await?;

        if chart.prices.len() < 2 {
            return Err(anyhow!("Insufficient price data for volatility calculation"));
        }

        // Calculate log returns
        let mut log_returns: Vec<f64> = Vec::with_capacity(chart.prices.len() - 1);
        for i in 1..chart.prices.len() {
            let prev_price = chart.prices[i - 1].1;
            let curr_price = chart.prices[i].1;
            if prev_price > 0.0 && curr_price > 0.0 {
                log_returns.push((curr_price / prev_price).ln());
            }
        }

        if log_returns.is_empty() {
            return Err(anyhow!("No valid returns for volatility calculation"));
        }

        // Calculate mean
        let mean: f64 = log_returns.iter().sum::<f64>() / log_returns.len() as f64;

        // Calculate variance
        let variance: f64 = log_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / log_returns.len() as f64;

        let daily_volatility = variance.sqrt();

        // Annualize (approximate - CoinGecko returns hourly data for recent days)
        // For days <= 90, data is hourly, so we have ~24 data points per day
        let periods_per_day: f64 = if days <= 90 { 24.0 } else { 1.0 };
        let annualized_volatility = daily_volatility * (365.0_f64 * periods_per_day).sqrt();

        Ok(VolatilityData {
            coin_id: Self::symbol_to_id(coin_id).to_string(),
            annualized_volatility,
            daily_volatility,
            days_used: days,
            calculated_at: Utc::now(),
        })
    }

    /// Get top coins by market cap
    pub async fn get_top_coins(&self, limit: u32) -> Result<Vec<CoinPrice>> {
        let url = format!(
            "{}/coins/markets?vs_currency=usd&order=market_cap_desc&per_page={}&page=1&sparkline=false",
            self.base_url, limit
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch top coins")?;

        if !response.status().is_success() {
            return Err(anyhow!("CoinGecko API error: {}", response.status()));
        }

        let coins: Vec<CoinGeckoMarketResponse> = response.json().await?;

        Ok(coins
            .into_iter()
            .map(|coin| CoinPrice {
                id: coin.id,
                symbol: coin.symbol,
                name: coin.name,
                current_price: coin.current_price,
                market_cap: coin.market_cap.unwrap_or(0.0),
                total_volume: coin.total_volume.unwrap_or(0.0),
                high_24h: coin.high_24h.unwrap_or(coin.current_price),
                low_24h: coin.low_24h.unwrap_or(coin.current_price),
                price_change_24h: coin.price_change_24h.unwrap_or(0.0),
                price_change_percentage_24h: coin.price_change_percentage_24h.unwrap_or(0.0),
                ath: coin.ath.unwrap_or(coin.current_price),
                ath_date: coin.ath_date,
                atl: coin.atl.unwrap_or(coin.current_price),
                atl_date: coin.atl_date,
                last_updated: coin.last_updated,
            })
            .collect())
    }
}

impl Default for CoinGeckoClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal response struct matching CoinGecko API
#[derive(Debug, Deserialize)]
struct CoinGeckoMarketResponse {
    id: String,
    symbol: String,
    name: String,
    current_price: f64,
    market_cap: Option<f64>,
    total_volume: Option<f64>,
    high_24h: Option<f64>,
    low_24h: Option<f64>,
    price_change_24h: Option<f64>,
    price_change_percentage_24h: Option<f64>,
    ath: Option<f64>,
    ath_date: Option<String>,
    atl: Option<f64>,
    atl_date: Option<String>,
    last_updated: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_to_id() {
        assert_eq!(CoinGeckoClient::symbol_to_id("BTC"), "bitcoin");
        assert_eq!(CoinGeckoClient::symbol_to_id("ETH"), "ethereum");
        assert_eq!(CoinGeckoClient::symbol_to_id("btc"), "bitcoin");
        assert_eq!(CoinGeckoClient::symbol_to_id("unknown"), "unknown");
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_get_price() {
        let client = CoinGeckoClient::new();
        let price = client.get_price("bitcoin").await.unwrap();
        assert_eq!(price.symbol, "btc");
        assert!(price.current_price > 0.0);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_calculate_volatility() {
        let client = CoinGeckoClient::new();
        let vol = client.calculate_volatility("bitcoin", 30).await.unwrap();
        assert!(vol.annualized_volatility > 0.0);
        assert!(vol.daily_volatility > 0.0);
    }
}
