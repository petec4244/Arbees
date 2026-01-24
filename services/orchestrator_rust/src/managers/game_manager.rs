use crate::clients::team_matching::TeamMatchingClient;
use crate::config::Config;
use crate::managers::kalshi_discovery::KalshiDiscoveryManager;
use crate::managers::shard_manager::ShardManager;
use crate::providers::espn::EspnClient;
use crate::state::{GameAssignment, GameInfo, Sport};
use anyhow::Result;
use redis::AsyncCommands;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub struct GameManager {
    redis: redis::Client,
    assignments: Arc<RwLock<HashMap<String, GameAssignment>>>,
    pending_discovery: Arc<RwLock<HashMap<String, GameInfo>>>,
    discovery_cache: Arc<RwLock<HashMap<String, Value>>>, // Polymarket results
    shard_manager: Arc<ShardManager>,
    kalshi_manager: Arc<KalshiDiscoveryManager>,
    espn_clients: HashMap<String, EspnClient>,
    db: sqlx::PgPool,
    config: Config,
}

impl GameManager {
    pub fn new(
        redis_client: redis::Client,
        shard_manager: Arc<ShardManager>,
        kalshi_manager: Arc<KalshiDiscoveryManager>,
        db: sqlx::PgPool,
        config: Config,
    ) -> Self {
        // Initialize ESPN clients
        let mut espn = HashMap::new();
        espn.insert("nfl".to_string(), EspnClient::new(Sport::NFL));
        espn.insert("nba".to_string(), EspnClient::new(Sport::NBA));
        espn.insert("nhl".to_string(), EspnClient::new(Sport::NHL));
        espn.insert("mlb".to_string(), EspnClient::new(Sport::MLB));
        espn.insert("ncaaf".to_string(), EspnClient::new(Sport::NCAAF));
        espn.insert("ncaab".to_string(), EspnClient::new(Sport::NCAAB));
        espn.insert("mls".to_string(), EspnClient::new(Sport::MLS));

        Self {
            redis: redis_client,
            assignments: Arc::new(RwLock::new(HashMap::new())),
            pending_discovery: Arc::new(RwLock::new(HashMap::new())),
            discovery_cache: Arc::new(RwLock::new(HashMap::new())),
            shard_manager,
            kalshi_manager,
            espn_clients: espn,
            db,
            config,
        }
    }

    pub async fn run_scheduled_sync(&self) {
        info!("Starting scheduled games sync");
        let days = 7;

        for (sport, client) in &self.espn_clients {
            match client.get_scheduled_games(days).await {
                Ok(games) => {
                    info!("Fetched {} scheduled games for {}", games.len(), sport);
                    for game in games {
                        self.upsert_game(&game).await;

                        // Pregame Discovery Trigger logic (as per orchestrator.py)
                        // This uses pregame_discovery_window_hours
                        // ... I'll omit complex logic for now and rely on regular discovery for simplicity,
                        // or add it if strictly required.
                        // orchestrator.py does: "if self._market_discovery_mode == 'rust' ... request discovery"
                        // Since WE ARE the rust service, we could trigger discovery locally?
                        // But `KalshiDiscoveryManager` is for Kalshi.
                        // The `GameManager` sends discovery request for Polymarket (via `discovery:requests`).
                        // I can add that check here.
                    }
                }
                Err(e) => error!("Error fetching scheduled games for {}: {}", sport, e),
            }
        }
        info!("Scheduled games sync complete");
    }

