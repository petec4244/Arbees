//! Politics Event Provider
//!
//! Provides political event data from prediction markets.
//! Focuses on elections, confirmations, and policy votes.

use super::{EventInfo, EventProvider, EventState, EventStatus, PoliticsStateData, StateData};
use crate::clients::kalshi::KalshiClient;
use crate::clients::polymarket::PolymarketClient;
use crate::models::{MarketType, PoliticsEventType};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde_json::json;
use std::sync::Arc;
use tracing::debug;

/// Politics event provider combining prediction market data
pub struct PoliticsEventProvider {
    /// Kalshi client for prediction markets
    kalshi: Option<Arc<KalshiClient>>,
    /// Polymarket client
    polymarket: Option<Arc<PolymarketClient>>,
}

impl PoliticsEventProvider {
    /// Create a new politics event provider
    pub fn new() -> Self {
        Self {
            kalshi: None,
            polymarket: None,
        }
    }

    /// Create with custom clients (for sharing/testing)
    pub fn with_clients(
        kalshi: Option<Arc<KalshiClient>>,
        polymarket: Option<Arc<PolymarketClient>>,
    ) -> Self {
        Self { kalshi, polymarket }
    }

    /// Detect political event type from market text
    pub fn detect_event_type(text: &str) -> Option<PoliticsEventType> {
        let text_lower = text.to_lowercase();

        // Check impeachment first (most specific)
        if text_lower.contains("impeach") {
            return Some(PoliticsEventType::Impeachment);
        }

        // Check confirmation (specific to nominations)
        if text_lower.contains("confirm")
            || text_lower.contains("nomination")
            || text_lower.contains("supreme court")
            || text_lower.contains("cabinet")
            || text_lower.contains("appoint")
        {
            return Some(PoliticsEventType::Confirmation);
        }

        // Check policy/legislation (bill + pass is specific)
        if text_lower.contains("bill")
            || text_lower.contains("legislation")
            || (text_lower.contains("pass") && !text_lower.contains("password"))
            || text_lower.contains("policy")
        {
            return Some(PoliticsEventType::PolicyVote);
        }

        // Check election (broader, check after more specific types)
        if text_lower.contains("election")
            || text_lower.contains("president")
            || text_lower.contains("governor")
            || text_lower.contains("senator")
            || text_lower.contains("mayor")
            || (text_lower.contains("vote") && text_lower.contains("win"))
            || (text_lower.contains("vote") && text_lower.contains("elect"))
        {
            return Some(PoliticsEventType::Election);
        }

        // Check general political content
        if text_lower.contains("politic")
            || text_lower.contains("government")
            || text_lower.contains("resign")
            || text_lower.contains("congress")
            || text_lower.contains("senate")
            || text_lower.contains("poll")
        {
            return Some(PoliticsEventType::Other);
        }

        None
    }

    /// Extract region from market text
    pub fn extract_region(text: &str) -> String {
        let text_lower = text.to_lowercase();

        // US regions/states
        if text_lower.contains("united states")
            || text_lower.contains("u.s.")
            || text_lower.contains("us ")
            || text_lower.contains("american")
            || text_lower.contains("white house")
            || text_lower.contains("congress")
            || text_lower.contains("senate")
        {
            return "US".to_string();
        }

        // Check for specific states
        let states = [
            ("california", "US-CA"),
            ("texas", "US-TX"),
            ("florida", "US-FL"),
            ("new york", "US-NY"),
            ("pennsylvania", "US-PA"),
            ("ohio", "US-OH"),
            ("georgia", "US-GA"),
            ("michigan", "US-MI"),
            ("arizona", "US-AZ"),
            ("wisconsin", "US-WI"),
            ("nevada", "US-NV"),
            ("north carolina", "US-NC"),
        ];

        for (state_name, code) in states {
            if text_lower.contains(state_name) {
                return code.to_string();
            }
        }

        // Other countries
        let countries = [
            ("uk", "UK"),
            ("britain", "UK"),
            ("british", "UK"),
            ("england", "UK"),
            ("france", "FR"),
            ("french", "FR"),
            ("germany", "DE"),
            ("german", "DE"),
            ("canada", "CA"),
            ("canadian", "CA"),
            ("australia", "AU"),
            ("australian", "AU"),
            ("brazil", "BR"),
            ("brazilian", "BR"),
            ("mexico", "MX"),
            ("mexican", "MX"),
            ("india", "IN"),
            ("indian", "IN"),
            ("china", "CN"),
            ("chinese", "CN"),
            ("russia", "RU"),
            ("russian", "RU"),
        ];

        for (name, code) in countries {
            if text_lower.contains(name) {
                return code.to_string();
            }
        }

        "GLOBAL".to_string()
    }

