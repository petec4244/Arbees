//! Crypto Event Provider
//!
//! Discovers and tracks cryptocurrency prediction markets from Kalshi and Polymarket.
//! Combines market discovery with live price data from CoinGecko.

use super::{CryptoStateData, EventInfo, EventProvider, EventState, EventStatus, StateData};
use crate::clients::coingecko::CoinGeckoClient;
use crate::models::{CryptoPredictionType, MarketType};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Crypto event provider
///
/// Discovers crypto prediction markets and enriches them with live price data.
pub struct CryptoEventProvider {
    /// HTTP client for API requests
    client: Client,
    /// CoinGecko client for price data
    coingecko: Arc<CoinGeckoClient>,
    /// Cache for discovered markets
    market_cache: Arc<RwLock<HashMap<String, CryptoMarket>>>,
    /// Last cache update time
    last_update: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Cache TTL in seconds
    cache_ttl_secs: i64,
}

/// Represents a crypto prediction market
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoMarket {
    pub market_id: String,
    pub platform: String, // "kalshi" or "polymarket"
    pub asset: String,    // "BTC", "ETH", etc.
    pub target_price: f64,
    pub target_date: DateTime<Utc>,
    pub prediction_type: CryptoPredictionType,
    pub title: String,
    pub description: Option<String>,
    pub yes_price: Option<f64>,
    pub no_price: Option<f64>,
    pub volume: Option<f64>,
    pub liquidity: Option<f64>,
    pub status: EventStatus,
    pub discovered_at: DateTime<Utc>,
}

/// Common crypto assets to track
pub const TRACKED_ASSETS: &[&str] = &[
    "BTC", "ETH", "SOL", "XRP", "DOGE", "ADA", "AVAX", "DOT", "MATIC", "LINK",
];

impl CryptoEventProvider {
    /// Create a new crypto event provider
    pub fn new() -> Self {
        Self::with_coingecko(Arc::new(CoinGeckoClient::new()))
    }

