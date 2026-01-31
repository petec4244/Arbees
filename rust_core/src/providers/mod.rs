//! Event provider abstractions for multi-market support
//!
//! Defines the EventProvider trait that allows pluggable event sources
//! (ESPN for sports, polling aggregators for politics, economic calendars, etc.)

use crate::models::MarketType;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Concrete provider implementations
pub mod crypto;
pub mod crypto_arbitrage;
pub mod economics;
pub mod espn;
pub mod politics;
pub mod registry;

// Re-export registry for convenient access
pub use registry::EventProviderRegistry;

/// Universal event provider trait
///
/// Implementations provide event data for different market types:
/// - Sports: ESPN API
/// - Politics: Polling aggregators (FiveThirtyEight, RealClearPolitics)
/// - Economics: Economic calendars (FRED, BLS)
/// - Crypto: Price feeds (CoinGecko, Binance)
#[async_trait]
pub trait EventProvider: Send + Sync {
    /// Get live events currently in progress
    async fn get_live_events(&self) -> Result<Vec<EventInfo>>;

    /// Get scheduled events in the next N days
    async fn get_scheduled_events(&self, days: u32) -> Result<Vec<EventInfo>>;

    /// Get detailed state for a specific event
    async fn get_event_state(&self, event_id: &str) -> Result<EventState>;

    /// Provider name for logging and debugging
    fn provider_name(&self) -> &str;

    /// Market types supported by this provider
    fn supported_market_types(&self) -> Vec<MarketType>;
}

/// Universal event information (lightweight, for discovery)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInfo {
    /// Unique event identifier
    pub event_id: String,

    /// Market type
    pub market_type: MarketType,

    /// Primary entity (home team, candidate, indicator, asset)
    pub entity_a: String,

    /// Secondary entity (away team, opponent, null for single-entity)
    pub entity_b: Option<String>,

    /// Scheduled start time
    pub scheduled_time: DateTime<Utc>,

    /// Current event status
    pub status: EventStatus,

    /// Optional venue information
    pub venue: Option<String>,

    /// Optional additional metadata
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Event status (universal across all market types)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// Event is scheduled but not started
    Scheduled,

    /// Event is currently in progress
    Live,

    /// Event has completed
    Completed,

    /// Event was cancelled
    Cancelled,

    /// Event was postponed
    Postponed,
}

/// Universal event state (detailed, for live tracking)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventState {
    /// Unique event identifier
    pub event_id: String,

    /// Market type
    pub market_type: MarketType,

    /// Primary entity
    pub entity_a: String,

    /// Secondary entity (optional)
    pub entity_b: Option<String>,

    /// Current event status
    pub status: EventStatus,

    /// Market-specific state data
    pub state: StateData,

    /// When this state was fetched
    pub fetched_at: DateTime<Utc>,
}

/// Market-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state_type", rename_all = "snake_case")]
pub enum StateData {
    /// Sports event state
    Sport(SportStateData),

    /// Politics event state
    Politics(PoliticsStateData),

    /// Economics event state
    Economics(EconomicsStateData),

    /// Crypto event state
    Crypto(CryptoStateData),

    /// Entertainment event state
    Entertainment(EntertainmentStateData),
}

/// Sports-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SportStateData {
    pub score_a: u16,
    pub score_b: u16,
    pub period: u8,
    pub time_remaining: u32,
    pub possession: Option<String>,
    pub sport_details: serde_json::Value,
}

/// Politics-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoliticsStateData {
    /// Current probability from polls/markets
    pub current_probability: Option<f64>,
    /// Last update timestamp
    pub last_updated: DateTime<Utc>,
    /// Number of polls in average
    pub poll_count: Option<u32>,
    /// Event date (election day, vote day)
    pub event_date: DateTime<Utc>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

/// Economics-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicsStateData {
    /// Current value (if released)
    pub current_value: Option<f64>,
    /// Consensus forecast
    pub forecast_value: Option<f64>,
    /// Release date
    pub release_date: DateTime<Utc>,
    /// Previous value
    pub previous_value: Option<f64>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

/// Crypto-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoStateData {
    /// Current price in USD
    pub current_price: f64,
    /// Target price for prediction
    pub target_price: f64,
    /// Target date
    pub target_date: DateTime<Utc>,
    /// 24h volatility
    pub volatility_24h: f64,
    /// 24h volume
    pub volume_24h: Option<f64>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

/// Entertainment-specific state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntertainmentStateData {
    /// Event description
    pub description: String,
    /// Event date
    pub event_date: DateTime<Utc>,
    /// Current probability if available
    pub current_probability: Option<f64>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_status_serialization() {
        let status = EventStatus::Live;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"live\"");

        let deserialized: EventStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, EventStatus::Live);
    }

    #[test]
    fn test_state_data_sport() {
        let state = StateData::Sport(SportStateData {
            score_a: 72,
            score_b: 68,
            period: 3,
            time_remaining: 420,
            possession: Some("home".to_string()),
            sport_details: serde_json::json!({}),
        });

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state_type\":\"sport\""));

        let deserialized: StateData = serde_json::from_str(&json).unwrap();
        match deserialized {
            StateData::Sport(s) => {
                assert_eq!(s.score_a, 72);
                assert_eq!(s.score_b, 68);
            }
            _ => panic!("Expected Sport state"),
        }
    }

    #[test]
    fn test_state_data_politics() {
        let state = StateData::Politics(PoliticsStateData {
            current_probability: Some(0.52),
            last_updated: Utc::now(),
            poll_count: Some(15),
            event_date: Utc::now(),
            metadata: serde_json::json!({"region": "us"}),
        });

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state_type\":\"politics\""));
    }
}
