use crate::state::{GameInfo, Sport};
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::{error, warn};

pub struct EspnClient {
    sport: Sport,
    client: Client,
    base_url: String,
}

impl EspnClient {
    pub fn new(sport: Sport) -> Self {
        let base_url = match sport {
            Sport::NFL => "https://site.api.espn.com/apis/site/v2/sports/football/nfl",
            Sport::NBA => "https://site.api.espn.com/apis/site/v2/sports/basketball/nba",
            Sport::NHL => "https://site.api.espn.com/apis/site/v2/sports/hockey/nhl",
            Sport::MLB => "https://site.api.espn.com/apis/site/v2/sports/baseball/mlb",
            Sport::NCAAF => {
                "https://site.api.espn.com/apis/site/v2/sports/football/college-football"
            }
            Sport::NCAAB => {
                "https://site.api.espn.com/apis/site/v2/sports/basketball/mens-college-basketball"
            }
            Sport::MLS => "https://site.api.espn.com/apis/site/v2/sports/soccer/usa.1",
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        Self {
            sport,
            client,
            base_url: base_url.to_string(),
        }
    }

    pub async fn get_live_games(&self) -> Result<Vec<GameInfo>> {
        let url = format!("{}/scoreboard", self.base_url);
        let data: Value = self.fetch(&url).await?;

        let events = data.get("events").and_then(|v| v.as_array());
        let mut games = Vec::new();

        if let Some(events) = events {
            for event in events {
                if let Some(game) = self.parse_game_info(event) {
                    // Check if live (status.type.name contains "in_progress" or similar)
                    // But typically Orchestrator wants all "live" games which might include halftime/etc.
                    // The Python code checks `status.type.name` against a list or simpler heuristic.
                    // Python logic: `if game_info and game_info.is_live:`
                    // `is_live` logic: status in ["in_progress", "halftime", "end_period", "delayed"]?
                    // Let's rely on the status string we parse.
                    match game.status.as_str() {
                        "in_progress" | "halftime" | "end_period" => {
                            games.push(game);
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(games)
    }

    pub async fn get_scheduled_games(&self, days_ahead: i64) -> Result<Vec<GameInfo>> {
        let mut games = Vec::new();
        let today = Utc::now().date_naive();

        for i in 0..=days_ahead {
            let date = today + chrono::Duration::days(i);
            let date_str = date.format("%Y%m%d").to_string();
            let url = format!("{}/scoreboard?dates={}", self.base_url, date_str);

            match self.fetch(&url).await {
                Ok(data) => {
                    if let Some(events) = data.get("events").and_then(|v| v.as_array()) {
                        for event in events {
                            if let Some(game) = self.parse_game_info(event) {
                                games.push(game);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Error fetching scheduled games for {}: {}", date_str, e);
                }
            }
        }

        Ok(games)
    }

    async fn fetch(&self, url: &str) -> Result<Value> {
        // Simple retry logic
        let mut attempts = 0;
        loop {
            match self.client.get(url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let json = resp.json::<Value>().await?;
                        return Ok(json);
                    } else {
                        warn!("ESPNApi non-success status: {}", resp.status());
                    }
                }
                Err(e) => {
                    warn!("ESPNApi request error: {}", e);
                }
            }

            attempts += 1;
            if attempts >= 3 {
                anyhow::bail!("Failed to fetch from ESPN after 3 attempts");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    fn parse_game_info(&self, event: &Value) -> Option<GameInfo> {
        let competitions = event.get("competitions").and_then(|v| v.as_array());
        if competitions.is_none() || competitions.unwrap().is_empty() {
            // tracing::debug!("Skipping event: No competitions");
            return None;
        }

        let competition = &competitions.unwrap()[0];
        let competitors = competition.get("competitors").and_then(|v| v.as_array());
        if competitors.is_none() || competitors.unwrap().len() < 2 {
            // tracing::debug!("Skipping event: Not enough competitors");
            return None;
        }
        let competitors_arr = competitors.unwrap();

        // Sometimes competitors are not marked home/away explicitly if TBD
        let home = competitors_arr
            .iter()
            .find(|c| c.get("homeAway").and_then(|h| h.as_str()) == Some("home"));
        let away = competitors_arr
            .iter()
            .find(|c| c.get("homeAway").and_then(|h| h.as_str()) == Some("away"));

        if home.is_none() || away.is_none() {
            // tracing::debug!("Skipping event: Missing home/away distinction");
            // If they are missing home/away but exist (e.g. index 0/1), we could infer?
            // But let's stick to safe parsing first.
            return None;
        }
        let home = home.unwrap();
        let away = away.unwrap();

        let home_team = home.get("team");
        let away_team = away.get("team");

        if home_team.is_none() || away_team.is_none() {
            return None;
        }

        let status_type = competition.get("status").and_then(|s| s.get("type"));
        let status_name = status_type
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str());

        if status_name.is_none() {
            return None;
        }
        let status_name_str = status_name.unwrap().to_lowercase();

        let status = match status_name_str.as_str() {
            s if s.contains("in_progress") => "in_progress",
            s if s.contains("halftime") => "halftime",
            s if s.contains("end_period") => "end_period",
            s if s.contains("final") => "final",
            s if s.contains("scheduled") => "scheduled",
            s => s, // Keep original if unknown or already simplified
        }
        .to_string();

        let date_str = event.get("date").and_then(|d| d.as_str());
        if date_str.is_none() {
            // tracing::debug!("Skipping event: Missing date");
            return None;
        }

        let scheduled_time = match DateTime::parse_from_rfc3339(date_str.unwrap()) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => {
                let fallback = NaiveDateTime::parse_from_str(date_str.unwrap(), "%Y-%m-%dT%H:%MZ")
                    .or_else(|_| NaiveDateTime::parse_from_str(date_str.unwrap(), "%Y-%m-%dT%H:%M:%SZ"));
                match fallback {
                    Ok(dt) => DateTime::<Utc>::from_utc(dt, Utc),
                    Err(e) => {
                        tracing::warn!("Failed to parse date '{}': {}", date_str.unwrap(), e);
                        return None;
                    }
                }
            }
        };

        let venue = competition
            .get("venue")
            .and_then(|v| v.get("fullName"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());

        let broadcast = competition
            .get("broadcasts")
            .and_then(|b| b.as_array())
            .and_then(|arr| arr.get(0))
            .and_then(|b| b.get("names"))
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.get(0))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());

        Some(GameInfo {
            game_id: event
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string(),
            sport: self.sport.clone(),
            home_team: home_team
                .unwrap()
                .get("displayName")
                .and_then(|s| s.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            away_team: away_team
                .unwrap()
                .get("displayName")
                .and_then(|s| s.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            home_team_abbrev: home_team
                .unwrap()
                .get("abbreviation")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            away_team_abbrev: away_team
                .unwrap()
                .get("abbreviation")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            scheduled_time,
            status,
            venue,
            broadcast,
        })
    }
}