    /// Extract candidate/entity name from market text
    pub fn extract_entity(text: &str) -> Option<String> {
        // Common political figures (2024-2026 era)
        let politicians = [
            "trump",
            "biden",
            "harris",
            "desantis",
            "newsom",
            "pence",
            "haley",
            "ramaswamy",
            "scott",
            "christie",
            "mccarthy",
            "pelosi",
            "schumer",
            "mcconnell",
        ];

        let text_lower = text.to_lowercase();

        for name in politicians {
            if text_lower.contains(name) {
                // Capitalize first letter
                let capitalized = name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
                    + &name[1..];
                return Some(capitalized);
            }
        }

        // Try to extract "Will X win/become" pattern
        if let Some(caps) =
            regex::Regex::new(r"(?i)will\s+(\w+(?:\s+\w+)?)\s+(?:win|become|be)")
                .ok()?
                .captures(&text_lower)
        {
            return Some(caps.get(1)?.as_str().to_string());
        }

        None
    }

    /// Extract date from market text
    pub fn extract_date(text: &str) -> Option<NaiveDate> {
        let text_lower = text.to_lowercase();

        // Try "2024 election", "2026 election"
        if let Some(caps) = regex::Regex::new(r"(202[4-9])\s*election")
            .ok()?
            .captures(&text_lower)
        {
            if let Some(year_match) = caps.get(1) {
                if let Ok(year) = year_match.as_str().parse::<i32>() {
                    return NaiveDate::from_ymd_opt(year, 11, 5);
                }
            }
        }

        // Try "November 2024", "Nov 2026"
        if let Some(caps) = regex::Regex::new(r"(?i)nov(?:ember)?\s*(202[4-9])")
            .ok()?
            .captures(&text_lower)
        {
            if let Some(year_match) = caps.get(1) {
                if let Ok(year) = year_match.as_str().parse::<i32>() {
                    return NaiveDate::from_ymd_opt(year, 11, 5);
                }
            }
        }

        // Try "by 2025", "in 2026"
        if let Some(caps) = regex::Regex::new(r"(?i)(?:by|in|before)\s*(202[4-9])")
            .ok()?
            .captures(&text_lower)
        {
            if let Some(year_match) = caps.get(1) {
                if let Ok(year) = year_match.as_str().parse::<i32>() {
                    return NaiveDate::from_ymd_opt(year, 12, 31);
                }
            }
        }

        // Default to next November for election-type content
        let now = Utc::now();
        let year = if now.month() > 11 {
            now.year() + 1
        } else {
            now.year()
        };
        NaiveDate::from_ymd_opt(year, 11, 5)
    }

    /// Discover politics markets from Kalshi
    async fn discover_kalshi_markets(&self) -> Result<Vec<EventInfo>> {
        let kalshi = match &self.kalshi {
            Some(k) => k,
            None => return Ok(vec![]),
        };

        let mut events = Vec::new();

        // Get politics-related markets
        let markets = kalshi.get_markets(Some("POLITICS")).await?;

        for market in markets {
            let event_type = Self::detect_event_type(&market.title);
            if event_type.is_none() {
                continue;
            }

            let event_type = event_type.unwrap();
            let region = Self::extract_region(&market.title);
            let entity = Self::extract_entity(&market.title);
            let event_date = Self::extract_date(&market.title)
                .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
                .unwrap_or_else(|| Utc::now() + Duration::days(365));

            let market_type = MarketType::Politics {
                region: region.clone(),
                event_type: event_type.clone(),
            };

            let status = if market.status == "active" {
                EventStatus::Live
            } else if market.status == "closed" {
                EventStatus::Completed
            } else {
                EventStatus::Scheduled
            };

            events.push(EventInfo {
                event_id: format!("kalshi-pol-{}", market.ticker),
                market_type,
                entity_a: entity.unwrap_or_else(|| "Unknown".to_string()),
                entity_b: None,
                scheduled_time: event_date,
                status,
                venue: Some("Kalshi".to_string()),
                metadata: json!({
                    "source": "kalshi",
                    "ticker": market.ticker,
                    "title": market.title,
                    "region": region,
                    "event_type": format!("{:?}", event_type),
                }),
            });
        }

        Ok(events)
    }