    /// Create with shared CoinGecko client
    pub fn with_coingecko(coingecko: Arc<CoinGeckoClient>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            coingecko,
            market_cache: Arc::new(RwLock::new(HashMap::new())),
            last_update: Arc::new(RwLock::new(None)),
            cache_ttl_secs: 300, // 5 minute cache
        }
    }

    /// Check if cache needs refresh
    async fn needs_refresh(&self) -> bool {
        let last = self.last_update.read().await;
        match *last {
            None => true,
            Some(t) => Utc::now().signed_duration_since(t).num_seconds() > self.cache_ttl_secs,
        }
    }

    /// Refresh the market cache from Polymarket and Kalshi
    pub async fn refresh_markets(&self) -> Result<()> {
        info!("Refreshing crypto markets...");

        let mut all_markets = Vec::new();

        // Fetch from Polymarket
        match self.fetch_polymarket_crypto_markets().await {
            Ok(markets) => {
                info!("Found {} crypto markets on Polymarket", markets.len());
                all_markets.extend(markets);
            }
            Err(e) => {
                warn!("Failed to fetch Polymarket crypto markets: {}", e);
            }
        }

        // Fetch from Kalshi
        match self.fetch_kalshi_crypto_markets().await {
            Ok(markets) => {
                info!("Found {} crypto markets on Kalshi", markets.len());
                all_markets.extend(markets);
            }
            Err(e) => {
                warn!("Failed to fetch Kalshi crypto markets: {}", e);
            }
        }

        // Update cache
        {
            let mut cache = self.market_cache.write().await;
            cache.clear();
            for market in all_markets {
                cache.insert(market.market_id.clone(), market);
            }
        }

        // Update timestamp
        {
            let mut last = self.last_update.write().await;
            *last = Some(Utc::now());
        }

        Ok(())
    }

    /// Fetch crypto markets from Polymarket Gamma API
    async fn fetch_polymarket_crypto_markets(&self) -> Result<Vec<CryptoMarket>> {
        let url = "https://gamma-api.polymarket.com/markets?closed=false&limit=100";

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch Polymarket markets")?;

        if !response.status().is_success() {
            return Err(anyhow!("Polymarket API error: {}", response.status()));
        }

        // Get raw text first for debugging
        let text = response.text().await?;

        let markets: Vec<PolymarketMarket> = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                // Log a snippet of the response for debugging
                let snippet: String = text.chars().take(500).collect();
                warn!("Polymarket JSON parse error: {}. Response snippet: {}", e, snippet);
                return Err(anyhow!("JSON parse error: {}", e));
            }
        };

        info!("Polymarket returned {} total markets", markets.len());

        // Filter for crypto markets
        let crypto_markets: Vec<CryptoMarket> = markets
            .into_iter()
            .filter_map(|m| self.parse_polymarket_crypto(&m))
            .collect();

        if !crypto_markets.is_empty() {
            info!(
                "Found {} crypto markets on Polymarket: {:?}",
                crypto_markets.len(),
                crypto_markets.iter().map(|m| &m.title).collect::<Vec<_>>()
            );
        }

        Ok(crypto_markets)
    }

    /// Parse a Polymarket market into a CryptoMarket if it's crypto-related
    fn parse_polymarket_crypto(&self, market: &PolymarketMarket) -> Option<CryptoMarket> {
        // Check if it's a crypto market using word-boundary matching
        let asset = TRACKED_ASSETS
            .iter()
            .find(|&asset| contains_crypto_asset(&market.question, asset))?;

        // Try to parse price target from question
        let (target_price, prediction_type) = self.parse_price_target(&market.question)?;

        // Parse end date
        let target_date = market
            .end_date
            .as_ref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::days(365));

        Some(CryptoMarket {
            market_id: format!("polymarket:{}", market.condition_id),
            platform: "polymarket".to_string(),
            asset: asset.to_string(),
            target_price,
            target_date,
            prediction_type,
            title: market.question.clone(),
            description: market.description.clone(),
            yes_price: market.outcome_prices.as_ref().and_then(|p| p.get(0).copied()),
            no_price: market.outcome_prices.as_ref().and_then(|p| p.get(1).copied()),
            volume: market.volume.map(|v| v as f64),
            liquidity: market.liquidity.map(|l| l as f64),
            status: if market.closed.unwrap_or(false) {
                EventStatus::Completed
            } else {
                EventStatus::Live
            },
            discovered_at: Utc::now(),
        })
    }

    /// Fetch crypto markets from Kalshi
    async fn fetch_kalshi_crypto_markets(&self) -> Result<Vec<CryptoMarket>> {
        // Kalshi uses different endpoints for different market types
        // We'll search for crypto-related markets
        let url = "https://api.elections.kalshi.com/trade-api/v2/markets?status=open&limit=200";

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch Kalshi markets")?;

        if !response.status().is_success() {
            return Err(anyhow!("Kalshi API error: {}", response.status()));
        }

        let resp: KalshiMarketsResponse = response.json().await?;
        info!("Kalshi returned {} total markets", resp.markets.len());

        // Log some sample titles to help debug crypto detection
        let sample_titles: Vec<&str> = resp.markets.iter().take(5).map(|m| m.title.as_str()).collect();
        info!("Kalshi sample titles: {:?}", sample_titles);

        // Filter for crypto markets
        let crypto_markets: Vec<CryptoMarket> = resp
            .markets
            .into_iter()
            .filter_map(|m| self.parse_kalshi_crypto(&m))
            .collect();

        if !crypto_markets.is_empty() {
            info!(
                "Found {} crypto markets on Kalshi: {:?}",
                crypto_markets.len(),
                crypto_markets.iter().map(|m| &m.title).collect::<Vec<_>>()
            );
        }

        Ok(crypto_markets)
    }

    /// Parse a Kalshi market into a CryptoMarket if it's crypto-related
    fn parse_kalshi_crypto(&self, market: &KalshiMarket) -> Option<CryptoMarket> {
        // Check if it's a crypto market using word-boundary matching
        let asset = TRACKED_ASSETS
            .iter()
            .find(|&asset| contains_crypto_asset(&market.title, asset))?;

        // Try to parse price target
        let (target_price, prediction_type) = self.parse_price_target(&market.title)?;

        // Parse end date
        let target_date = market
            .close_time
            .as_ref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::days(365));

        Some(CryptoMarket {
            market_id: format!("kalshi:{}", market.ticker),
            platform: "kalshi".to_string(),
            asset: asset.to_string(),
            target_price,
            target_date,
            prediction_type,
            title: market.title.clone(),
            description: market.subtitle.clone(),
            yes_price: market.yes_bid.map(|p| p as f64 / 100.0),
            no_price: market.no_bid.map(|p| p as f64 / 100.0),
            volume: market.volume.map(|v| v as f64),
            liquidity: market.open_interest.map(|o| o as f64),
            status: match market.status.as_str() {
                "open" => EventStatus::Live,
                "closed" => EventStatus::Completed,
                _ => EventStatus::Scheduled,
            },
            discovered_at: Utc::now(),
        })
    }

    /// Parse price target from market question/title
    fn parse_price_target(&self, text: &str) -> Option<(f64, CryptoPredictionType)> {
        let text_upper = text.to_uppercase();

        // Common patterns:
        // "Bitcoin above $100,000"
        // "BTC > $100k"
        // "ETH to hit $5,000"
        // "Will Bitcoin reach $150,000"

        // Extract price using regex-like matching
        let price = extract_price_from_text(text)?;

        // Determine prediction type
        let prediction_type = if text_upper.contains("ABOVE")
            || text_upper.contains(">")
            || text_upper.contains("REACH")
            || text_upper.contains("HIT")
            || text_upper.contains("EXCEED")
        {
            CryptoPredictionType::PriceTarget
        } else if text_upper.contains("BELOW") || text_upper.contains("<") {
            CryptoPredictionType::PriceTarget
        } else if text_upper.contains("ETF")
            || text_upper.contains("LAUNCH")
            || text_upper.contains("APPROVAL")
        {
            CryptoPredictionType::Event
        } else {
            CryptoPredictionType::PriceTarget
        };

        Some((price, prediction_type))
    }

    /// Enrich a market with live price data
    async fn enrich_with_price(&self, market: &CryptoMarket) -> Result<EventState> {
        // Get current price from CoinGecko
        let price = self.coingecko.get_price(&market.asset).await?;

        // Calculate volatility
        let volatility = self
            .coingecko
            .calculate_volatility(&market.asset, 30)
            .await
            .map(|v| v.daily_volatility)
            .unwrap_or(0.03); // Default 3% daily vol

        Ok(EventState {
            event_id: market.market_id.clone(),
            market_type: MarketType::Crypto {
                asset: market.asset.clone(),
                prediction_type: market.prediction_type,
            },
            entity_a: market.asset.clone(),
            entity_b: None,
            status: market.status,
            state: StateData::Crypto(CryptoStateData {
                current_price: price.current_price,
                target_price: market.target_price,
                target_date: market.target_date,
                volatility_24h: volatility,
                volume_24h: Some(price.total_volume),
                metadata: serde_json::json!({
                    "market_cap": price.market_cap,
                    "high_24h": price.high_24h,
                    "low_24h": price.low_24h,
                    "ath": price.ath,
                    "atl": price.atl,
                    "yes_price": market.yes_price,
                    "no_price": market.no_price,
                    "platform": market.platform,
                }),
            }),
            fetched_at: Utc::now(),
        })
    }
}

