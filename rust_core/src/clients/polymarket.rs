use crate::utils::matching::names_match; // Updated import
use anyhow::Result;
use log::{error, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const POLY_API: &str = "https://gamma-api.polymarket.com/markets";

fn tag_id_for_slug(slug: &str) -> Option<u64> {
    // Gamma tag slugs -> numeric tag IDs (as returned by https://gamma-api.polymarket.com/tags).
    // We hardcode these to keep discovery low-latency and avoid an extra tags lookup call.
    // Tag IDs can be found at: https://gamma-api.polymarket.com/tags
    match slug.to_lowercase().as_str() {
        // Broad category
        "sports" => Some(1),
        // Sport types
        "basketball" => Some(28),
        "football" => Some(10),
        "hockey" => Some(100088),
        "baseball" => Some(100089),
        "soccer" => Some(100090),
        "tennis" => Some(100091),
        "mma" | "ufc" => Some(100092),
        // League-specific (preferred - more precise)
        "nba" => Some(745),
        "nfl" => Some(450),
        "nhl" => Some(899),
        "ncaab" | "ncaa_basketball" => Some(101952),
        "ncaaf" | "ncaa_football" | "cfb" => Some(101953),
        "mlb" => Some(100094),
        "mls" => Some(100095),
        "epl" | "premier_league" => Some(100096),
        "uefa" | "champions_league" => Some(100097),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct PolymarketClient {
    client: Client,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Market {
    pub id: String,
    pub condition_id: Option<String>,
    pub question: String,
    pub outcomes: Option<Value>, // Can be array or JSON string
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: Option<Value>, // Can be array or JSON string
    pub tokens: Option<Value>,   // Can be array or JSON string
}

impl PolymarketClient {
    pub fn new() -> Self {
        let mut client_builder = Client::builder().timeout(std::time::Duration::from_secs(10));

        // Check for proxy in environment
        if let Ok(proxy_url) = std::env::var("POLYMARKET_PROXY_URL") {
            if !proxy_url.is_empty() {
                if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                    client_builder = client_builder.proxy(proxy);
                    info!("Polymarket Rust client using proxy: {}", proxy_url);
                }
            }
        }

        Self {
            client: client_builder.build().unwrap_or_else(|_| Client::new()),
        }
    }

    pub fn parse_json_string_or_array(v: &Value) -> Vec<String> {
        match v {
            Value::Array(arr) => arr
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect(),
            Value::String(s) => {
                let parsed: Value = serde_json::from_str(s).unwrap_or(Value::Null);
                if let Value::Array(arr) = parsed {
                    arr.iter()
                        .map(|item| item.as_str().unwrap_or_default().to_string())
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    pub async fn search_markets(&self, query: &str, sport: &str) -> Result<Vec<Market>> {
        let url = format!("{}", POLY_API);

        // More intensive fetch: paginate through top markets by tag and filter locally.
        // This avoids missing games that aren't in the top 500 by volume.
        const BATCH_SIZE: usize = 500;
        const MAX_FETCH: usize = 5000;

        // IMPORTANT: Gamma's /markets does NOT filter correctly with `tag=<slug>`.
        // It *does* filter with `tag_id=<numeric>`. If we don't have a tag id, fall back
        // to the broad Sports tag to avoid returning non-sports markets.
        let tag_id = tag_id_for_slug(sport).unwrap_or(1);

        let mut all: Vec<Market> = Vec::new();
        let mut offset: usize = 0;

        loop {
            let params = [
                ("limit", BATCH_SIZE.to_string()),
                ("offset", offset.to_string()),
                ("closed", "false".to_string()),
                ("active", "true".to_string()),
                ("tag_id", tag_id.to_string()),
                ("order", "volume".to_string()),
                ("ascending", "false".to_string()),
            ];

            let resp = self.client.get(&url).query(&params).send().await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                error!("Polymarket API Error: {} - {}", status, text);
                break;
            }

            let batch: Vec<Market> = resp.json().await?;
            let count = batch.len();
            if count == 0 {
                break;
            }

            all.extend(batch);
            offset += count;

            if count < BATCH_SIZE || all.len() >= MAX_FETCH {
                break;
            }
        }

        info!(
            "Polymarket fetched {} markets for tag '{}' (tag_id={})",
            all.len(),
            sport,
            tag_id
        );

        // Filter locally by query in title (if query is empty, this keeps all)
        let query_norm = query.to_lowercase();
        let filtered: Vec<Market> = if query_norm.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|m| m.question.to_lowercase().contains(&query_norm))
                .collect()
        };

        Ok(filtered)
    }

    pub fn resolve_token_id(
        &self,
        market: &Market,
        outcome_candidate: &str,
        sport: &str,
    ) -> Option<String> {
        // 1. Try parallel arrays (outcomes + clobTokenIds)
        if let (Some(outcomes_val), Some(clob_ids_val)) = (&market.outcomes, &market.clob_token_ids)
        {
            let outcomes = Self::parse_json_string_or_array(outcomes_val);
            let clob_ids = Self::parse_json_string_or_array(clob_ids_val);

            if outcomes.len() == clob_ids.len() && !outcomes.is_empty() {
                for (i, outcome) in outcomes.iter().enumerate() {
                    if names_match(outcome_candidate, outcome, sport) {
                        return Some(clob_ids[i].clone());
                    }
                }
            }
        }

        // 2. Try tokens array
        if let Some(tokens_val) = &market.tokens {
            let tokens_json = match tokens_val {
                Value::Array(arr) => arr.clone(),
                Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
                _ => vec![],
            };

            for t_val in tokens_json {
                let outcome = t_val["outcome"].as_str().unwrap_or_default();
                if names_match(outcome_candidate, outcome, sport) {
                    if let Some(tid) = t_val["token_id"].as_str().or_else(|| t_val["id"].as_str()) {
                        return Some(tid.to_string());
                    }
                }
            }
        }

        None
    }
}
