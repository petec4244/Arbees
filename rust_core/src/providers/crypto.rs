//! Crypto Event Provider
//!
//! Discovers and tracks cryptocurrency prediction markets from Kalshi and Polymarket.
//! Combines market discovery with live price data from multiple sources
//! (Coinbase → Binance → CoinGecko fallback chain).

use super::{CryptoStateData, EventInfo, EventProvider, EventState, EventStatus, StateData};
use crate::clients::chained_price::ChainedPriceProvider;
use crate::clients::crypto_price::CryptoPriceProvider;
use crate::models::{CryptoPredictionType, MarketType};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tracing::{debug, info, warn};

/// Crypto event provider
///
/// Discovers crypto prediction markets and enriches them with live price data.
/// Uses ChainedPriceProvider for resilient price fetching (Coinbase → Binance → CoinGecko).
pub struct CryptoEventProvider {
    /// HTTP client for API requests
    client: Client,
    /// Price provider with fallback chain
    price_provider: Arc<dyn CryptoPriceProvider>,
    /// Cache for discovered markets
    market_cache: Arc<RwLock<HashMap<String, CryptoMarket>>>,
    /// Last cache update time
    last_update: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Cache TTL in seconds
    cache_ttl_secs: i64,
    /// Mutex to prevent concurrent cache refreshes (prevents stack overflow)
    refresh_lock: Arc<Mutex<()>>,
    /// Semaphore to limit concurrent get_event_state calls (prevents stack overflow)
    state_semaphore: Arc<Semaphore>,
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
    /// Create a new crypto event provider with default chained price provider
    /// (Coinbase → Binance → CoinGecko fallback)
    pub fn new() -> Self {
        Self::with_price_provider(Arc::new(ChainedPriceProvider::new_default()))
    }

