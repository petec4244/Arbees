use anyhow::Result;
use arbees_rust_core::clients::{
    espn::EspnClient,
    kalshi::KalshiClient,
    polymarket::PolymarketClient,
};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use futures_util::StreamExt;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct GameShard {
    shard_id: String,
    redis: RedisBus,
    espn: EspnClient,
    kalshi: KalshiClient,
    polymarket: PolymarketClient,
    games: Arc<Mutex<HashMap<String, GameEntry>>>,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    max_games: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameContext {
    pub game_id: String,
    pub sport: String,
    // Add more fields as needed: poll intervals, last state, etc.
}

struct GameEntry {
    context: GameContext,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Deserialize)]
struct ShardCommand {
    #[serde(rename = "type")]
    command_type: String,
    game_id: Option<String>,
    sport: Option<String>,
    kalshi_market_id: Option<String>,
    polymarket_market_id: Option<String>,
}

impl GameShard {
    pub async fn new(shard_id: String) -> Result<Self> {
        let redis = RedisBus::new().await?;
        let espn = EspnClient::new();
        let kalshi = KalshiClient::new();
        let polymarket = PolymarketClient::new();
        let poll_interval = Duration::from_secs_f64(
            env::var("POLL_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(1.0),
        );
        let heartbeat_interval = Duration::from_secs(
            env::var("HEARTBEAT_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10),
        );
        let max_games = env::var("MAX_GAMES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20);

        Ok(Self {
            shard_id,
            redis,
            espn,
            kalshi,
            polymarket,
            games: Arc::new(Mutex::new(HashMap::new())),
            poll_interval,
            heartbeat_interval,
            max_games,
        })
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting GameShard {}", self.shard_id);

        let heartbeat_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = heartbeat_shard.heartbeat_loop().await {
                error!("Heartbeat loop exited: {}", e);
            }
        });

        let command_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = command_shard.command_loop().await {
                error!("Command loop exited: {}", e);
            }
        });

        Ok(())
    }

    pub async fn add_game(&self, game_id: String, sport: String) -> Result<()> {
        info!("Adding game: {} ({})", game_id, sport);

        let mut games = self.games.lock().await;
        if games.contains_key(&game_id) {
            warn!("Game already tracked: {}", game_id);
            return Ok(());
        }

        let context = GameContext {
            game_id: game_id.clone(),
            sport: sport.clone(),
        };

        let redis = self.redis.clone();
        let espn = self.espn.clone();
        let poll_interval = self.poll_interval;
        let task = tokio::spawn(async move {
            monitor_game(redis, espn, game_id, sport, poll_interval).await;
        });

        games.insert(
            context.game_id.clone(),
            GameEntry {
                context,
                task,
            },
        );

        Ok(())
    }

    pub async fn remove_game(&self, game_id: String) -> Result<()> {
        info!("Removing game: {}", game_id);
        let mut games = self.games.lock().await;
        if let Some(entry) = games.remove(&game_id) {
            entry.task.abort();
        }
        Ok(())
    }

    async fn command_loop(&self) -> Result<()> {
        let channel = format!("shard:{}:command", self.shard_id);
        let mut pubsub = self.redis.subscribe(&channel).await?;
        info!("Subscribed to {}", channel);

        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
                Ok(p) => p,
                Err(e) => {
                    warn!("Command payload read error: {}", e);
                    continue;
                }
            };

            let command: ShardCommand = match serde_json::from_slice(&payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Command JSON parse error: {}", e);
                    continue;
                }
            };

            match command.command_type.as_str() {
                "add_game" => {
                    if let (Some(game_id), Some(sport)) =
                        (command.game_id.clone(), command.sport.clone())
                    {
                        info!(
                            "Received add_game: {} ({}) kalshi={:?} poly={:?}",
                            game_id, sport, command.kalshi_market_id, command.polymarket_market_id
                        );
                        if let Err(e) = self.add_game(game_id, sport).await {
                            error!("Failed to add_game: {}", e);
                        }
                    } else {
                        warn!("add_game command missing game_id or sport");
                    }
                }
                "remove_game" => {
                    if let Some(game_id) = command.game_id.clone() {
                        if let Err(e) = self.remove_game(game_id).await {
                            error!("Failed to remove_game: {}", e);
                        }
                    } else {
                        warn!("remove_game command missing game_id");
                    }
                }
                other => {
                    warn!("Unknown command type: {}", other);
                }
            }
        }

        Ok(())
    }

    async fn heartbeat_loop(&self) -> Result<()> {
        let channel = format!("shard:{}:heartbeat", self.shard_id);
        loop {
            let (game_ids, count) = {
                let games = self.games.lock().await;
                let ids = games.keys().cloned().collect::<Vec<_>>();
                (ids, games.len())
            };

            let payload = json!({
                "shard_id": self.shard_id,
                "game_count": count,
                "max_games": self.max_games,
                "games": game_ids,
                "timestamp": Utc::now().to_rfc3339(),
            });

            if let Err(e) = self.redis.publish(&channel, &payload).await {
                warn!("Heartbeat publish error: {}", e);
            }

            tokio::time::sleep(self.heartbeat_interval).await;
        }
    }
}

async fn monitor_game(
    redis: RedisBus,
    espn: EspnClient,
    game_id: String,
    sport: String,
    poll_interval: Duration,
) {
    loop {
        if let Some(state) = fetch_game_state(&espn, &game_id, &sport).await {
            let channel = format!("game:{}:state", game_id);
            if let Err(e) = redis.publish(&channel, &state).await {
                warn!("Game state publish error: {}", e);
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

async fn fetch_game_state(
    espn: &EspnClient,
    game_id: &str,
    sport: &str,
) -> Option<serde_json::Value> {
    let (espn_sport, espn_league) = match espn_sport_league(sport) {
        Some(v) => v,
        None => {
            warn!("Unsupported sport for ESPN polling: {}", sport);
            return None;
        }
    };

    let games = match espn.get_games(espn_sport, espn_league).await {
        Ok(g) => g,
        Err(e) => {
            warn!("ESPN fetch error: {}", e);
            return None;
        }
    };

    let game = games.into_iter().find(|g| g.id == game_id)?;
    Some(json!({
        "game_id": game.id,
        "sport": sport,
        "name": game.name,
        "short_name": game.short_name,
        "scheduled_time": game.date,
        "home_team": game.home_team,
        "away_team": game.away_team,
        "home_abbr": game.home_abbr,
        "away_abbr": game.away_abbr,
        "source": "espn_scoreboard",
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

fn espn_sport_league(sport: &str) -> Option<(&'static str, &'static str)> {
    match sport.to_lowercase().as_str() {
        "nfl" => Some(("football", "nfl")),
        "ncaaf" => Some(("football", "college-football")),
        "nba" => Some(("basketball", "nba")),
        "ncaab" => Some(("basketball", "mens-college-basketball")),
        "nhl" => Some(("hockey", "nhl")),
        "mlb" => Some(("baseball", "mlb")),
        "mls" => Some(("soccer", "usa.1")),
        "soccer" => Some(("soccer", "eng.1")),
        _ => None,
    }
}
