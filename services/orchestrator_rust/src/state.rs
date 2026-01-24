use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Sport {
    NFL,
    NBA,
    NHL,
    MLB,
    NCAAF,
    NCAAB,
    MLS,
}

impl Sport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sport::NFL => "nfl",
            Sport::NBA => "nba",
            Sport::NHL => "nhl",
            Sport::MLB => "mlb",
            Sport::NCAAF => "ncaaf",
            Sport::NCAAB => "ncaab",
            Sport::MLS => "mls",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameInfo {
    pub game_id: String,
    pub sport: Sport,
    pub home_team: String,
    pub away_team: String,
    pub home_team_abbrev: String,
    pub away_team_abbrev: String,
    pub scheduled_time: DateTime<Utc>,
    pub status: String,
    pub venue: Option<String>,
    pub broadcast: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardInfo {
    pub shard_id: String,
    pub game_count: usize,
    pub max_games: usize,
    pub games: Vec<String>,
    pub last_heartbeat: DateTime<Utc>,
    pub is_healthy: bool,
}

impl ShardInfo {
    pub fn available_capacity(&self) -> usize {
        if self.game_count >= self.max_games {
            0
        } else {
            self.max_games - self.game_count
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameAssignment {
    pub game_id: String,
    pub sport: Sport,
    pub shard_id: String,
    pub kalshi_market_id: Option<String>,
    pub polymarket_market_id: Option<String>,
    pub market_ids_by_type: HashMap<String, HashMap<String, String>>, // market_type -> platform -> id
    pub assigned_at: DateTime<Utc>,
}