impl Default for CryptoEventProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventProvider for CryptoEventProvider {
    async fn get_live_events(&self) -> Result<Vec<EventInfo>> {
        // Refresh if needed
        if self.needs_refresh().await {
            self.refresh_markets().await?;
        }

        let cache = self.market_cache.read().await;

        let events: Vec<EventInfo> = cache
            .values()
            .filter(|m| m.status == EventStatus::Live)
            .map(|m| EventInfo {
                event_id: m.market_id.clone(),
                market_type: MarketType::Crypto {
                    asset: m.asset.clone(),
                    prediction_type: m.prediction_type,
                },
                entity_a: m.asset.clone(),
                entity_b: None,
                scheduled_time: m.target_date,
                status: m.status,
                venue: Some(m.platform.clone()),
                metadata: serde_json::json!({
                    "title": m.title,
                    "target_price": m.target_price,
                }),
            })
            .collect();

        Ok(events)
    }

    async fn get_scheduled_events(&self, days: u32) -> Result<Vec<EventInfo>> {
        // Refresh if needed
        if self.needs_refresh().await {
            self.refresh_markets().await?;
        }

        let cache = self.market_cache.read().await;
        let cutoff = Utc::now() + chrono::Duration::days(days as i64);

        let events: Vec<EventInfo> = cache
            .values()
            .filter(|m| m.target_date <= cutoff)
            .map(|m| EventInfo {
                event_id: m.market_id.clone(),
                market_type: MarketType::Crypto {
                    asset: m.asset.clone(),
                    prediction_type: m.prediction_type,
                },
                entity_a: m.asset.clone(),
                entity_b: None,
                scheduled_time: m.target_date,
                status: m.status,
                venue: Some(m.platform.clone()),
                metadata: serde_json::json!({
                    "title": m.title,
                    "target_price": m.target_price,
                }),
            })
            .collect();

        Ok(events)
    }