    /// Discover politics markets from Polymarket
    async fn discover_polymarket_markets(&self) -> Result<Vec<EventInfo>> {
        let polymarket = match &self.polymarket {
            Some(p) => p,
            None => return Ok(vec![]),
        };

        let mut events = Vec::new();

        // Search for politics-related markets
        let search_terms = [
            "president",
            "election",
            "congress",
            "senate",
            "governor",
            "vote",
        ];

        for term in search_terms {
            match polymarket.search_markets(term, "").await {
                Ok(markets) => {
                    for market in markets {
                        let event_type = Self::detect_event_type(&market.question);
                        if event_type.is_none() {
                            continue;
                        }

                        let event_type = event_type.unwrap();
                        let region = Self::extract_region(&market.question);
                        let entity = Self::extract_entity(&market.question);
                        let event_date = Self::extract_date(&market.question)
                            .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
                            .unwrap_or_else(|| Utc::now() + Duration::days(365));

                        let market_type = MarketType::Politics {
                            region: region.clone(),
                            event_type: event_type.clone(),
                        };

                        // Get condition_id, skip if missing
                        let condition_id = match &market.condition_id {
                            Some(id) => id.clone(),
                            None => continue,
                        };

                        events.push(EventInfo {
                            event_id: format!("poly-pol-{}", condition_id),
                            market_type,
                            entity_a: entity.unwrap_or_else(|| "Unknown".to_string()),
                            entity_b: None,
                            scheduled_time: event_date,
                            status: EventStatus::Live,
                            venue: Some("Polymarket".to_string()),
                            metadata: json!({
                                "source": "polymarket",
                                "condition_id": condition_id,
                                "question": market.question,
                                "region": region,
                                "event_type": format!("{:?}", event_type),
                            }),
                        });
                    }
                }
                Err(e) => {
                    debug!("Failed to search Polymarket for {}: {}", term, e);
                }
            }
        }

        Ok(events)
    }
}

impl Default for PoliticsEventProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventProvider for PoliticsEventProvider {
    async fn get_live_events(&self) -> Result<Vec<EventInfo>> {
        let mut events = Vec::new();

        // Get from Kalshi
        match self.discover_kalshi_markets().await {
            Ok(kalshi_events) => {
                events.extend(kalshi_events.into_iter().filter(|e| e.status == EventStatus::Live));
            }
            Err(e) => debug!("Failed to get Kalshi politics markets: {}", e),
        }

        // Get from Polymarket
        match self.discover_polymarket_markets().await {
            Ok(poly_events) => {
                events.extend(poly_events.into_iter().filter(|e| e.status == EventStatus::Live));
            }
            Err(e) => debug!("Failed to get Polymarket politics markets: {}", e),
        }

        Ok(events)
    }

    async fn get_scheduled_events(&self, days: u32) -> Result<Vec<EventInfo>> {
        let cutoff = Utc::now() + Duration::days(days as i64);
        let mut events = Vec::new();

        // Get from Kalshi
        match self.discover_kalshi_markets().await {
            Ok(kalshi_events) => {
                events.extend(
                    kalshi_events
                        .into_iter()
                        .filter(|e| e.scheduled_time <= cutoff),
                );
            }
            Err(e) => debug!("Failed to get Kalshi politics markets: {}", e),
        }

        // Get from Polymarket
        match self.discover_polymarket_markets().await {
            Ok(poly_events) => {
                events.extend(
                    poly_events
                        .into_iter()
                        .filter(|e| e.scheduled_time <= cutoff),
                );
            }
            Err(e) => debug!("Failed to get Polymarket politics markets: {}", e),
        }

        Ok(events)
    }

