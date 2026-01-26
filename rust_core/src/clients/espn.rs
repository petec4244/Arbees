use crate::circuit_breaker::{ApiCircuitBreaker, ApiCircuitBreakerConfig};
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct EspnClient {
    client: Client,
    circuit_breaker: Arc<ApiCircuitBreaker>,
}

impl std::fmt::Debug for EspnClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EspnClient")
            .field("circuit_breaker_state", &self.circuit_breaker.state())
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub date: String,
    pub home_team: String,
    pub away_team: String,
    pub home_abbr: String,
    pub away_abbr: String,
    // Game state
    pub home_score: u16,
    pub away_score: u16,
    pub period: u8,
    pub time_remaining_seconds: u32,
    pub status: String,
    pub possession: Option<String>,
    // Football-specific
    pub down: Option<u8>,
    pub yards_to_go: Option<u8>,
    pub yard_line: Option<u8>,
    pub is_redzone: bool,
}

impl EspnClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            circuit_breaker: Arc::new(ApiCircuitBreaker::new(
                "espn",
                ApiCircuitBreakerConfig {
                    failure_threshold: 5,
                    recovery_timeout: Duration::from_secs(30),
                    success_threshold: 2,
                },
            )),
        }
    }

    /// Create with custom circuit breaker configuration
    pub fn with_config(config: ApiCircuitBreakerConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            circuit_breaker: Arc::new(ApiCircuitBreaker::new("espn", config)),
        }
    }

    /// Check if the ESPN API is available (circuit breaker is not open)
    pub fn is_available(&self) -> bool {
        self.circuit_breaker.is_available()
    }

    /// Get the current circuit breaker state
    pub fn circuit_state(&self) -> crate::circuit_breaker::ApiCircuitState {
        self.circuit_breaker.state()
    }

    /// Reset the circuit breaker
    pub fn reset_circuit_breaker(&self) {
        self.circuit_breaker.reset();
    }

    pub async fn get_games(&self, sport: &str, league: &str) -> Result<Vec<Game>> {
        // Check circuit breaker before making request
        if !self.circuit_breaker.is_available() {
            return Err(anyhow!(
                "ESPN API circuit breaker is open (sport={}, league={})",
                sport,
                league
            ));
        }

        let url = format!(
            "http://site.api.espn.com/apis/site/v2/sports/{}/{}/scoreboard",
            sport, league
        );

        let result = self.fetch_games_internal(&url).await;

        // Record success or failure
        match &result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(_) => self.circuit_breaker.record_failure(),
        }

        result
    }

    /// Internal fetch method that performs the actual HTTP request
    async fn fetch_games_internal(&self, url: &str) -> Result<Vec<Game>> {
        let resp = self.client.get(url).send().await?;
        let data: serde_json::Value = resp.json().await?;

        let mut games = Vec::new();

        if let Some(events) = data["events"].as_array() {
            for event in events {
                let id = event["id"].as_str().unwrap_or_default().to_string();
                let name = event["name"].as_str().unwrap_or_default().to_string();
                let short_name = event["shortName"].as_str().unwrap_or_default().to_string();
                let date = event["date"].as_str().unwrap_or_default().to_string();

                let competitions = &event["competitions"][0];
                let competitors = competitions["competitors"].as_array();
                let situation = &competitions["situation"];
                let status_obj = &event["status"];

                let mut home_team = String::new();
                let mut away_team = String::new();
                let mut home_abbr = String::new();
                let mut away_abbr = String::new();
                let mut home_score: u16 = 0;
                let mut away_score: u16 = 0;
                let mut possession: Option<String> = None;

                if let Some(comps) = competitors {
                    for comp in comps {
                        let team = &comp["team"];
                        let team_name = team["displayName"].as_str().unwrap_or_default().to_string();
                        let team_abbr =
                            team["abbreviation"].as_str().unwrap_or_default().to_string();
                        let score = comp["score"]
                            .as_str()
                            .and_then(|s| s.parse::<u16>().ok())
                            .unwrap_or(0);
                        let has_possession = comp["possession"].as_bool().unwrap_or(false);

                        if comp["homeAway"].as_str() == Some("home") {
                            home_team = team_name.clone();
                            home_abbr = team_abbr;
                            home_score = score;
                            if has_possession {
                                possession = Some(team_name);
                            }
                        } else {
                            away_team = team_name.clone();
                            away_abbr = team_abbr;
                            away_score = score;
                            if has_possession {
                                possession = Some(team_name);
                            }
                        }
                    }
                }

                // Parse period and time
                let period = status_obj["period"].as_u64().unwrap_or(1) as u8;
                let clock_str = status_obj["displayClock"].as_str().unwrap_or("0:00");
                let time_remaining_seconds = parse_clock(clock_str);
                let status = status_obj["type"]["name"]
                    .as_str()
                    .unwrap_or("scheduled")
                    .to_string();

                // Football-specific situation
                let down = situation["down"].as_u64().map(|d| d as u8);
                let yards_to_go = situation["distance"].as_u64().map(|d| d as u8);
                let yard_line = situation["yardLine"].as_u64().map(|y| y as u8);
                let is_redzone = situation["isRedZone"].as_bool().unwrap_or(false);

                games.push(Game {
                    id,
                    name,
                    short_name,
                    date,
                    home_team,
                    away_team,
                    home_abbr,
                    away_abbr,
                    home_score,
                    away_score,
                    period,
                    time_remaining_seconds,
                    status,
                    possession,
                    down,
                    yards_to_go,
                    yard_line,
                    is_redzone,
                });
            }
        }

        Ok(games)
    }
}

/// Parse clock string like "12:34" or "5:00" into seconds
fn parse_clock(clock: &str) -> u32 {
    let parts: Vec<&str> = clock.split(':').collect();
    match parts.len() {
        2 => {
            let mins = parts[0].parse::<u32>().unwrap_or(0);
            let secs = parts[1].parse::<u32>().unwrap_or(0);
            mins * 60 + secs
        }
        1 => parts[0].parse::<u32>().unwrap_or(0),
        _ => 0,
    }
}
