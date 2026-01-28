//! Market type taxonomy for multi-market arbitrage support
//!
//! Defines market type abstractions that generalize beyond sports to support
//! politics, economics, cryptocurrency, and entertainment markets.

use serde::{Deserialize, Serialize};

use super::Sport;

/// Universal market type discriminator
///
/// Wraps the existing Sport enum for backward compatibility while adding
/// support for non-sports markets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarketType {
    /// Sports markets (existing functionality)
    #[serde(rename = "sport")]
    Sport {
        sport: Sport,
    },

    /// Political/Election markets
    #[serde(rename = "politics")]
    Politics {
        region: String,
        event_type: PoliticsEventType,
    },

    /// Economic indicator markets
    #[serde(rename = "economics")]
    Economics {
        indicator: EconomicIndicator,
        threshold: Option<f64>,
    },

    /// Cryptocurrency prediction markets
    #[serde(rename = "crypto")]
    Crypto {
        asset: String,
        prediction_type: CryptoPredictionType,
    },

    /// Entertainment markets
    #[serde(rename = "entertainment")]
    Entertainment {
        category: String,
    },
}

impl MarketType {
    /// Create a sport market type (backward compatibility helper)
    pub fn sport(sport: Sport) -> Self {
        Self::Sport { sport }
    }

    /// Extract Sport if this is a sports market
    pub fn as_sport(&self) -> Option<Sport> {
        match self {
            Self::Sport { sport } => Some(*sport),
            _ => None,
        }
    }

    /// Check if this is a sports market
    pub fn is_sport(&self) -> bool {
        matches!(self, Self::Sport { .. })
    }

    /// Get human-readable market type name
    pub fn type_name(&self) -> &str {
        match self {
            Self::Sport { .. } => "sport",
            Self::Politics { .. } => "politics",
            Self::Economics { .. } => "economics",
            Self::Crypto { .. } => "crypto",
            Self::Entertainment { .. } => "entertainment",
        }
    }
}

/// Politics event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoliticsEventType {
    /// Elections (presidential, congressional, gubernatorial, etc.)
    Election,
    /// Confirmation votes (Supreme Court, Cabinet, etc.)
    Confirmation,
    /// Policy/legislation votes
    PolicyVote,
    /// Impeachment proceedings
    Impeachment,
    /// General political outcome
    Other,
}

/// Cryptocurrency prediction types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptoPredictionType {
    /// Price target prediction (e.g., "BTC > $100k by Dec 2025")
    PriceTarget,
    /// Protocol/network event (e.g., "ETH 2.0 launches Q1 2026")
    Event,
    /// Protocol metrics (e.g., "Uniswap TVL > $10B")
    Protocol,
    /// Token listing/delisting
    Listing,
    /// General crypto outcome
    Other,
}

/// Economic indicator types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicIndicator {
    /// Consumer Price Index (headline inflation)
    CPI,
    /// Core CPI (excluding food and energy)
    CoreCPI,
    /// Personal Consumption Expenditures (Fed's preferred measure)
    PCE,
    /// Core PCE (excluding food and energy)
    CorePCE,
    /// Unemployment Rate
    Unemployment,
    /// Nonfarm Payrolls (jobs report)
    NonfarmPayrolls,
    /// Federal Funds Rate
    FedFundsRate,
    /// Real GDP
    GDP,
    /// GDP Growth Rate (annualized)
    GDPGrowth,
    /// Initial Jobless Claims (weekly)
    JoblessClaims,
    /// Consumer Sentiment Index
    ConsumerSentiment,
    /// 10-Year Treasury Yield
    Treasury10Y,
    /// 2-Year Treasury Yield
    Treasury2Y,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_type_sport_serialization() {
        let market_type = MarketType::sport(Sport::NBA);
        let json = serde_json::to_string(&market_type).unwrap();
        assert!(json.contains("\"type\":\"sport\""));
        // Sport enum serializes as UPPERCASE (NBA, NFL, etc.)
        assert!(json.contains("\"sport\":\"NBA\""));

        let deserialized: MarketType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, market_type);
        assert_eq!(deserialized.as_sport(), Some(Sport::NBA));
    }

    #[test]
    fn test_market_type_politics_serialization() {
        let market_type = MarketType::Politics {
            region: "us".to_string(),
            event_type: PoliticsEventType::Election,
        };
        let json = serde_json::to_string(&market_type).unwrap();
        assert!(json.contains("\"type\":\"politics\""));
        assert!(json.contains("\"region\":\"us\""));
        assert!(json.contains("\"event_type\":\"election\""));

        let deserialized: MarketType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, market_type);
        assert!(!deserialized.is_sport());
    }

    #[test]
    fn test_market_type_crypto_serialization() {
        let market_type = MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        };
        let json = serde_json::to_string(&market_type).unwrap();
        assert!(json.contains("\"type\":\"crypto\""));
        assert!(json.contains("\"asset\":\"BTC\""));

        let deserialized: MarketType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, market_type);
    }

    #[test]
    fn test_market_type_economics() {
        let market_type = MarketType::Economics {
            indicator: EconomicIndicator::CPI,
            threshold: Some(3.0),
        };
        assert_eq!(market_type.type_name(), "economics");
        assert!(!market_type.is_sport());
        assert_eq!(market_type.as_sport(), None);
    }

    #[test]
    fn test_market_type_helpers() {
        let sport_market = MarketType::sport(Sport::NFL);
        assert!(sport_market.is_sport());
        assert_eq!(sport_market.type_name(), "sport");
        assert_eq!(sport_market.as_sport(), Some(Sport::NFL));

        let politics_market = MarketType::Politics {
            region: "uk".to_string(),
            event_type: PoliticsEventType::Election,
        };
        assert!(!politics_market.is_sport());
        assert_eq!(politics_market.type_name(), "politics");
        assert_eq!(politics_market.as_sport(), None);
    }
}