    async fn get_event_state(&self, event_id: &str) -> Result<EventState> {
        // Find the event
        let all_events = self.get_scheduled_events(365).await?;

        let event = all_events
            .into_iter()
            .find(|e| e.event_id == event_id)
            .ok_or_else(|| anyhow!("Event not found: {}", event_id))?;

        // Politics events use market-implied probabilities
        // In a real implementation, we'd fetch current prices from the market
        Ok(EventState {
            event_id: event.event_id.clone(),
            market_type: event.market_type.clone(),
            entity_a: event.entity_a.clone(),
            entity_b: event.entity_b.clone(),
            status: event.status,
            state: StateData::Politics(PoliticsStateData {
                current_probability: None, // Would be filled from market prices
                last_updated: Utc::now(),
                poll_count: None,
                event_date: event.scheduled_time,
                metadata: event.metadata.clone(),
            }),
            fetched_at: Utc::now(),
        })
    }

    fn provider_name(&self) -> &str {
        "PoliticsProvider"
    }

    fn supported_market_types(&self) -> Vec<MarketType> {
        vec![MarketType::Politics {
            region: "US".to_string(),
            event_type: PoliticsEventType::Election,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_detect_event_type() {
        assert_eq!(
            PoliticsEventProvider::detect_event_type("2024 Presidential Election"),
            Some(PoliticsEventType::Election)
        );
        assert_eq!(
            PoliticsEventProvider::detect_event_type("Will Trump win the election?"),
            Some(PoliticsEventType::Election)
        );
        assert_eq!(
            PoliticsEventProvider::detect_event_type("Supreme Court nomination confirmation"),
            Some(PoliticsEventType::Confirmation)
        );
        assert_eq!(
            PoliticsEventProvider::detect_event_type("Will the bill pass Congress?"),
            Some(PoliticsEventType::PolicyVote)
        );
        assert_eq!(
            PoliticsEventProvider::detect_event_type("Impeachment proceedings"),
            Some(PoliticsEventType::Impeachment)
        );
        assert_eq!(
            PoliticsEventProvider::detect_event_type("Random text"),
            None
        );
    }

    #[test]
    fn test_extract_region() {
        assert_eq!(
            PoliticsEventProvider::extract_region("US Presidential Election"),
            "US"
        );
        assert_eq!(
            PoliticsEventProvider::extract_region("California Governor Race"),
            "US-CA"
        );
        assert_eq!(
            PoliticsEventProvider::extract_region("UK Parliament Vote"),
            "UK"
        );
        assert_eq!(
            PoliticsEventProvider::extract_region("French Election"),
            "FR"
        );
        assert_eq!(
            PoliticsEventProvider::extract_region("Random event"),
            "GLOBAL"
        );
    }

    #[test]
    fn test_extract_entity() {
        assert_eq!(
            PoliticsEventProvider::extract_entity("Will Trump win?"),
            Some("Trump".to_string())
        );
        assert_eq!(
            PoliticsEventProvider::extract_entity("Biden reelection"),
            Some("Biden".to_string())
        );
        assert_eq!(
            PoliticsEventProvider::extract_entity("DeSantis announcement"),
            Some("Desantis".to_string())
        );
    }

    #[test]
    fn test_extract_date() {
        let date = PoliticsEventProvider::extract_date("2024 election");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.year(), 2024);
        assert_eq!(d.month(), 11);

        let date = PoliticsEventProvider::extract_date("November 2026");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.year(), 2026);
        assert_eq!(d.month(), 11);
    }

    #[tokio::test]
    async fn test_provider_basics() {
        let provider = PoliticsEventProvider::new();
        assert_eq!(provider.provider_name(), "PoliticsProvider");

        let market_types = provider.supported_market_types();
        assert!(!market_types.is_empty());
        match &market_types[0] {
            MarketType::Politics { .. } => {}
            _ => panic!("Expected Politics market type"),
        }
    }
}