    async fn get_event_state(&self, event_id: &str) -> Result<EventState> {
        // Check if cache is empty and needs refresh
        {
            let cache = self.market_cache.read().await;
            if cache.is_empty() {
                drop(cache);
                info!("Market cache empty, refreshing...");
                if let Err(e) = self.refresh_markets().await {
                    warn!("Failed to refresh market cache: {}", e);
                }
            }
        }

        let cache = self.market_cache.read().await;

        let market = cache
            .get(event_id)
            .ok_or_else(|| anyhow!("Market not found: {}", event_id))?
            .clone();

        drop(cache); // Release lock before async call

        self.enrich_with_price(&market).await
    }

    fn provider_name(&self) -> &str {
        "CryptoEventProvider"
    }

    fn supported_market_types(&self) -> Vec<MarketType> {
        vec![
            MarketType::Crypto {
                asset: "BTC".to_string(),
                prediction_type: CryptoPredictionType::PriceTarget,
            },
            MarketType::Crypto {
                asset: "ETH".to_string(),
                prediction_type: CryptoPredictionType::PriceTarget,
            },
        ]
    }
}

/// Extract price from text like "$100,000" or "$100k" or "100000"
fn extract_price_from_text(text: &str) -> Option<f64> {
    // Remove common formatting and punctuation
    let cleaned = text
        .replace(',', "")
        .replace('$', "")
        .replace('?', "")
        .replace('.', " ")
        .replace('!', "")
        .to_uppercase();

    // Find number patterns
    for word in cleaned.split_whitespace() {
        // Clean trailing punctuation
        let word = word.trim_end_matches(|c: char| !c.is_alphanumeric());

        // Handle "100k", "100K", "100m", "100M"
        if word.ends_with('K') {
            if let Ok(num) = word[..word.len() - 1].parse::<f64>() {
                return Some(num * 1_000.0);
            }
        }
        if word.ends_with('M') {
            if let Ok(num) = word[..word.len() - 1].parse::<f64>() {
                return Some(num * 1_000_000.0);
            }
        }

        // Try direct number parsing
        if let Ok(num) = word.parse::<f64>() {
            if num > 100.0 {
                // Likely a price, not a percentage
                return Some(num);
            }
        }
    }

    None
}

/// Check if text contains asset as a whole word (not part of another word)
/// This prevents "Kenneth" from matching "ETH"
fn contains_crypto_asset(text: &str, asset: &str) -> bool {
    let text_upper = text.to_uppercase();
    let asset_upper = asset.to_uppercase();
    let full_name = asset_full_name(asset).to_uppercase();

    // Check for the asset symbol with word boundaries
    // Allow matches like "BTC", "$BTC", "BTC:", "BTC,", "(BTC)", etc.
    for word in text_upper.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')' || c == ':' || c == ';') {
        let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed == asset_upper || trimmed == format!("${}", asset_upper) {
            return true;
        }
    }

    // Check for full name (e.g., "BITCOIN", "ETHEREUM")
    text_upper.contains(&full_name)
}

