use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const KALSHI_API: &str = "https://api.elections.kalshi.com/trade-api/v2";

#[derive(Debug, Clone)]
pub struct KalshiClient {
    client: Client,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KalshiMarket {
    pub ticker: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub status: String,
}

impl KalshiClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Fetch markets, optionally filtering by sport/series_ticker.
    /// Does NOT apply client-side query filtering (caller should handle that).
    pub async fn get_markets(&self, sport: Option<&str>) -> Result<Vec<KalshiMarket>> {
        let url = format!("{}/markets", KALSHI_API);

        let mut params = vec![
            ("limit", "500"),
            ("status", "open"),
        ];

        let series_ticker;
        if let Some(s) = sport {
            series_ticker = s.to_uppercase();
            params.push(("series_ticker", &series_ticker));
        }

        let resp = self.client.get(&url).query(&params).send().await?;

        // Handle potential errors or empty responses gracefully
        if !resp.status().is_success() {
            // Return empty list on error to avoid breaking loops? Or error?
            // For now error is better
            resp.error_for_status_ref()?;
        }

        let data: serde_json::Value = resp.json().await?;
        
        let markets = match data.get("markets") {
             Some(v) if !v.is_null() => serde_json::from_value(v.clone())?,
             _ => Vec::new(),
        };

        Ok(markets)
    }

    /// Legacy compat / search helper
    pub async fn search_markets(&self, query: &str, sport: &str) -> Result<Vec<KalshiMarket>> {
        let markets = self.get_markets(Some(sport)).await?;
        
        let query_norm = query.to_lowercase();
        if query_norm.is_empty() {
             return Ok(markets);
        }

        let filtered = markets
            .into_iter()
            .filter(|m| m.title.to_lowercase().contains(&query_norm))
            .collect();
        
        Ok(filtered)
    }
}