    /// Create with custom price provider
    pub fn with_price_provider(price_provider: Arc<dyn CryptoPriceProvider>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            price_provider,
            market_cache: Arc::new(RwLock::new(HashMap::new())),
            last_update: Arc::new(RwLock::new(None)),
            cache_ttl_secs: 300, // 5 minute cache
            refresh_lock: Arc::new(Mutex::new(())),
            // Limit concurrent get_event_state calls to prevent stack overflow
            // when many events are monitored simultaneously
            state_semaphore: Arc::new(Semaphore::new(3)),
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
    ///
    /// Uses two approaches:
    /// 1. Text search for price-target markets (e.g., "will BTC reach $100k")
    /// 2. Direct slug lookup for directional markets (e.g., "btc-updown-15m")
    async fn fetch_polymarket_crypto_markets(&self) -> Result<Vec<CryptoMarket>> {
        // Search terms for crypto markets - use full names for better API-side filtering
        const CRYPTO_SEARCH_TERMS: &[&str] = &[
            "bitcoin",
            "ethereum",
            "solana",
            "dogecoin",
            "cardano",
            "ripple",
            "XRP",
            "crypto",
        ];

        // Known recurring directional market series (15-minute, hourly, 4-hour)
        // These are high-velocity binary markets perfect for crypto arbitrage
        // Discovered by examining: https://github.com/peterpeterparker/Polymarket-Kalshi-Arbitrage-bot
        const DIRECTIONAL_SERIES: &[&str] = &[
            // 15-minute markets (highest velocity)
            "btc-updown-15m",
            "eth-updown-15m",
            "sol-updown-15m",
            "xrp-updown-15m",
            // Hourly markets
            "btc-up-or-down-hourly",
            "eth-up-or-down-hourly",
            "solana-up-or-down-hourly",
            "xrp-up-or-down-hourly",
            // 4-hour markets (optional - slower)
            "btc-updown-4h",
            "eth-updown-4h",
            "sol-updown-4h",
            "xrp-updown-4h",
        ];

        let mut all_markets: HashMap<String, CryptoMarket> = HashMap::new();

        // First, search for price-target markets using text search
        for search_term in CRYPTO_SEARCH_TERMS {
            match self.fetch_polymarket_events_for_query(search_term).await {
                Ok(markets) => {
                    for market in markets {
                        // Dedupe by market_id
                        all_markets.entry(market.market_id.clone()).or_insert(market);
                    }
                }
                Err(e) => {
                    warn!("Failed to search Polymarket for '{}': {}", search_term, e);
                }
            }
        }

        // Then, fetch directional markets using slug-based lookup
        // (these don't appear in text search results - must use /markets?slug= endpoint)
        let mut directional_found = 0;
        for series in DIRECTIONAL_SERIES {
            match self.fetch_polymarket_directional_market(series).await {
                Ok(market) => {
                    directional_found += 1;
                    // Dedupe by market_id
                    all_markets.entry(market.market_id.clone()).or_insert(market);
                }
                Err(_e) => {
                    // Directional markets not found - they may not exist on Polymarket
                    // or may have different naming conventions
                }
            }
        }

        if directional_found == 0 {
            debug!(
                "No Polymarket directional markets found via slug lookup (tried {} series)",
                DIRECTIONAL_SERIES.len()
            );
        } else {
            info!("Found {} directional (Up/Down) markets on Polymarket", directional_found);
        }

        let crypto_markets: Vec<CryptoMarket> = all_markets.into_values().collect();

        if !crypto_markets.is_empty() {
            info!(
                "Found {} crypto markets on Polymarket: {:?}",
                crypto_markets.len(),
                crypto_markets.iter().map(|m| &m.title).collect::<Vec<_>>()
            );
        }

        Ok(crypto_markets)
    }

    /// Fetch a specific directional market from Polymarket using slug-based lookup
    ///
    /// Directional markets (e.g., "btc-updown-15m") use the /markets?slug= endpoint
    /// for direct lookup, not the text search /events endpoint.
    ///
    /// Returns the first matching market for the given slug.
    async fn fetch_polymarket_directional_market(&self, slug: &str) -> Result<CryptoMarket> {
        const GAMMA_API_BASE: &str = "https://gamma-api.polymarket.com";

        let url = format!("{}/markets?slug={}", GAMMA_API_BASE, slug);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Polymarket directional market")?;

        if !response.status().is_success() {
            return Err(anyhow!("Polymarket API error for slug {}: {}", slug, response.status()));
        }

        let text = response.text().await?;
        let markets: Vec<PolymarketMarketSimple> = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                let snippet: String = text.chars().take(500).collect();
                warn!("Polymarket directional market JSON parse error: {}. Snippet: {}", e, snippet);
                return Err(anyhow!("JSON parse error: {}", e));
            }
        };

        // Find the first active/non-closed market from the results
        for market in markets {
            if market.closed.unwrap_or(false) {
                continue;
            }

            // Parse as crypto market
            if let Some(crypto_market) = self.parse_polymarket_market_simple(&market) {
                debug!("Found directional market for slug '{}': {}", slug, crypto_market.title);
                return Ok(crypto_market);
            }
        }