/// Get full name for asset symbol
fn asset_full_name(symbol: &str) -> String {
    match symbol {
        "BTC" => "BITCOIN".to_string(),
        "ETH" => "ETHEREUM".to_string(),
        "SOL" => "SOLANA".to_string(),
        "XRP" => "RIPPLE".to_string(),
        "DOGE" => "DOGECOIN".to_string(),
        "ADA" => "CARDANO".to_string(),
        "AVAX" => "AVALANCHE".to_string(),
        "DOT" => "POLKADOT".to_string(),
        "MATIC" => "POLYGON".to_string(),
        "LINK" => "CHAINLINK".to_string(),
        _ => symbol.to_string(),
    }
}

// ============================================================================
// API Response Structs
// ============================================================================

/// Helper module to deserialize numbers that may come as strings
mod string_or_number {
    use serde::{self, Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNumber {
            String(String),
            Number(f64),
            Null,
        }

        match StringOrNumber::deserialize(deserializer)? {
            StringOrNumber::String(s) => Ok(s.parse().ok()),
            StringOrNumber::Number(n) => Ok(Some(n)),
            StringOrNumber::Null => Ok(None),
        }
    }

    /// Deserialize outcome_prices which can be:
    /// - A JSON array: [0.5, 0.5]
    /// - A JSON string containing an array: "[\"0.5\", \"0.5\"]"
    /// - null/missing
    pub fn deserialize_outcome_prices<'de, D>(deserializer: D) -> Result<Option<Vec<f64>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OutcomePrices {
            // String containing JSON array like "[\"0.0355\", \"0.9645\"]"
            JsonString(String),
            // Direct array of numbers
            NumberArray(Vec<f64>),
            // Direct array of strings
            StringArray(Vec<String>),
        }

        let opt: Option<OutcomePrices> = Option::deserialize(deserializer)?;
        Ok(opt.and_then(|prices| match prices {
            OutcomePrices::JsonString(s) => {
                // Try to parse as JSON array of strings
                if let Ok(arr) = serde_json::from_str::<Vec<String>>(&s) {
                    Some(arr.iter().filter_map(|v| v.parse().ok()).collect())
                } else if let Ok(arr) = serde_json::from_str::<Vec<f64>>(&s) {
                    Some(arr)
                } else {
                    None
                }
            }
            OutcomePrices::NumberArray(arr) => Some(arr),
            OutcomePrices::StringArray(arr) => {
                Some(arr.iter().filter_map(|v| v.parse().ok()).collect())
            }
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolymarketMarket {
    condition_id: String,
    question: String,
    description: Option<String>,
    end_date: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize_outcome_prices")]
    outcome_prices: Option<Vec<f64>>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    volume: Option<f64>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    liquidity: Option<f64>,
    closed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct KalshiMarketsResponse {
    markets: Vec<KalshiMarket>,
}

#[derive(Debug, Deserialize)]
struct KalshiMarket {
    ticker: String,
    title: String,
    subtitle: Option<String>,
    close_time: Option<String>,
    status: String,
    yes_bid: Option<i32>,
    no_bid: Option<i32>,
    volume: Option<i64>,
    open_interest: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_price() {
        assert_eq!(extract_price_from_text("Bitcoin above $100,000"), Some(100000.0));
        assert_eq!(extract_price_from_text("BTC > $100k"), Some(100000.0));
        assert_eq!(extract_price_from_text("ETH to hit 5000"), Some(5000.0));
        assert_eq!(extract_price_from_text("Will reach $1M"), Some(1000000.0));
    }

    #[test]
    fn test_asset_detection() {
        let provider = CryptoEventProvider::new();

        // Should detect price target
        let text = "Will Bitcoin reach $100,000?";
        let result = provider.parse_price_target(text);
        assert!(result.is_some(), "Should parse price target from: {}", text);
        let (price, _) = result.unwrap();
        assert_eq!(price, 100000.0);

        // Should detect k suffix
        let text2 = "BTC above $100k by December";
        let result2 = provider.parse_price_target(text2);
        assert!(result2.is_some(), "Should parse $100k");
        assert_eq!(result2.unwrap().0, 100000.0);
    }

    #[tokio::test]
    async fn test_provider_creation() {
        let provider = CryptoEventProvider::new();
        assert_eq!(provider.provider_name(), "CryptoEventProvider");
        assert!(!provider.supported_market_types().is_empty());
    }
}
