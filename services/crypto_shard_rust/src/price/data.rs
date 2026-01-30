//! Crypto price data structures

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;

/// Custom deserializer for timestamps that can be either:
/// - Unix milliseconds (integer)
/// - RFC 3339 formatted string
fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Deserialize as _};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TimestampValue {
        Millis(i64),
        String(String),
    }

    match TimestampValue::deserialize(deserializer)? {
        TimestampValue::Millis(ms) => {
            let secs = ms / 1000;
            let nsecs = ((ms % 1000) * 1_000_000) as u32;
            Ok(DateTime::<Utc>::from_timestamp(secs, nsecs).unwrap_or_else(|| Utc::now()))
        }
        TimestampValue::String(s) => {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(de::Error::custom)
        }
    }
}

/// Incoming price from price monitors (published via ZMQ)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingCryptoPrice {
    pub market_id: String,
    pub platform: String,       // "kalshi" | "polymarket"
    pub asset: Option<String>,  // "BTC", "ETH", etc. - may be null, extracted from market_id if so
    pub yes_bid: f64,           // Bid price (0-1)
    pub yes_ask: f64,           // Ask price (0-1)
    pub mid_price: Option<f64>, // Calculated if not provided
    pub yes_bid_size: Option<f64>, // Bid liquidity
    pub yes_ask_size: Option<f64>, // Ask liquidity
    pub liquidity: Option<f64>, // Total available liquidity
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub timestamp: DateTime<Utc>,
}

impl IncomingCryptoPrice {
    /// Extract asset from market_id if not explicitly provided
    pub fn resolve_asset(&self) -> String {
        if let Some(asset) = &self.asset {
            return asset.clone();
        }

        // Extract asset from market_id
        // Kalshi format: "KXBTC-..." or "KXETH..." → extract "BTC" or "ETH"
        // Polymarket format: condition ID (hex) → not extractable, use "UNKNOWN"
        let market_id = &self.market_id;

        if market_id.starts_with("KX") && market_id.len() > 2 {
            // Kalshi: KXBTC, KXETH, KXDOGE, etc.
            let rest = &market_id[2..]; // Skip "KX"
            // Extract asset name before first dash or special char
            if let Some(pos) = rest.find(|c: char| !c.is_alphabetic()) {
                rest[..pos].to_uppercase()
            } else {
                rest.to_uppercase()
            }
        } else if market_id.starts_with("INX") {
            // Kalshi intraday: INXBTC, INXETH, etc.
            let rest = &market_id[3..]; // Skip "INX"
            if let Some(pos) = rest.find(|c: char| !c.is_alphabetic()) {
                rest[..pos].to_uppercase()
            } else {
                rest.to_uppercase()
            }
        } else {
            // Polymarket or unknown - can't extract
            "UNKNOWN".to_string()
        }
    }
}

/// Processed crypto price data (stored in memory)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoPriceData {
    pub market_id: String,
    pub platform: String,
    pub asset: String,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub mid_price: f64,
    pub yes_bid_size: Option<f64>,
    pub yes_ask_size: Option<f64>,
    pub total_liquidity: Option<f64>,
    pub timestamp: DateTime<Utc>,
}

impl CryptoPriceData {
    /// Spread in basis points (bps)
    pub fn spread_bps(&self) -> f64 {
        ((self.yes_ask - self.yes_bid) / self.mid_price) * 10000.0
    }

    /// Check if price is stale (older than duration)
    pub fn is_stale(&self, max_age: Duration) -> bool {
        let age = Utc::now()
            .signed_duration_since(self.timestamp)
            .to_std()
            .unwrap_or(Duration::from_secs(u64::MAX));
        age > max_age
    }

    /// Get mid price explicitly
    pub fn get_mid(&self) -> f64 {
        self.mid_price
    }

    /// Calculate available size for a trade direction
    /// For long (buy YES), limited by ask side liquidity
    /// For short (buy NO), limited by bid side liquidity
    pub fn available_liquidity_for_direction(&self, is_long: bool) -> Option<f64> {
        if is_long {
            self.yes_ask_size
        } else {
            self.yes_bid_size
        }
    }
}

impl From<IncomingCryptoPrice> for CryptoPriceData {
    fn from(incoming: IncomingCryptoPrice) -> Self {
        let mid_price = incoming
            .mid_price
            .unwrap_or_else(|| (incoming.yes_bid + incoming.yes_ask) / 2.0);

        let asset = incoming.resolve_asset(); // Extract from market_id if null

        Self {
            market_id: incoming.market_id,
            platform: incoming.platform,
            asset,
            yes_bid: incoming.yes_bid,
            yes_ask: incoming.yes_ask,
            mid_price,
            yes_bid_size: incoming.yes_bid_size,
            yes_ask_size: incoming.yes_ask_size,
            total_liquidity: incoming.liquidity,
            timestamp: incoming.timestamp,
        }
    }
}

