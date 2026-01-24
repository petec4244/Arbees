use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub date: String,
    pub home_team: String,
    pub away_team: String,
    pub home_abbr: String,
    pub away_abbr: String,
}

impl EspnClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub async fn get_games(&self, sport: &str, league: &str) -> Result<Vec<Game>> {
        let url = format!("http://site.api.espn.com/apis/site/v2/sports/{}/{}/scoreboard", sport, league);
        
        let resp = self.client.get(&url).send().await?;
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
                
                let mut home_team = String::new();
                let mut away_team = String::new();
                let mut home_abbr = String::new();
                let mut away_abbr = String::new();

                if let Some(comps) = competitors {
                    for comp in comps {
                        let team = &comp["team"];
                        let team_name = team["displayName"].as_str().unwrap_or_default().to_string();
                        let team_abbr = team["abbreviation"].as_str().unwrap_or_default().to_string();
                        
                        if comp["homeAway"].as_str() == Some("home") {
                            home_team = team_name;
                            home_abbr = team_abbr;
                        } else {
                            away_team = team_name;
                            away_abbr = team_abbr;
                        }
                    }
                }

                games.push(Game {
                    id,
                    name,
                    short_name,
                    date,
                    home_team,
                    away_team,
                    home_abbr,
                    away_abbr,
                });
            }
        }

        Ok(games)
    }
}
