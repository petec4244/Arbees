//! Economics Event Provider
//!
//! Provides economic indicator events from prediction markets
//! matched with real-time data from FRED (Federal Reserve Economic Data).

use super::{EconomicsStateData, EventInfo, EventProvider, EventState, EventStatus, StateData};
use crate::clients::fred::{series, FredClient};
use crate::clients::kalshi::KalshiClient;
use crate::clients::polymarket::PolymarketClient;
use crate::models::{EconomicIndicator, MarketType};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{Duration, NaiveDate, Utc};
use serde_json::json;
use std::sync::Arc;
use tracing::debug;

/// Economics event provider combining prediction markets with FRED data
pub struct EconomicsEventProvider {
    /// FRED client for economic data
    fred: Arc<FredClient>,
    /// Kalshi client for prediction markets
    kalshi: Option<Arc<KalshiClient>>,
    /// Polymarket client
    polymarket: Option<Arc<PolymarketClient>>,
}

impl EconomicsEventProvider {
    /// Create a new economics event provider
    pub fn new() -> Self {
        Self {
            fred: Arc::new(FredClient::new()),
            kalshi: None,
            polymarket: None,
        }
    }

    /// Create with custom clients (for sharing/testing)
    pub fn with_clients(
        fred: Arc<FredClient>,
        kalshi: Option<Arc<KalshiClient>>,
        polymarket: Option<Arc<PolymarketClient>>,
    ) -> Self {
        Self {
            fred,
            kalshi,
            polymarket,
        }
    }

    /// Detect economic indicator from market text
    pub fn detect_indicator(text: &str) -> Option<EconomicIndicator> {
        let text_lower = text.to_lowercase();

        // Check for specific indicators in order of specificity
        if text_lower.contains("core cpi") || text_lower.contains("core inflation") {
            Some(EconomicIndicator::CoreCPI)
        } else if text_lower.contains("cpi")
            || text_lower.contains("consumer price")
            || text_lower.contains("inflation")
        {
            Some(EconomicIndicator::CPI)
        } else if text_lower.contains("core pce") {
            Some(EconomicIndicator::CorePCE)
        } else if text_lower.contains("pce") || text_lower.contains("personal consumption") {
            Some(EconomicIndicator::PCE)
        } else if text_lower.contains("nonfarm")
            || text_lower.contains("payroll")
            || text_lower.contains("jobs report")
            || text_lower.contains("employment report")
        {
            Some(EconomicIndicator::NonfarmPayrolls)
        } else if text_lower.contains("unemployment")
            || text_lower.contains("jobless rate")
            || text_lower.contains("unemployment rate")
        {
            Some(EconomicIndicator::Unemployment)
        } else if text_lower.contains("fed fund")
            || text_lower.contains("interest rate")
            || text_lower.contains("fomc")
            || text_lower.contains("federal reserve")
        {
            Some(EconomicIndicator::FedFundsRate)
        } else if text_lower.contains("gdp growth") || text_lower.contains("gdp rate") {
            Some(EconomicIndicator::GDPGrowth)
        } else if text_lower.contains("gdp") || text_lower.contains("gross domestic") {
            Some(EconomicIndicator::GDP)
        } else if text_lower.contains("jobless claims")
            || text_lower.contains("initial claims")
            || text_lower.contains("weekly claims")
        {
            Some(EconomicIndicator::JoblessClaims)
        } else if text_lower.contains("consumer sentiment") || text_lower.contains("michigan") {
            Some(EconomicIndicator::ConsumerSentiment)
        } else if text_lower.contains("treasury") || text_lower.contains("yield") {
            if text_lower.contains("10") {
                Some(EconomicIndicator::Treasury10Y)
            } else if text_lower.contains("2") {
                Some(EconomicIndicator::Treasury2Y)
            } else {
                Some(EconomicIndicator::Treasury10Y)
            }
        } else {
            None
        }
    }

