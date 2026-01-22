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

    pub async fn search_markets(&self, query: &str, sport: &str) -> Result<Vec<KalshiMarket>> {
        let url = format!("{}/markets", KALSHI_API);

        // Kalshi uses series_ticker for sports filtering
        // Map sport to Kalshi series (e.g., "nba" -> "NBA", "ncaab" -> "NCAAB")
        let series_ticker = sport.to_uppercase();

        let params = [
            ("limit", "500"),
            ("status", "open"),
            ("series_ticker", series_ticker.as_str()),
        ];

        let resp = self.client.get(&url).query(&params).send().await?;

        let data: serde_json::Value = resp.json().await?;
        let markets: Vec<KalshiMarket> = serde_json::from_value(data["markets"].clone())?;

        // Filter locally by query in title
        let query_norm = query.to_lowercase();
        let filtered = markets
            .into_iter()
            .filter(|m| m.title.to_lowercase().contains(&query_norm))
            .collect();

        Ok(filtered)
    }
}