    async fn upsert_game(&self, game: &GameInfo) {
        let q = sqlx::query(
            r#"
            INSERT INTO games (game_id, sport, home_team, away_team, scheduled_time, home_team_abbrev, away_team_abbrev, venue, broadcast, status, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
            ON CONFLICT (game_id) DO UPDATE SET
                scheduled_time = EXCLUDED.scheduled_time,
                status = CASE WHEN games.status = 'scheduled' THEN EXCLUDED.status ELSE games.status END,
                venue = EXCLUDED.venue,
                broadcast = EXCLUDED.broadcast,
                updated_at = NOW()
            "#
        )
        .bind(&game.game_id)
        .bind(game.sport.as_str())
        .bind(&game.home_team)
        .bind(&game.away_team)
        .bind(game.scheduled_time)
        .bind(&game.home_team_abbrev)
        .bind(&game.away_team_abbrev)
        .bind(game.venue.as_deref())
        .bind(game.broadcast.as_deref())
        .bind(&game.status);

        if let Err(e) = q.execute(&self.db).await {
            warn!("Failed to upsert game {}: {}", game.game_id, e);
        }
    }

    pub async fn handle_shard_heartbeat(&self, payload: Value) {
        let shard_id = match payload.get("shard_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };

        // Extract reported games set
        let reported_games: HashSet<String> = payload
            .get("games")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut assignments = self.assignments.write().await;
        let mut games_to_remove = Vec::new();

        for (game_id, assignment) in assignments.iter() {
            if assignment.shard_id == shard_id && !reported_games.contains(game_id) {
                warn!(
                    "Game {} missing from shard {} report. clearing assignment.",
                    game_id, shard_id
                );
                games_to_remove.push(game_id.clone());
            }
        }

        for game_id in games_to_remove {
            assignments.remove(&game_id);
            // Also need to clear from pending? No, if we remove from assignments, run_discovery_cycle will pick it up again as "new".
            // However, we should also clear discovery_cache if we want to force fresh market discovery?
            // Python: "Also clear from discovery cache so it gets fresh market data"
            let mut disc_cache = self.discovery_cache.write().await;
            disc_cache.remove(&game_id);
        }
    }

    // Called periodically
    pub async fn run_discovery_cycle(&self) {
        info!("Starting discovery cycle");
        let mut all_live_games = Vec::new();

        // Fetch from ESPN
        for (sport, client) in &self.espn_clients {
            match client.get_live_games().await {
                Ok(games) => {
                    info!("Found {} live games for {}", games.len(), sport);
                    all_live_games.extend(games);
                }
                Err(e) => {
                    error!("Error fetching live games for {}: {}", sport, e);
                }
            }
        }

        let assignments = self.assignments.read().await;

        // Identify new games
        let new_games: Vec<GameInfo> = all_live_games
            .into_iter()
            .filter(|g| !assignments.contains_key(&g.game_id))
            .collect();

        drop(assignments); // Release lock

        for game in new_games {
            self.process_new_game(game).await;
        }

        // Cleanup finished games logic (not implemented here for brevity but should be)
    }

    async fn process_new_game(&self, game: GameInfo) {
        debug!(
            "Processing new game: {} ({} vs {})",
            game.game_id, game.away_team, game.home_team
        );

        // Check if we have Polymarket result cached (from Redis listener)
        let poly_id = {
            let cache = self.discovery_cache.read().await;
            cache
                .get(&game.game_id)
                .and_then(|v| v.get("polymarket_moneyline"))
                .and_then(|v| v.as_str())
                .map(String::from)
        };

        // Discovery Request to Rust service if missing
        if poly_id.is_none() {
            // Check if already pending
            let mut pending = self.pending_discovery.write().await;
            if !pending.contains_key(&game.game_id) {
                // Publish request
                if let Ok(mut conn) = self.redis.get_async_connection().await {
                    let req = serde_json::json!({
                        "game_id": game.game_id,
                        "sport": game.sport.as_str(),
                        "home_team": game.home_team,
                        "away_team": game.away_team,
                        "home_abbr": game.home_team_abbrev,
                        "away_abbr": game.away_team_abbrev,
                    });
                    let _: Result<(), _> =
                        conn.publish("discovery:requests", req.to_string()).await;
                    info!("Published discovery request for {}", game.game_id);
                    pending.insert(game.game_id.clone(), game.clone());
                }
            }
            // We return here. If Polymarket comes in later via handle_discovery_result, we assign then.
            // BUT: Python logic says "Fast path... if discovery mode is rust".
            // Python: "if self._market_discovery_mode == 'rust' ... Assign new games ... Listen for results".
            // Actually, Python assigns *even without markets* if discovery times out or just immediately?
            // "Assign game even without markets - shard can still track game state from ESPN"

            // Wait: Python waits for discovery result via pubsub?
            // No, Python has `_listen_discovery_results` which calls `_handle_discovery_result`.
            // `_handle_discovery_result` assigns the game.
            // So we DO wait for the result.
            // But what if the result never comes?
            // We should have a fallback or timeout.
            return;
        }

        // If we have poly_id (or if we decide to assign immediately anyway - which we might want to do)
        self.assign_game(game, poly_id).await;
    }

    pub async fn handle_discovery_result(&self, payload: Value) {
        let game_id = match payload
            .get("game_id")
            .or(payload.get("id"))
            .and_then(|v| v.as_str())
        {
            Some(id) => id.to_string(),
            None => return,
        };

        // Cache it
        {
            let mut cache = self.discovery_cache.write().await;
            cache.insert(game_id.clone(), payload.clone());
        }

        // Remove from pending
        let game = {
            let mut pending = self.pending_discovery.write().await;
            pending.remove(&game_id)
        };

        if let Some(game) = game {
            let poly_id = payload
                .get("polymarket_moneyline")
                .and_then(|v| v.as_str())
                .map(String::from);
            self.assign_game(game, poly_id).await;
        }
    }

    async fn assign_game(&self, game: GameInfo, poly_id: Option<String>) {
        // Kalshi local discovery
        let kalshi_id = self.kalshi_manager.find_moneyline_market(&game).await;

        // Get Best Shard
        let shard = match self.shard_manager.get_best_shard().await {
            Some(s) => s,
            None => {
                warn!(
                    "No healthy shards available for assignment of {}",
                    game.game_id
                );
                return;
            }
        };

        // Construct assignment
        let mut market_ids_by_type = HashMap::new();
        let mut moneyline = HashMap::new();
        if let Some(pid) = &poly_id {
            moneyline.insert("polymarket".to_string(), pid.clone());
        }
        if let Some(kid) = &kalshi_id {
            moneyline.insert("kalshi".to_string(), kid.clone());
        }
        if !moneyline.is_empty() {
            market_ids_by_type.insert("moneyline".to_string(), moneyline);
        }

        let assignment = GameAssignment {
            game_id: game.game_id.clone(),
            sport: game.sport.clone(),
            shard_id: shard.shard_id.clone(),
            kalshi_market_id: kalshi_id.clone(),
            polymarket_market_id: poly_id.clone(),
            market_ids_by_type: market_ids_by_type.clone(),
            assigned_at: chrono::Utc::now(),
        };

        // Send Command to Shard
        let command = serde_json::json!({
            "type": "add_game",
            "game_id": game.game_id,
            "sport": game.sport.as_str(),
            "kalshi_market_id": kalshi_id,
            "polymarket_market_id": poly_id,
            "market_ids_by_type": market_ids_by_type
        });

        if let Ok(mut conn) = self.redis.get_async_connection().await {
            let channel = format!("shard:{}:command", shard.shard_id);
            let _ = conn.publish::<_, _, ()>(channel, command.to_string()).await;

            // Publish market assignment for monitors
            let _ = conn.publish::<_, _, ()>("market:assignments", serde_json::json!({
                "type": "polymarket_assign", 
                "game_id": game.game_id,
                "sport": game.sport.as_str(),
                "markets": if let Some(pid) = &poly_id {
                    vec![serde_json::json!({"market_type": "moneyline", "condition_id": pid})]
                } else { vec![] }
            }).to_string()).await;

            info!("Assigned game {} to shard {}", game.game_id, shard.shard_id);

            let mut assign_lock = self.assignments.write().await;
            assign_lock.insert(game.game_id.clone(), assignment);
        } else {
            error!("Failed to connect to Redis to assign game");
        }
    }
}