        Err(anyhow!("No valid crypto markets found for slug '{}'", slug))
    }

    /// Fetch events from Polymarket using /events endpoint with text search and pagination
    async fn fetch_polymarket_events_for_query(&self, query: &str) -> Result<Vec<CryptoMarket>> {
        const PAGE_SIZE: u32 = 100;
        const MAX_PAGES: u32 = 5; // Limit to 500 events per query

        let mut all_markets = Vec::new();
        let mut offset = 0u32;

        for _page in 0..MAX_PAGES {
            let url = format!(
                "https://gamma-api.polymarket.com/events?active=true&closed=false&limit={}&offset={}&_q={}",
                PAGE_SIZE, offset, query
            );

            let response = self
                .client
                .get(&url)
                .send()
                .await
                .context("Failed to fetch Polymarket events")?;

            if !response.status().is_success() {
                return Err(anyhow!("Polymarket API error: {}", response.status()));
            }

            let text = response.text().await?;
            let events: Vec<PolymarketEvent> = match serde_json::from_str(&text) {
                Ok(e) => e,
                Err(e) => {
                    let snippet: String = text.chars().take(500).collect();
                    warn!("Polymarket events JSON parse error: {}. Snippet: {}", e, snippet);
                    return Err(anyhow!("JSON parse error: {}", e));
                }
            };

            let event_count = events.len();
            debug!("Polymarket query '{}' offset {} returned {} events", query, offset, event_count);

            // Extract markets from events
            for event in events {
                for market in event.markets.into_iter().flatten() {
                    // Skip closed markets
                    if market.closed.unwrap_or(false) {
                        continue;
                    }

                    // Try to parse as crypto market
                    if let Some(crypto_market) = self.parse_polymarket_market(&market, &event.title) {
                        all_markets.push(crypto_market);
                    }
                }
            }

            // Stop if we got fewer than a full page (no more results)
            if event_count < PAGE_SIZE as usize {
                break;
            }

            offset += PAGE_SIZE;
        }

        Ok(all_markets)
    }

    /// Parse a Polymarket market from the /events response into a CryptoMarket
    fn parse_polymarket_market(&self, market: &PolymarketMarketNested, event_title: &str) -> Option<CryptoMarket> {
        // Use the market question, falling back to event title
        let question = if market.question.is_empty() {
            event_title.to_string()
        } else {
            market.question.clone()
        };

        // Check if it's a crypto market using word-boundary matching
        let asset = TRACKED_ASSETS
            .iter()
            .find(|&asset| contains_crypto_asset(&question, asset))?;

        // Check if this is a directional market (Up or Down) without explicit price target
        let is_directional = is_polymarket_directional(&question);

        // Try to parse price target from question (for price-target markets)
        // For directional markets, use a default target price (50% midpoint)
        let (target_price, prediction_type) = if is_directional {
            // Directional markets don't have explicit price targets
            // Use a placeholder (0.0) and mark as directional
            (10000.0, CryptoPredictionType::PriceTarget) // Placeholder - actual comparison is binary Up/Down
        } else {
            self.parse_price_target(&question)?
        };

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
            title: question,
            description: market.description.clone(),
            yes_price: market.outcome_prices.as_ref().and_then(|p| p.first().copied()),
            no_price: market.outcome_prices.as_ref().and_then(|p| p.get(1).copied()),
            volume: market.volume,
            liquidity: market.liquidity,
            status: if market.closed.unwrap_or(false) {
                EventStatus::Completed
            } else {
                EventStatus::Live
            },
            discovered_at: Utc::now(),
        })
    }

    /// Parse a Polymarket market from the /markets?slug= endpoint (directional markets)
    ///
    /// Similar to parse_polymarket_market() but uses PolymarketMarketSimple struct.
    /// Used for directional markets (Up/Down binary markets).
    fn parse_polymarket_market_simple(&self, market: &PolymarketMarketSimple) -> Option<CryptoMarket> {
        let question = market.question.clone();

        // Check if it's a crypto market using word-boundary matching
        let asset = TRACKED_ASSETS
            .iter()
            .find(|&asset| contains_crypto_asset(&question, asset))?;

        // Check if this is a directional market (Up or Down) without explicit price target
        let is_directional = is_polymarket_directional(&question);

        // Try to parse price target from question (for price-target markets)
        // For directional markets, use a default target price (50% midpoint)
        let (target_price, prediction_type) = if is_directional {
            // Directional markets don't have explicit price targets
            // Use a placeholder (0.0) and mark as directional
            (10000.0, CryptoPredictionType::PriceTarget) // Placeholder - actual comparison is binary Up/Down
        } else {
            self.parse_price_target(&question)?
        };

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
            title: question,
            description: market.description.clone(),
            yes_price: market.outcome_prices.as_ref().and_then(|p| p.first().copied()),
            no_price: market.outcome_prices.as_ref().and_then(|p| p.get(1).copied()),
            volume: market.volume,
            liquidity: market.liquidity,
            status: if market.closed.unwrap_or(false) {
                EventStatus::Completed
            } else {
                EventStatus::Live
            },
            discovered_at: Utc::now(),
        })
    }

    /// Fetch crypto markets from Kalshi API
    ///
    /// Uses api.elections.kalshi.com (Kalshi consolidated to a single API).
    /// Searches multiple crypto series tickers for short-term price prediction markets.
    async fn fetch_kalshi_crypto_markets(&self) -> Result<Vec<CryptoMarket>> {
        // Kalshi API base URL (consolidated - includes crypto, elections, etc.)
        const KALSHI_API: &str = "https://api.elections.kalshi.com/trade-api/v2";

        // Crypto series tickers to search - from kalshi.com/category/crypto/btc
        // These include short-term (daily) and longer-term price prediction markets
        const CRYPTO_SERIES: &[&str] = &[
            // Bitcoin short-term/daily markets (high frequency)
            "KXBTC",       // Bitcoin price range today at 12pm EST
            "KXBTCD",      // Bitcoin price today at 11am EST
            "KXBTCW",      // Bitcoin weekly

            // Bitcoin milestone/target markets
            "KXBTCMAXY",   // How high will Bitcoin get this year (yearly max)
            "KXBTCMINY",   // How low will Bitcoin fall this year (yearly min)
            "KXBTCMAX150", // When will Bitcoin hit $150k
            "KXBTC2025100",// Will Bitcoin cross $100k again this year
            // Ethereum markets (likely similar naming)
            "KXETH",       // Ethereum price range
            "KXETHD",      // Ethereum daily
            "KXETHMAXY",   // Ethereum yearly max
            // Other crypto
            "KXSOL",       // Solana
            "KXDOGE",      // Dogecoin

            "INXBTC",      // Intraday Bitcoin (15-min markets)
            "INXETH",      // Intraday Ethereum
            "BTCPRICE",    // Alternative Bitcoin ticker
            "ETHPRICE",    // Alternative Ethereum ticker
        ];

        let mut all_markets: HashMap<String, CryptoMarket> = HashMap::new();

        // Search each crypto series
        for series in CRYPTO_SERIES {
            match self.fetch_kalshi_series(KALSHI_API, series).await {
                Ok(markets) => {
                    for market in markets {
                        all_markets.entry(market.market_id.clone()).or_insert(market);
                    }
                }
                Err(e) => {
                    debug!("Kalshi series {} not found or error: {}", series, e);
                }
            }
        }

        // Also try a general search for any open crypto-related markets
        match self.fetch_kalshi_general_crypto(KALSHI_API).await {
            Ok(markets) => {
                for market in markets {
                    all_markets.entry(market.market_id.clone()).or_insert(market);
                }
            }
            Err(e) => {
                debug!("Kalshi general crypto search failed: {}", e);
            }
        }

        let crypto_markets: Vec<CryptoMarket> = all_markets.into_values().collect();

        if !crypto_markets.is_empty() {
            info!(
                "Found {} crypto markets on Kalshi: {:?}",
                crypto_markets.len(),
                crypto_markets.iter().map(|m| &m.title).collect::<Vec<_>>()
            );
        } else {
            info!("Found 0 crypto markets on Kalshi");
        }

        Ok(crypto_markets)
    }

    /// Fetch markets from a specific Kalshi series
    async fn fetch_kalshi_series(&self, base_url: &str, series_ticker: &str) -> Result<Vec<CryptoMarket>> {
        let url = format!(
            "{}/markets?series_ticker={}&status=open&limit=100",
            base_url, series_ticker
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Kalshi series")?;

        if !response.status().is_success() {
            return Err(anyhow!("Kalshi API error for series {}: {}", series_ticker, response.status()));
        }

        let resp: KalshiMarketsResponse = response.json().await?;

        if !resp.markets.is_empty() {
            debug!("Kalshi series {} returned {} markets", series_ticker, resp.markets.len());
        }

        let crypto_markets: Vec<CryptoMarket> = resp
            .markets
            .into_iter()
            .filter_map(|m| self.parse_kalshi_crypto(&m))
            .collect();

        Ok(crypto_markets)
    }

    /// Fetch general crypto markets from Kalshi by scanning all open markets
    async fn fetch_kalshi_general_crypto(&self, base_url: &str) -> Result<Vec<CryptoMarket>> {
        // Fetch open markets and filter locally for crypto
        let url = format!("{}/markets?status=open&limit=500", base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch Kalshi markets")?;

        if !response.status().is_success() {
            return Err(anyhow!("Kalshi API error: {}", response.status()));
        }

        let resp: KalshiMarketsResponse = response.json().await?;
        debug!("Kalshi general search returned {} total markets", resp.markets.len());

        // Filter for crypto markets by scanning titles
        let crypto_markets: Vec<CryptoMarket> = resp
            .markets
            .into_iter()
            .filter_map(|m| self.parse_kalshi_crypto(&m))
            .collect();

        Ok(crypto_markets)
    }

    /// Parse a Kalshi market into a CryptoMarket if it's crypto-related
    fn parse_kalshi_crypto(&self, market: &KalshiMarket) -> Option<CryptoMarket> {
        // FILTER: Only accept 15-minute intraday markets (INXBTC, INXETH)
        // We focus on the fastest markets for highest velocity arbitrage
        if !is_intraday_market(&market.ticker) {
            debug!("Filtered non-intraday market: {} ({})", market.ticker, market.title);
            return None;
        }

        // Check if it's a crypto market using word-boundary matching
        let asset = TRACKED_ASSETS
            .iter()
            .find(|&asset| contains_crypto_asset(&market.title, asset))?;

        // Extract target price from floor_strike/cap_strike fields (more reliable than parsing title)
        // - "greater" type: target is floor_strike (price must be above this)
        // - "less" type: target is cap_strike (price must be below this)
        // - "between" type: use floor_strike as the lower bound
        let target_price = match market.strike_type.as_deref() {
            Some("greater") => market.floor_strike,
            Some("less") => market.cap_strike,
            Some("between") => market.floor_strike, // Use lower bound for between ranges
            _ => None
        }
        .or_else(|| {
            // Fallback 1: Try to extract from Kalshi ticker format (e.g., "KXDOGE-26JAN3017-B0.227")
            extract_price_from_kalshi_ticker(&market.ticker)
        })
        .or_else(|| {
            // Fallback 2: Parse from title if all else fails
            self.parse_price_target(&market.title).map(|(p, _)| p)
        })?;

        // Determine prediction type from strike_type
        let prediction_type = match market.strike_type.as_deref() {
            Some("greater") | Some("less") | Some("between") => CryptoPredictionType::PriceTarget,
            _ => self
                .parse_price_target(&market.title)
                .map(|(_, t)| t)
                .unwrap_or(CryptoPredictionType::PriceTarget),
        };

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
            // Use yes_ask for yes_price (what you'd pay to buy YES)
            yes_price: market.yes_ask.map(|p| p as f64 / 100.0),
            // Use no_ask for no_price (what you'd pay to buy NO)
            no_price: market.no_ask.map(|p| p as f64 / 100.0),
            volume: market.volume.map(|v| v as f64),
            liquidity: market.open_interest.map(|o| o as f64),
            // Kalshi uses "active" for open/tradeable markets
            status: match market.status.as_str() {
                "active" | "open" => EventStatus::Live,
                "closed" | "settled" => EventStatus::Completed,
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
    ///
    /// Returns a boxed future to prevent stack overflow in deeply nested async call chains.
    /// Uses tokio::spawn to run price and volatility fetches in parallel, which also
    /// breaks the call chain depth by executing on separate tasks.
    fn enrich_with_price<'a>(
        &'a self,
        market: &'a CryptoMarket,
    ) -> Pin<Box<dyn Future<Output = Result<EventState>> + Send + 'a>> {
        Box::pin(async move {
            // Spawn price and volatility fetches in parallel on separate tasks.
            // This breaks the call chain depth and runs both operations concurrently.
            let price_provider = self.price_provider.clone();
            let asset = market.asset.clone();

            let price_handle = tokio::spawn(async move { price_provider.get_price(&asset).await });

            let price_provider2 = self.price_provider.clone();
            let asset2 = market.asset.clone();

            let volatility_handle = tokio::spawn(async move {
                price_provider2.calculate_volatility(&asset2, 30).await
            });

            // Await both in parallel
            let price = price_handle
                .await
                .context("Price fetch task panicked")??;

            let volatility = volatility_handle
                .await
                .context("Volatility fetch task panicked")?
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
                    volume_24h: Some(price.volume_24h),
                    metadata: serde_json::json!({
                        "market_cap": price.market_cap,
                        "high_24h": price.high_24h,
                        "low_24h": price.low_24h,
                        "price_source": price.source,
                        "yes_price": market.yes_price,
                        "no_price": market.no_price,
                        "platform": market.platform,
                    }),
                }),
                fetched_at: Utc::now(),
            })
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
            .map(|m| {
                // Extract raw condition_id/ticker from market_id (format: "platform:id")
                let raw_id = m.market_id.split(':').nth(1).unwrap_or(&m.market_id);

                EventInfo {
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
                        // Include platform-specific IDs for monitor assignment
                        "polymarket_condition_id": if m.platform == "polymarket" { Some(raw_id) } else { None },
                        "kalshi_ticker": if m.platform == "kalshi" { Some(raw_id) } else { None },
                    }),
                }
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
            .map(|m| {
                // Extract raw condition_id/ticker from market_id (format: "platform:id")
                let raw_id = m.market_id.split(':').nth(1).unwrap_or(&m.market_id);

                EventInfo {
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
                        // Include platform-specific IDs for monitor assignment
                        "polymarket_condition_id": if m.platform == "polymarket" { Some(raw_id) } else { None },
                        "kalshi_ticker": if m.platform == "kalshi" { Some(raw_id) } else { None },
                    }),
                }
            })
            .collect();

        Ok(events)
    }

    async fn get_event_state(&self, event_id: &str) -> Result<EventState> {
        // Acquire semaphore to limit concurrent calls (prevents stack overflow)
        let _permit = self.state_semaphore.acquire().await
            .map_err(|e| anyhow!("Failed to acquire state semaphore: {}", e))?;

        // Check if cache is empty and needs refresh
        // Use refresh_lock to prevent concurrent refreshes (which cause stack overflow)
        {
            let cache = self.market_cache.read().await;
            let needs_refresh = cache.is_empty();
            drop(cache);

            if needs_refresh {
                // Try to acquire the refresh lock - only one task does the refresh
                match self.refresh_lock.try_lock() {
                    Ok(_guard) => {
                        // Double-check cache is still empty (another task may have filled it)
                        let cache = self.market_cache.read().await;
                        if cache.is_empty() {
                            drop(cache);
                            debug!("Market cache empty, refreshing (holding lock)...");
                            if let Err(e) = self.refresh_markets().await {
                                warn!("Failed to refresh market cache: {}", e);
                            }
                        }
                        // _guard drops here, releasing the lock
                    }
                    Err(_) => {
                        // Another task is refreshing, wait for it
                        debug!("Waiting for another task to refresh cache...");
                        let _guard = self.refresh_lock.lock().await;
                        // Cache should be populated now
                    }
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

/// Extract price from Kalshi ticker format like "KXDOGE-26JAN3017-B0.227"
/// Format: {ticker}-{date}-{strike_type}{price}
/// Returns the price value (e.g., 0.227 from "B0.227")
fn extract_price_from_kalshi_ticker(ticker: &str) -> Option<f64> {
    // Format: "KXDOGE-26JAN3017-B0.227"
    // Split by hyphen: ["KXDOGE", "26JAN3017", "B0.227"]
    let parts: Vec<&str> = ticker.split('-').collect();
    if parts.len() < 3 {
        return None;
    }

    let strike_part = parts[2]; // "B0.227"

    // The strike part starts with a letter (B, T, L, etc.) and is followed by the price
    // Extract everything after the first character (which is the strike type)
    if strike_part.len() > 1 {
        let price_str = &strike_part[1..]; // "0.227"
        if let Ok(price) = price_str.parse::<f64>() {
            return Some(price);
        }
    }

    None
}

/// Check if a Kalshi market is a 15-minute intraday market.
/// We ONLY trade these fast markets for crypto arbitrage.
/// Intraday markets expire every 15 minutes, providing high-velocity arbitrage opportunities.
///
/// Configuration:
/// - CRYPTO_ALLOW_ALL_TIMEFRAMES=false (default): Only INXBTC/INXETH
/// - CRYPTO_ALLOW_ALL_TIMEFRAMES=true: Include all Kalshi crypto series
fn is_intraday_market(ticker: &str) -> bool {
    // Check if we're allowing all timeframes
    let allow_all = std::env::var("CRYPTO_ALLOW_ALL_TIMEFRAMES")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    if allow_all {
        // If enabled, accept all crypto markets (no filtering)
        return true;
    }

    // Default: Only 15-minute intraday markets
    ticker.starts_with("INXBTC") || ticker.starts_with("INXETH")
}

/// Check if a Polymarket market is a directional (Up/Down) market.
/// Directional markets are simple binary outcome markets predicting price movement direction.
/// Examples:
/// - "Bitcoin Up or Down 15m" (15-minute price direction)
/// - "Ethereum Up or Down Hourly" (1-hour price direction)
/// - "SOL Up or Down 4h" (4-hour price direction)
///
/// These markets have high velocity and real liquidity, perfect for crypto arbitrage.
fn is_polymarket_directional(title: &str) -> bool {
    let t = title.to_lowercase();

    // Check for directional market patterns
    let has_direction = t.contains("up or down") ||
                       t.contains("up/down") ||
                       t.contains("updown");

    // Check for timeframe indicators (15m, hourly, 4h, etc.)
    let has_timeframe = t.contains("15m") ||
                       t.contains("15-min") ||
                       t.contains("15 min") ||
                       t.contains("hourly") ||
                       t.contains("hour") ||
                       t.contains("4h") ||
                       t.contains("4-hour") ||
                       t.contains("4 hour");

    has_direction && has_timeframe
}

/// Check if text contains asset as a whole word (not part of another word)
/// This prevents "Kenneth" from matching "ETH"
fn contains_crypto_asset(text: &str, asset: &str) -> bool {
    let text_upper = text.to_uppercase();
    let asset_upper = asset.to_uppercase();
    let full_name = asset_full_name(asset).to_uppercase();

    // Special handling for DOGE - filter out Department of Government Efficiency markets
    // These are political markets about Elon Musk's cost-cutting initiative, not Dogecoin
    if asset_upper == "DOGE" && is_government_doge_market(&text_upper) {
        return false;
    }

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

/// Check if text is about Department of Government Efficiency (DOGE), not Dogecoin
/// Returns true if the text contains political/government keywords indicating it's not crypto
fn is_government_doge_market(text_upper: &str) -> bool {
    // Keywords that indicate Department of Government Efficiency, not Dogecoin
    const GOVT_DOGE_KEYWORDS: &[&str] = &[
        "FEDERAL SPENDING",
        "GOVERNMENT SPENDING",
        "FEDERAL BUDGET",
        "GOVERNMENT EFFICIENCY",
        "DEPT OF GOVERNMENT",
        "DEPARTMENT OF GOVERNMENT",
        "ELON AND DOGE",
        "MUSK AND DOGE",
        "DOGE CUT",
        "DOGE SAVE",
        "DOGE REDUCE",
        "DOGE SLASH",
        "BILLION IN SPENDING",
        "TRILLION IN SPENDING",
        "EXECUTIVE ORDER",
        "WHITE HOUSE",
        "TRUMP ADMIN",
        "VIVEK",
        "RAMASWAMY",
    ];

    for keyword in GOVT_DOGE_KEYWORDS {
        if text_upper.contains(keyword) {
            return true;
        }
    }

    false
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

/// Polymarket event from /events endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolymarketEvent {
    #[allow(dead_code)]
    id: String,
    title: String,
    #[serde(default)]
    markets: Option<Vec<PolymarketMarketNested>>,
}

/// Polymarket market nested inside an event
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolymarketMarketNested {
    condition_id: String,
    #[serde(default)]
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

/// Legacy: Polymarket market from /markets endpoint (kept for compatibility)
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

/// Polymarket market returned from /markets?slug= endpoint (directional markets)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolymarketMarketSimple {
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
    yes_ask: Option<i32>,
    no_ask: Option<i32>,
    volume: Option<i64>,
    open_interest: Option<i64>,
    /// Price floor for "greater than" or "between" markets
    floor_strike: Option<f64>,
    /// Price cap for "less than" or "between" markets
    cap_strike: Option<f64>,
    /// Type of strike: "greater", "less", "between"
    strike_type: Option<String>,
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

    #[test]
    fn test_intraday_filtering_kalshi() {
        // Should accept intraday markets (INXBTC, INXETH)
        assert!(is_intraday_market("INXBTC-30JAN25-1530"), "Should keep INXBTC");
        assert!(is_intraday_market("INXBTC"), "Should keep INXBTC prefix");
        assert!(is_intraday_market("INXETH-30JAN25-1545"), "Should keep INXETH");
        assert!(is_intraday_market("INXETH"), "Should keep INXETH prefix");

        // Should reject daily/weekly markets
        assert!(!is_intraday_market("KXBTC-30JAN25"), "Should filter KXBTC daily");
        assert!(!is_intraday_market("KXBTCD"), "Should filter KXBTCD daily");
        assert!(!is_intraday_market("KXBTCW-FEB25"), "Should filter weekly");
        assert!(!is_intraday_market("KXETH-30JAN25"), "Should filter daily ETH");
        assert!(!is_intraday_market("KXSOL"), "Should filter SOL daily");
        assert!(!is_intraday_market("KXDOGE"), "Should filter DOGE daily");
        assert!(!is_intraday_market("KXBTCMAXY"), "Should filter yearly max");
        assert!(!is_intraday_market("BTCPRICE"), "Should filter alternative ticker");
    }

    #[test]
    fn test_doge_government_filtering() {
        // Should NOT match - these are Department of Government Efficiency markets
        assert!(
            !contains_crypto_asset("Will Elon and DOGE cut less than $50b in federal spending in 2025?", "DOGE"),
            "Should filter out government DOGE markets"
        );
        assert!(
            !contains_crypto_asset("Will DOGE save $1 trillion in government spending?", "DOGE"),
            "Should filter out government spending DOGE markets"
        );
        assert!(
            !contains_crypto_asset("DOGE slash federal budget by 20%", "DOGE"),
            "Should filter out federal budget DOGE markets"
        );

        // SHOULD match - these are actual Dogecoin markets
        assert!(
            contains_crypto_asset("Will DOGE reach $1?", "DOGE"),
            "Should match Dogecoin price target"
        );
        assert!(
            contains_crypto_asset("Dogecoin above $0.50 by December", "DOGE"),
            "Should match Dogecoin by full name"
        );
        assert!(
            contains_crypto_asset("Will $DOGE hit $2?", "DOGE"),
            "Should match $DOGE ticker format"
        );
    }
}