/// Cache key for storing prices: "asset|platform"
pub fn cache_key(asset: &str, platform: &str) -> String {
    format!("{}|{}", asset, platform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_incoming_to_crypto_price_conversion() {
        let incoming = IncomingCryptoPrice {
            market_id: "btc_100k".to_string(),
            platform: "kalshi".to_string(),
            asset: Some("BTC".to_string()),
            yes_bid: 0.45,
            yes_ask: 0.47,
            mid_price: None,
            yes_bid_size: Some(1000.0),
            yes_ask_size: Some(1500.0),
            liquidity: Some(2500.0),
            timestamp: Utc::now(),
        };

        let price: CryptoPriceData = incoming.into();
        assert_eq!(price.asset, "BTC");
        assert_eq!(price.yes_bid, 0.45);
        assert_eq!(price.yes_ask, 0.47);
        assert!((price.mid_price - 0.46).abs() < 0.0001);
    }

    #[test]
    fn test_mid_price_from_incoming() {
        let incoming = IncomingCryptoPrice {
            market_id: "eth".to_string(),
            platform: "polymarket".to_string(),
            asset: Some("ETH".to_string()),
            yes_bid: 0.30,
            yes_ask: 0.32,
            mid_price: Some(0.315),
            yes_bid_size: None,
            yes_ask_size: None,
            liquidity: None,
            timestamp: Utc::now(),
        };

        let price: CryptoPriceData = incoming.into();
        assert_eq!(price.mid_price, 0.315);
    }

    #[test]
    fn test_resolve_asset_from_kalshi_market_id() {
        // Test Kalshi market format (KXBTC, KXETH, etc.)
        let incoming = IncomingCryptoPrice {
            market_id: "KXBTC-26JAN3017-T94249.99".to_string(),
            platform: "kalshi".to_string(),
            asset: None, // Asset is null, should be extracted
            yes_bid: 0.45,
            yes_ask: 0.47,
            mid_price: None,
            yes_bid_size: None,
            yes_ask_size: None,
            liquidity: None,
            timestamp: Utc::now(),
        };

        assert_eq!(incoming.resolve_asset(), "BTC");
    }

    #[test]
    fn test_resolve_asset_from_intraday_market_id() {
        // Test Kalshi intraday format (INXBTC, INXETH, etc.)
        let incoming = IncomingCryptoPrice {
            market_id: "INXBTC-27JAN01-6000.00".to_string(),
            platform: "kalshi".to_string(),
            asset: None, // Asset is null, should be extracted
            yes_bid: 0.45,
            yes_ask: 0.47,
            mid_price: None,
            yes_bid_size: None,
            yes_ask_size: None,
            liquidity: None,
            timestamp: Utc::now(),
        };

        assert_eq!(incoming.resolve_asset(), "BTC");
    }

    #[test]
    fn test_spread_bps_calculation() {
        let price = CryptoPriceData {
            market_id: "btc".to_string(),
            platform: "kalshi".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.50,
            yes_ask: 0.51,
            mid_price: 0.505,
            yes_bid_size: None,
            yes_ask_size: None,
            total_liquidity: None,
            timestamp: Utc::now(),
        };

        let spread = price.spread_bps();
        assert!(spread > 0.0);
        assert!(spread < 500.0); // 1% spread max
    }

    #[test]
    fn test_is_stale() {
        let now = Utc::now();
        let fresh = CryptoPriceData {
            market_id: "btc".to_string(),
            platform: "kalshi".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.50,
            yes_ask: 0.51,
            mid_price: 0.505,
            yes_bid_size: None,
            yes_ask_size: None,
            total_liquidity: None,
            timestamp: now,
        };

        assert!(!fresh.is_stale(Duration::from_secs(60)));

        let old = CryptoPriceData {
            timestamp: now - chrono::Duration::seconds(120),
            ..fresh.clone()
        };

        assert!(old.is_stale(Duration::from_secs(60)));
    }

    #[test]
    fn test_available_liquidity_for_direction() {
        let price = CryptoPriceData {
            market_id: "btc".to_string(),
            platform: "kalshi".to_string(),
            asset: "BTC".to_string(),
            yes_bid: 0.50,
            yes_ask: 0.51,
            mid_price: 0.505,
            yes_bid_size: Some(1000.0),
            yes_ask_size: Some(2000.0),
            total_liquidity: Some(3000.0),
            timestamp: Utc::now(),
        };

        // Long (buy YES) limited by ask side
        assert_eq!(price.available_liquidity_for_direction(true), Some(2000.0));

        // Short (buy NO) limited by bid side
        assert_eq!(price.available_liquidity_for_direction(false), Some(1000.0));
    }

    #[test]
    fn test_cache_key_generation() {
        assert_eq!(cache_key("BTC", "kalshi"), "BTC|kalshi");
        assert_eq!(cache_key("ETH", "polymarket"), "ETH|polymarket");
    }
}
