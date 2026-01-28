//! FRED API Client (Federal Reserve Economic Data)
//!
//! Provides economic indicator data for economics prediction market
//! probability calculations.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::debug;

/// FRED API client with caching
pub struct FredClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    /// Cache: series_id -> (observations, fetched_at)
    cache: Arc<RwLock<HashMap<String, (Vec<Observation>, DateTime<Utc>)>>>,
    /// Cache TTL in seconds (economic data updates slowly)
    cache_ttl_secs: i64,
}

/// Common economic indicator series IDs
pub mod series {
    /// Consumer Price Index for All Urban Consumers
    pub const CPI: &str = "CPIAUCSL";
    /// Core CPI (excluding food and energy)
    pub const CORE_CPI: &str = "CPILFESL";
    /// Personal Consumption Expenditures
    pub const PCE: &str = "PCEPI";
    /// Core PCE (Fed's preferred inflation measure)
    pub const CORE_PCE: &str = "PCEPILFE";
    /// Unemployment Rate
    pub const UNEMPLOYMENT: &str = "UNRATE";
    /// Nonfarm Payrolls
    pub const NONFARM_PAYROLLS: &str = "PAYEMS";
    /// Federal Funds Effective Rate
    pub const FED_FUNDS_RATE: &str = "FEDFUNDS";
    /// Real GDP
    pub const GDP: &str = "GDPC1";
    /// GDP Growth Rate (annualized)
    pub const GDP_GROWTH: &str = "A191RL1Q225SBEA";
    /// 10-Year Treasury Rate
    pub const TREASURY_10Y: &str = "DGS10";
    /// 2-Year Treasury Rate
    pub const TREASURY_2Y: &str = "DGS2";
    /// Consumer Sentiment (University of Michigan)
    pub const CONSUMER_SENTIMENT: &str = "UMCSENT";
    /// Initial Jobless Claims
    pub const JOBLESS_CLAIMS: &str = "ICSA";
}

/// Single observation from FRED
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub date: NaiveDate,
    pub value: Option<f64>,
}

/// Series metadata from FRED
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInfo {
    pub id: String,
    pub title: String,
    pub frequency: String,
    pub units: String,
    pub seasonal_adjustment: String,
    pub last_updated: Option<String>,
}

/// Economic indicator with latest data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicIndicator {
    pub series_id: String,
    pub title: String,
    pub latest_value: Option<f64>,
    pub latest_date: Option<NaiveDate>,
    pub previous_value: Option<f64>,
    pub previous_date: Option<NaiveDate>,
    pub yoy_change: Option<f64>,
    pub mom_change: Option<f64>,
    pub units: String,
}

/// Release schedule information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub id: i32,
    pub name: String,
    pub press_release: bool,
    pub next_release_date: Option<NaiveDate>,
}

impl FredClient {
    /// Create a new FRED client
    pub fn new() -> Self {
        Self::with_api_key(std::env::var("FRED_API_KEY").ok())
    }