    /// Get FRED series ID for an indicator
    pub fn indicator_to_series(indicator: &EconomicIndicator) -> &'static str {
        match indicator {
            EconomicIndicator::CPI => series::CPI,
            EconomicIndicator::CoreCPI => series::CORE_CPI,
            EconomicIndicator::PCE => series::PCE,
            EconomicIndicator::CorePCE => series::CORE_PCE,
            EconomicIndicator::Unemployment => series::UNEMPLOYMENT,
            EconomicIndicator::NonfarmPayrolls => series::NONFARM_PAYROLLS,
            EconomicIndicator::FedFundsRate => series::FED_FUNDS_RATE,
            EconomicIndicator::GDP => series::GDP,
            EconomicIndicator::GDPGrowth => series::GDP_GROWTH,
            EconomicIndicator::JoblessClaims => series::JOBLESS_CLAIMS,
            EconomicIndicator::ConsumerSentiment => series::CONSUMER_SENTIMENT,
            EconomicIndicator::Treasury10Y => series::TREASURY_10Y,
            EconomicIndicator::Treasury2Y => series::TREASURY_2Y,
        }
    }

    /// Extract threshold value from market text
    /// e.g., "CPI above 3.5%?" -> 3.5
    pub fn extract_threshold(text: &str) -> Option<f64> {
        // Look for percentage patterns
        let patterns = [
            // "above 3.5%"
            regex::Regex::new(r"(?i)(?:above|below|over|under|exceed|at least)\s*(\d+\.?\d*)%?")
                .ok()?,
            // "3.5% or higher"
            regex::Regex::new(r"(\d+\.?\d*)%?\s*(?:or higher|or more|or lower|or less)").ok()?,
            // "reach 3.5"
            regex::Regex::new(r"(?i)reach\s*(\d+\.?\d*)").ok()?,
            // "hit 3.5%"
            regex::Regex::new(r"(?i)hit\s*(\d+\.?\d*)%?").ok()?,
        ];

        for pattern in &patterns {
            if let Some(caps) = pattern.captures(text) {
                if let Some(m) = caps.get(1) {
                    if let Ok(val) = m.as_str().parse::<f64>() {
                        return Some(val);
                    }
                }
            }
        }

        None
    }

    /// Extract date from market text
    /// e.g., "January 2026 CPI" -> 2026-01-15
    pub fn extract_date(text: &str) -> Option<NaiveDate> {
        let months = [
            ("january", 1),
            ("february", 2),
            ("march", 3),
            ("april", 4),
            ("may", 5),
            ("june", 6),
            ("july", 7),
            ("august", 8),
            ("september", 9),
            ("october", 10),
            ("november", 11),
            ("december", 12),
            ("jan", 1),
            ("feb", 2),
            ("mar", 3),
            ("apr", 4),
            ("jun", 6),
            ("jul", 7),
            ("aug", 8),
            ("sep", 9),
            ("oct", 10),
            ("nov", 11),
            ("dec", 12),
        ];

        let text_lower = text.to_lowercase();

        // Look for "Month YYYY" pattern
        for (month_name, month_num) in months {
            if text_lower.contains(month_name) {
                // Try to find year near the month
                if let Some(year_match) =
                    regex::Regex::new(r"(202[4-9]|203[0-5])").ok()?.find(&text_lower)
                {
                    if let Ok(year) = year_match.as_str().parse::<i32>() {
                        return NaiveDate::from_ymd_opt(year, month_num, 15);
                    }
                }
            }
        }

        // Look for Q1/Q2/Q3/Q4 patterns
        if let Some(caps) = regex::Regex::new(r"(?i)Q([1-4])\s*(202[4-9])")
            .ok()?
            .captures(&text_lower)
        {
            let quarter: u32 = caps.get(1)?.as_str().parse().ok()?;
            let year: i32 = caps.get(2)?.as_str().parse().ok()?;
            let month = match quarter {
                1 => 2,  // Mid Q1
                2 => 5,  // Mid Q2
                3 => 8,  // Mid Q3
                4 => 11, // Mid Q4
                _ => return None,
            };
            return NaiveDate::from_ymd_opt(year, month, 15);
        }

        None
    }

    /// Discover economics markets from Kalshi
    async fn discover_kalshi_markets(&self) -> Result<Vec<EventInfo>> {
        let kalshi = match &self.kalshi {
            Some(k) => k,
            None => return Ok(vec![]),
        };

        let mut events = Vec::new();

        // Get economics-related markets from Kalshi
        let markets = kalshi.get_markets(Some("ECONOMICS")).await?;

        for market in markets {
            // Try to detect indicator type
            let indicator = Self::detect_indicator(&market.title);
            if indicator.is_none() {
                continue;
            }

            let indicator = indicator.unwrap();
            let threshold = Self::extract_threshold(&market.title);
            let release_date = Self::extract_date(&market.title)
                .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
                .unwrap_or_else(|| Utc::now() + Duration::days(30));

            let market_type = MarketType::Economics {
                indicator: indicator.clone(),
                threshold,
            };

            let status = if market.status == "active" {
                EventStatus::Live
            } else if market.status == "closed" {
                EventStatus::Completed
            } else {
                EventStatus::Scheduled
            };

            events.push(EventInfo {
                event_id: format!("kalshi-econ-{}", market.ticker),
                market_type,
                entity_a: format!("{:?}", indicator),
                entity_b: threshold.map(|t| format!("{}", t)),
                scheduled_time: release_date,
                status,
                venue: Some("Kalshi".to_string()),
                metadata: json!({
                    "source": "kalshi",
                    "ticker": market.ticker,
                    "title": market.title,
                    "indicator": format!("{:?}", indicator),
                }),
            });
        }

        Ok(events)
    }

    /// Discover economics markets from Polymarket
    async fn discover_polymarket_markets(&self) -> Result<Vec<EventInfo>> {
        let polymarket = match &self.polymarket {
            Some(p) => p,
            None => return Ok(vec![]),
        };

        let mut events = Vec::new();

        // Search for economics-related markets
        let search_terms = ["CPI", "inflation", "unemployment", "GDP", "fed rate", "interest rate"];

        for term in search_terms {
            // Pass empty string for sport since economics aren't sports
            match polymarket.search_markets(term, "").await {
                Ok(markets) => {
                    for market in markets {
                        let indicator = Self::detect_indicator(&market.question);
                        if indicator.is_none() {
                            continue;
                        }

                        let indicator = indicator.unwrap();
                        let threshold = Self::extract_threshold(&market.question);
                        let release_date = Self::extract_date(&market.question)
                            .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
                            .unwrap_or_else(|| Utc::now() + Duration::days(30));

                        let market_type = MarketType::Economics {
                            indicator: indicator.clone(),
                            threshold,
                        };

                        // Markets returned from search are assumed to be active
                        let status = EventStatus::Live;

                        // Get condition_id, skip if missing
                        let condition_id = match &market.condition_id {
                            Some(id) => id.clone(),
                            None => continue,
                        };

                        events.push(EventInfo {
                            event_id: format!("poly-econ-{}", condition_id),
                            market_type,
                            entity_a: format!("{:?}", indicator),
                            entity_b: threshold.map(|t| format!("{}", t)),
                            scheduled_time: release_date,
                            status,
                            venue: Some("Polymarket".to_string()),
                            metadata: json!({
                                "source": "polymarket",
                                "condition_id": condition_id,
                                "question": market.question,
                                "indicator": format!("{:?}", indicator),
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

    /// Enrich event with current FRED data
    async fn enrich_with_fred_data(&self, event: &EventInfo) -> Result<EventState> {
        let indicator = match &event.market_type {
            MarketType::Economics { indicator, .. } => indicator.clone(),
            _ => return Err(anyhow!("Not an economics market")),
        };

        let series_id = Self::indicator_to_series(&indicator);

        // Get latest data from FRED
        let fred_data = self.fred.get_latest(series_id).await?;

        let threshold = match &event.market_type {
            MarketType::Economics {
                threshold: Some(t), ..
            } => *t,
            _ => fred_data.latest_value.unwrap_or(0.0),
        };

        Ok(EventState {
            event_id: event.event_id.clone(),
            market_type: event.market_type.clone(),
            entity_a: event.entity_a.clone(),
            entity_b: event.entity_b.clone(),
            status: event.status,
            state: StateData::Economics(EconomicsStateData {
                current_value: fred_data.latest_value,
                forecast_value: Some(threshold),
                release_date: event.scheduled_time,
                previous_value: fred_data.previous_value,
                metadata: json!({
                    "yoy_change": fred_data.yoy_change,
                    "mom_change": fred_data.mom_change,
                    "series_id": series_id,
                    "latest_date": fred_data.latest_date,
                }),
            }),
            fetched_at: Utc::now(),
        })
    }
}

impl Default for EconomicsEventProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventProvider for EconomicsEventProvider {
    async fn get_live_events(&self) -> Result<Vec<EventInfo>> {
        let mut events = Vec::new();

        // Get from Kalshi
        match self.discover_kalshi_markets().await {
            Ok(kalshi_events) => {
                events.extend(kalshi_events.into_iter().filter(|e| e.status == EventStatus::Live));
            }
            Err(e) => debug!("Failed to get Kalshi economics markets: {}", e),
        }

        // Get from Polymarket
        match self.discover_polymarket_markets().await {
            Ok(poly_events) => {
                events.extend(poly_events.into_iter().filter(|e| e.status == EventStatus::Live));
            }
            Err(e) => debug!("Failed to get Polymarket economics markets: {}", e),
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
            Err(e) => debug!("Failed to get Kalshi economics markets: {}", e),
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
            Err(e) => debug!("Failed to get Polymarket economics markets: {}", e),
        }

        Ok(events)
    }

    async fn get_event_state(&self, event_id: &str) -> Result<EventState> {
        // First find the event
        let all_events = self.get_scheduled_events(365).await?;

        let event = all_events
            .into_iter()
            .find(|e| e.event_id == event_id)
            .ok_or_else(|| anyhow!("Event not found: {}", event_id))?;

        // Enrich with FRED data
        self.enrich_with_fred_data(&event).await
    }

    fn provider_name(&self) -> &str {
        "EconomicsProvider"
    }

    fn supported_market_types(&self) -> Vec<MarketType> {
        vec![MarketType::Economics {
            indicator: EconomicIndicator::CPI,
            threshold: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_detect_indicator() {
        assert_eq!(
            EconomicsEventProvider::detect_indicator("CPI above 3%"),
            Some(EconomicIndicator::CPI)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("Core CPI reading"),
            Some(EconomicIndicator::CoreCPI)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("Unemployment rate below 4%"),
            Some(EconomicIndicator::Unemployment)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("Nonfarm payrolls report"),
            Some(EconomicIndicator::NonfarmPayrolls)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("Fed funds rate decision"),
            Some(EconomicIndicator::FedFundsRate)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("GDP growth rate"),
            Some(EconomicIndicator::GDPGrowth)
        );
        assert_eq!(
            EconomicsEventProvider::detect_indicator("Random text"),
            None
        );
    }

    #[test]
    fn test_extract_threshold() {
        assert_eq!(
            EconomicsEventProvider::extract_threshold("CPI above 3.5%"),
            Some(3.5)
        );
        assert_eq!(
            EconomicsEventProvider::extract_threshold("Unemployment below 4%"),
            Some(4.0)
        );
        assert_eq!(
            EconomicsEventProvider::extract_threshold("Will reach 5.25"),
            Some(5.25)
        );
        assert_eq!(
            EconomicsEventProvider::extract_threshold("Hit 100000 jobs"),
            Some(100000.0)
        );
        assert_eq!(
            EconomicsEventProvider::extract_threshold("No threshold here"),
            None
        );
    }

    #[test]
    fn test_extract_date() {
        let date = EconomicsEventProvider::extract_date("January 2026 CPI");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.month(), 1);
        assert_eq!(d.year(), 2026);

        let date = EconomicsEventProvider::extract_date("Q2 2025 GDP");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.month(), 5); // Mid Q2
        assert_eq!(d.year(), 2025);

        let date = EconomicsEventProvider::extract_date("No date here");
        assert!(date.is_none());
    }

    #[test]
    fn test_indicator_to_series() {
        assert_eq!(
            EconomicsEventProvider::indicator_to_series(&EconomicIndicator::CPI),
            series::CPI
        );
        assert_eq!(
            EconomicsEventProvider::indicator_to_series(&EconomicIndicator::Unemployment),
            series::UNEMPLOYMENT
        );
        assert_eq!(
            EconomicsEventProvider::indicator_to_series(&EconomicIndicator::FedFundsRate),
            series::FED_FUNDS_RATE
        );
    }

    #[tokio::test]
    async fn test_provider_basics() {
        let provider = EconomicsEventProvider::new();
        assert_eq!(provider.provider_name(), "EconomicsProvider");

        let market_types = provider.supported_market_types();
        assert!(!market_types.is_empty());
        match &market_types[0] {
            MarketType::Economics { .. } => {}
            _ => panic!("Expected Economics market type"),
        }
    }
}