    /// Create with explicit API key
    pub fn with_api_key(api_key: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Arbees/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: "https://api.stlouisfed.org/fred".to_string(),
            api_key,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_secs: 3600, // 1 hour cache for economic data
        }
    }

    /// Map common names to FRED series IDs
    pub fn name_to_series_id(name: &str) -> &str {
        match name.to_uppercase().as_str() {
            "CPI" | "INFLATION" => series::CPI,
            "CORE CPI" | "CORE_CPI" => series::CORE_CPI,
            "PCE" => series::PCE,
            "CORE PCE" | "CORE_PCE" => series::CORE_PCE,
            "UNEMPLOYMENT" | "JOBLESS" | "UNEMPLOYMENT RATE" => series::UNEMPLOYMENT,
            "PAYROLLS" | "NFP" | "NONFARM" | "JOBS" => series::NONFARM_PAYROLLS,
            "FED FUNDS" | "INTEREST RATE" | "FED RATE" => series::FED_FUNDS_RATE,
            "GDP" => series::GDP,
            "GDP GROWTH" | "GDP_GROWTH" => series::GDP_GROWTH,
            "10Y" | "10 YEAR" | "TREASURY" => series::TREASURY_10Y,
            "2Y" | "2 YEAR" => series::TREASURY_2Y,
            "SENTIMENT" | "CONSUMER SENTIMENT" => series::CONSUMER_SENTIMENT,
            "CLAIMS" | "JOBLESS CLAIMS" | "INITIAL CLAIMS" => series::JOBLESS_CLAIMS,
            _ => name, // Assume it's already a series ID
        }
    }

    /// Get observations for a series
    pub async fn get_observations(
        &self,
        series_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<Observation>> {
        let series_id = Self::name_to_series_id(series_id);

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some((obs, fetched_at)) = cache.get(series_id) {
                let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
                if age < self.cache_ttl_secs {
                    debug!("Cache hit for {}", series_id);
                    let limit = limit.unwrap_or(100) as usize;
                    return Ok(obs.iter().rev().take(limit).cloned().collect());
                }
            }
        }

        // Build URL
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("FRED_API_KEY not set"))?;

        let limit_param = limit.unwrap_or(100);
        let url = format!(
            "{}/series/observations?series_id={}&api_key={}&file_type=json&sort_order=desc&limit={}",
            self.base_url, series_id, api_key, limit_param
        );

        debug!("Fetching observations for {} from FRED", series_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from FRED")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("FRED API error: {} - {}", status, body));
        }

        let fred_response: FredObservationsResponse = response
            .json()
            .await
            .context("Failed to parse FRED response")?;

        let observations: Vec<Observation> = fred_response
            .observations
            .into_iter()
            .filter_map(|obs| {
                let date = NaiveDate::parse_from_str(&obs.date, "%Y-%m-%d").ok()?;
                let value = if obs.value == "." {
                    None
                } else {
                    obs.value.parse::<f64>().ok()
                };
                Some(Observation { date, value })
            })
            .collect();

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(series_id.to_string(), (observations.clone(), Utc::now()));
        }

        Ok(observations)
    }

    /// Get latest value for a series
    pub async fn get_latest(&self, series_id: &str) -> Result<EconomicIndicator> {
        let observations = self.get_observations(series_id, Some(13)).await?;

        // Get latest and previous values
        let latest = observations.first();
        let previous = observations.get(1);

        // Try to find value from ~12 months ago for YoY
        let year_ago = observations.get(12);

        let (latest_value, latest_date) = match latest {
            Some(obs) => (obs.value, Some(obs.date)),
            None => (None, None),
        };

        let (previous_value, previous_date) = match previous {
            Some(obs) => (obs.value, Some(obs.date)),
            None => (None, None),
        };

        // Calculate month-over-month change
        let mom_change = match (latest_value, previous_value) {
            (Some(l), Some(p)) if p != 0.0 => Some((l - p) / p * 100.0),
            _ => None,
        };

        // Calculate year-over-year change
        let yoy_change = match (latest_value, year_ago.and_then(|o| o.value)) {
            (Some(l), Some(ya)) if ya != 0.0 => Some((l - ya) / ya * 100.0),
            _ => None,
        };

        Ok(EconomicIndicator {
            series_id: Self::name_to_series_id(series_id).to_string(),
            title: series_id.to_string(), // Would need series/info endpoint for real title
            latest_value,
            latest_date,
            previous_value,
            previous_date,
            yoy_change,
            mom_change,
            units: "".to_string(),
        })
    }

    /// Get multiple indicators at once
    pub async fn get_indicators(&self, series_ids: &[&str]) -> Result<Vec<EconomicIndicator>> {
        let mut indicators = Vec::new();

        for series_id in series_ids {
            match self.get_latest(series_id).await {
                Ok(ind) => indicators.push(ind),
                Err(e) => {
                    debug!("Failed to get {}: {}", series_id, e);
                }
            }
        }

        Ok(indicators)
    }

    /// Get key economic indicators
    pub async fn get_key_indicators(&self) -> Result<Vec<EconomicIndicator>> {
        self.get_indicators(&[
            series::CPI,
            series::CORE_PCE,
            series::UNEMPLOYMENT,
            series::FED_FUNDS_RATE,
            series::GDP_GROWTH,
        ])
        .await
    }

    /// Calculate consensus deviation
    ///
    /// Returns how far the actual value deviated from consensus forecast
    pub fn calculate_consensus_deviation(
        actual: f64,
        consensus: f64,
        _historical_std: Option<f64>,
    ) -> f64 {
        if consensus == 0.0 {
            return 0.0;
        }
        (actual - consensus) / consensus.abs() * 100.0
    }
}

impl Default for FredClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal FRED API response structures
#[derive(Debug, Deserialize)]
struct FredObservationsResponse {
    observations: Vec<FredObservation>,
}

#[derive(Debug, Deserialize)]
struct FredObservation {
    date: String,
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_to_series_id() {
        assert_eq!(FredClient::name_to_series_id("CPI"), series::CPI);
        assert_eq!(FredClient::name_to_series_id("inflation"), series::CPI);
        assert_eq!(
            FredClient::name_to_series_id("UNEMPLOYMENT"),
            series::UNEMPLOYMENT
        );
        assert_eq!(FredClient::name_to_series_id("NFP"), series::NONFARM_PAYROLLS);
        assert_eq!(FredClient::name_to_series_id("unknown"), "unknown");
    }

    #[test]
    fn test_consensus_deviation() {
        // Actual higher than consensus
        let dev = FredClient::calculate_consensus_deviation(3.5, 3.0, None);
        assert!((dev - 16.67).abs() < 0.1);

        // Actual lower than consensus
        let dev = FredClient::calculate_consensus_deviation(2.5, 3.0, None);
        assert!((dev - (-16.67)).abs() < 0.1);

        // Exact match
        let dev = FredClient::calculate_consensus_deviation(3.0, 3.0, None);
        assert!((dev - 0.0).abs() < 0.001);
    }

    #[tokio::test]
    #[ignore] // Requires FRED API key
    async fn test_get_observations() {
        let client = FredClient::new();
        let obs = client.get_observations(series::UNEMPLOYMENT, Some(5)).await;
        assert!(obs.is_ok() || obs.unwrap_err().to_string().contains("API_KEY"));
    }
}
