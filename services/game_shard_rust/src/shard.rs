use anyhow::Result;
use arbees_rust_core::clients::espn::{EspnClient, Game as EspnGame};
use arbees_rust_core::models::{
    channels, GameState, Platform, SignalDirection, SignalType, Sport, TradingSignal,
};
use arbees_rust_core::redis::bus::RedisBus;
use arbees_rust_core::win_prob::calculate_win_probability;
use chrono::Utc;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

/// Minimum edge percentage to generate a signal
const MIN_EDGE_PCT: f64 = 2.0;
/// Maximum probability to buy (avoid buying near-certain outcomes)
const MAX_BUY_PROB: f64 = 0.95;
/// Minimum probability to buy (avoid buying very unlikely outcomes)
const MIN_BUY_PROB: f64 = 0.05;

#[derive(Clone)]
pub struct GameShard {
    shard_id: String,
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    games: Arc<Mutex<HashMap<String, GameEntry>>>,
    /// Shared market prices: game_id -> (team, MarketPrice)
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    max_games: usize,
    min_edge_pct: f64,
}

#[derive(Debug, Clone)]
struct MarketPriceData {
    pub market_id: String,
    pub platform: String,
    pub contract_team: String,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub mid_price: f64,
    pub timestamp: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameContext {
    pub game_id: String,
    pub sport: String,
    pub polymarket_id: Option<String>,
    pub kalshi_id: Option<String>,
}

struct GameEntry {
    context: GameContext,
    task: tokio::task::JoinHandle<()>,
    /// Last calculated home win probability
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
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

/// Incoming market price message from polymarket_monitor
#[derive(Debug, Deserialize)]
struct IncomingMarketPrice {
    market_id: String,
    platform: String,
    game_id: String,
    contract_team: Option<String>,
    yes_bid: f64,
    yes_ask: f64,
    mid_price: Option<f64>,
    implied_probability: Option<f64>,
    timestamp: Option<String>,
}

impl GameShard {
    pub async fn new(shard_id: String) -> Result<Self> {
        let redis = RedisBus::new().await?;
        let espn = EspnClient::new();

        // Create database pool
        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://arbees:arbees@localhost:5432/arbees".to_string());
        let db_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        info!("Connected to database");

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
        let min_edge_pct = env::var("MIN_EDGE_PCT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(MIN_EDGE_PCT);

        Ok(Self {
            shard_id,
            redis,
            espn,
            db_pool,
            games: Arc::new(Mutex::new(HashMap::new())),
            market_prices: Arc::new(RwLock::new(HashMap::new())),
            poll_interval,
            heartbeat_interval,
            max_games,
            min_edge_pct,
        })
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting GameShard {}", self.shard_id);

        // Heartbeat loop
        let heartbeat_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = heartbeat_shard.heartbeat_loop().await {
                error!("Heartbeat loop exited: {}", e);
            }
        });

        // Command loop (receives game assignments)
        let command_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = command_shard.command_loop().await {
                error!("Command loop exited: {}", e);
            }
        });

        // Market price listener (subscribes to game:*:price)
        let price_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = price_shard.price_listener_loop().await {
                error!("Price listener loop exited: {}", e);
            }
        });

        Ok(())
    }

    pub async fn add_game(
        &self,
        game_id: String,
        sport: String,
        polymarket_id: Option<String>,
        kalshi_id: Option<String>,
    ) -> Result<()> {
        info!("Adding game: {} ({})", game_id, sport);

        let mut games = self.games.lock().await;
        if games.contains_key(&game_id) {
            warn!("Game already tracked: {}", game_id);
            return Ok(());
        }

        let context = GameContext {
            game_id: game_id.clone(),
            sport: sport.clone(),
            polymarket_id,
            kalshi_id,
        };

        let last_prob = Arc::new(RwLock::new(None));
        let last_prob_clone = last_prob.clone();
        let redis = self.redis.clone();
        let espn = self.espn.clone();
        let db_pool = self.db_pool.clone();
        let poll_interval = self.poll_interval;
        let market_prices = self.market_prices.clone();
        let min_edge = self.min_edge_pct;
        let gid = game_id.clone();
        let sp = sport.clone();

        let task = tokio::spawn(async move {
            monitor_game(
                redis,
                espn,
                db_pool,
                gid,
                sp,
                poll_interval,
                last_prob_clone,
                market_prices,
                min_edge,
            )
            .await;
        });

        games.insert(
            context.game_id.clone(),
            GameEntry {
                context,
                task,
                last_home_win_prob: last_prob,
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
        // Also remove market prices
        let mut prices = self.market_prices.write().await;
        prices.remove(&game_id);
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
                        if let Err(e) = self
                            .add_game(
                                game_id,
                                sport,
                                command.polymarket_market_id,
                                command.kalshi_market_id,
                            )
                            .await
                        {
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

    /// Listen for market price updates from polymarket_monitor
    async fn price_listener_loop(&self) -> Result<()> {
        // Subscribe to game:*:price pattern
        let mut pubsub = self.redis.psubscribe("game:*:price").await?;
        info!("Subscribed to game:*:price pattern");

        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let channel: String = match msg.get_channel::<String>() {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Extract game_id from channel: game:{game_id}:price
            let game_id = channel
                .strip_prefix("game:")
                .and_then(|s| s.strip_suffix(":price"))
                .map(|s| s.to_string());

            let game_id = match game_id {
                Some(gid) => gid,
                None => continue,
            };

            let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Try to parse as msgpack first, then JSON
            let price: IncomingMarketPrice = match rmp_serde::from_slice(&payload) {
                Ok(p) => p,
                Err(_) => match serde_json::from_slice(&payload) {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("Failed to parse price message: {}", e);
                        continue;
                    }
                },
            };

            // Store the price
            if let Some(team) = &price.contract_team {
                let mid = price
                    .mid_price
                    .or(price.implied_probability)
                    .unwrap_or((price.yes_bid + price.yes_ask) / 2.0);

                let data = MarketPriceData {
                    market_id: price.market_id.clone(),
                    platform: price.platform.clone(),
                    contract_team: team.clone(),
                    yes_bid: price.yes_bid,
                    yes_ask: price.yes_ask,
                    mid_price: mid,
                    timestamp: Utc::now(),
                };

                let mut prices = self.market_prices.write().await;
                let game_prices = prices.entry(game_id.clone()).or_insert_with(HashMap::new);
                game_prices.insert(team.clone(), data);
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
    db_pool: PgPool,
    game_id: String,
    sport: String,
    poll_interval: Duration,
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    min_edge_pct: f64,
) {
    let sport_enum = match parse_sport(&sport) {
        Some(s) => s,
        None => {
            warn!("Unsupported sport: {}", sport);
            return;
        }
    };

    // Signal debouncing: (team, direction) -> last_signal_time
    let mut last_signal_times: HashMap<(String, String), Instant> = HashMap::new();
    let signal_debounce_secs: u64 = env::var("SIGNAL_DEBOUNCE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    loop {
        // Fetch game state from ESPN
        if let Some((game, state)) = fetch_game_state(&espn, &game_id, &sport).await {
            // Calculate win probability
            let home_win_prob = calculate_win_probability(&state, true);

            // Format time remaining as string
            let time_remaining_str = format_time_remaining(game.time_remaining_seconds);

            // Insert into database
            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO game_states (game_id, sport, home_score, away_score, period, time_remaining, status, possession, home_win_prob, time)
                VALUES ($1, $2::sport_enum, $3, $4, $5, $6, $7, $8, $9, NOW())
                "#,
            )
            .bind(&game.id)
            .bind(&sport.to_lowercase())
            .bind(game.home_score as i32)
            .bind(game.away_score as i32)
            .bind(game.period as i32)
            .bind(&time_remaining_str)
            .bind(&game.status)
            .bind(&game.possession)
            .bind(home_win_prob)
            .execute(&db_pool)
            .await
            {
                warn!("Database insert error: {}", e);
            }

            // Publish game state to Redis
            let state_channel = format!("game:{}:state", game_id);
            let state_json = json!({
                "game_id": game.id,
                "sport": sport,
                "name": game.name,
                "short_name": game.short_name,
                "scheduled_time": game.date,
                "home_team": game.home_team,
                "away_team": game.away_team,
                "home_abbr": game.home_abbr,
                "away_abbr": game.away_abbr,
                "home_score": game.home_score,
                "away_score": game.away_score,
                "period": game.period,
                "time_remaining": game.time_remaining_seconds,
                "status": game.status,
                "source": "espn_scoreboard",
                "timestamp": Utc::now().to_rfc3339(),
            });

            if let Err(e) = redis.publish(&state_channel, &state_json).await {
                warn!("Game state publish error: {}", e);
            }

            // Check for signals
            let old_prob = *last_home_win_prob.read().await;

            // Update last probability
            *last_home_win_prob.write().await = Some(home_win_prob);

            // Skip signal generation if game is not in progress
            if game.status != "STATUS_IN_PROGRESS" && game.status != "in" {
                continue;
            }

            // Skip signal generation if game is in overtime (too volatile)
            if is_overtime(sport_enum, game.period) {
                debug!("OVERTIME: Skipping signals for {} (period {})", game_id, game.period);
                continue;
            }

            // Get market prices for this game
            let prices = market_prices.read().await;
            if let Some(game_prices) = prices.get(&game_id) {
                // Check home team
                if let Some(home_price) = find_team_price(game_prices, &game.home_team) {
                    // Pre-calculate direction for debounce check
                    let home_edge = (home_win_prob - home_price.mid_price) * 100.0;
                    let home_direction = if home_edge > 0.0 { "buy" } else { "sell" };
                    let home_key = (game.home_team.clone(), home_direction.to_string());

                    // Check debounce
                    let should_emit_home = match last_signal_times.get(&home_key) {
                        Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                        None => true,
                    };

                    if should_emit_home && home_edge.abs() >= min_edge_pct {
                        if check_and_emit_signal(
                            &redis,
                            &game_id,
                            sport_enum,
                            &game.home_team,
                            home_win_prob,
                            home_price,
                            old_prob,
                            min_edge_pct,
                        )
                        .await
                        {
                            last_signal_times.insert(home_key, Instant::now());
                        }
                    } else if !should_emit_home && home_edge.abs() >= min_edge_pct {
                        debug!(
                            "DEBOUNCE: {} {} - {}s remaining",
                            game.home_team,
                            home_direction,
                            signal_debounce_secs.saturating_sub(
                                last_signal_times.get(&home_key).map(|t| t.elapsed().as_secs()).unwrap_or(0)
                            )
                        );
                    }
                }

                // Check away team
                let away_win_prob = 1.0 - home_win_prob;
                if let Some(away_price) = find_team_price(game_prices, &game.away_team) {
                    // Pre-calculate direction for debounce check
                    let away_edge = (away_win_prob - away_price.mid_price) * 100.0;
                    let away_direction = if away_edge > 0.0 { "buy" } else { "sell" };
                    let away_key = (game.away_team.clone(), away_direction.to_string());

                    // Check debounce
                    let should_emit_away = match last_signal_times.get(&away_key) {
                        Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                        None => true,
                    };

                    if should_emit_away && away_edge.abs() >= min_edge_pct {
                        if check_and_emit_signal(
                            &redis,
                            &game_id,
                            sport_enum,
                            &game.away_team,
                            away_win_prob,
                            away_price,
                            old_prob.map(|p| 1.0 - p),
                            min_edge_pct,
                        )
                        .await
                        {
                            last_signal_times.insert(away_key, Instant::now());
                        }
                    } else if !should_emit_away && away_edge.abs() >= min_edge_pct {
                        debug!(
                            "DEBOUNCE: {} {} - {}s remaining",
                            game.away_team,
                            away_direction,
                            signal_debounce_secs.saturating_sub(
                                last_signal_times.get(&away_key).map(|t| t.elapsed().as_secs()).unwrap_or(0)
                            )
                        );
                    }
                }
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

fn find_team_price<'a>(
    prices: &'a HashMap<String, MarketPriceData>,
    team: &str,
) -> Option<&'a MarketPriceData> {
    // Try exact match first
    if let Some(price) = prices.get(team) {
        return Some(price);
    }
    // Try case-insensitive partial match
    let team_lower = team.to_lowercase();
    for (key, price) in prices {
        if key.to_lowercase().contains(&team_lower) || team_lower.contains(&key.to_lowercase()) {
            return Some(price);
        }
    }
    None
}

/// Returns true if a signal was emitted, false otherwise
async fn check_and_emit_signal(
    redis: &RedisBus,
    game_id: &str,
    sport: Sport,
    team: &str,
    model_prob: f64,
    market_price: &MarketPriceData,
    _old_prob: Option<f64>,
    min_edge_pct: f64,
) -> bool {
    let market_prob = market_price.mid_price;

    // Calculate edge: model_prob - market_prob (as percentage)
    let edge_pct = (model_prob - market_prob) * 100.0;

    // Skip if edge is too small
    if edge_pct.abs() < min_edge_pct {
        return false;
    }

    // Determine direction
    let (direction, signal_type) = if edge_pct > 0.0 {
        // Model thinks team is undervalued -> BUY
        if model_prob > MAX_BUY_PROB {
            debug!(
                "Skipping buy signal for {} - prob too high: {:.1}%",
                team,
                model_prob * 100.0
            );
            return false;
        }
        (SignalDirection::Buy, SignalType::ModelEdgeYes)
    } else {
        // Model thinks team is overvalued -> SELL
        if model_prob < MIN_BUY_PROB {
            debug!(
                "Skipping sell signal for {} - prob too low: {:.1}%",
                team,
                model_prob * 100.0
            );
            return false;
        }
        (SignalDirection::Sell, SignalType::ModelEdgeNo)
    };

    // Create signal
    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction,
        model_prob,
        market_prob: Some(market_prob),
        edge_pct: edge_pct.abs(),
        confidence: (edge_pct.abs() / 10.0).min(1.0), // Simple confidence based on edge size
        platform_buy: Some(Platform::Polymarket),
        platform_sell: None,
        buy_price: Some(market_price.yes_ask),
        sell_price: Some(market_price.yes_bid),
        liquidity_available: 10000.0, // TODO: Get actual liquidity
        reason: format!(
            "Model: {:.1}% vs Market: {:.1}% = {:.1}% edge",
            model_prob * 100.0,
            market_prob * 100.0,
            edge_pct.abs()
        ),
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::seconds(30)),
        play_id: None,
    };

    // Format direction as "to win" / "to lose" for clarity
    let direction_str = match direction {
        SignalDirection::Buy => "to win",
        SignalDirection::Sell => "to lose",
        SignalDirection::Hold => "hold",
    };
    info!(
        "SIGNAL: {} {} - model={:.1}% market={:.1}% edge={:.1}%",
        team,
        direction_str,
        model_prob * 100.0,
        market_prob * 100.0,
        edge_pct.abs()
    );

    // Publish signal
    match redis.publish(channels::SIGNALS_NEW, &signal).await {
        Ok(_) => true,
        Err(e) => {
            error!("Failed to publish signal: {}", e);
            false
        }
    }
}

async fn fetch_game_state(
    espn: &EspnClient,
    game_id: &str,
    sport: &str,
) -> Option<(EspnGame, GameState)> {
    let (espn_sport, espn_league) = espn_sport_league(sport)?;

    let games = match espn.get_games(espn_sport, espn_league).await {
        Ok(g) => g,
        Err(e) => {
            warn!("ESPN fetch error: {}", e);
            return None;
        }
    };

    let game = games.into_iter().find(|g| g.id == game_id)?;

    let sport_enum = parse_sport(sport)?;

    let state = GameState {
        game_id: game.id.clone(),
        sport: sport_enum,
        home_team: game.home_team.clone(),
        away_team: game.away_team.clone(),
        home_score: game.home_score,
        away_score: game.away_score,
        period: game.period,
        time_remaining_seconds: game.time_remaining_seconds,
        possession: game.possession.clone(),
        down: game.down,
        yards_to_go: game.yards_to_go,
        yard_line: game.yard_line,
        is_redzone: game.is_redzone,
    };

    Some((game, state))
}

fn parse_sport(sport: &str) -> Option<Sport> {
    match sport.to_lowercase().as_str() {
        "nfl" => Some(Sport::NFL),
        "ncaaf" => Some(Sport::NCAAF),
        "nba" => Some(Sport::NBA),
        "ncaab" => Some(Sport::NCAAB),
        "nhl" => Some(Sport::NHL),
        "mlb" => Some(Sport::MLB),
        "mls" => Some(Sport::MLS),
        "soccer" => Some(Sport::Soccer),
        "tennis" => Some(Sport::Tennis),
        "mma" => Some(Sport::MMA),
        _ => None,
    }
}

/// Check if a game is in overtime based on sport and period
/// Returns true if the game has exceeded regular periods/innings
fn is_overtime(sport: Sport, period: u8) -> bool {
    match sport {
        Sport::NHL => period > 3,       // Regular NHL: 3 periods
        Sport::NBA => period > 4,       // Regular NBA: 4 quarters
        Sport::NFL => period > 4,       // Regular NFL: 4 quarters
        Sport::NCAAF => period > 4,     // Regular NCAAF: 4 quarters
        Sport::NCAAB => period > 2,     // Regular NCAAB: 2 halves
        Sport::MLB => period > 9,       // Regular MLB: 9 innings
        Sport::MLS | Sport::Soccer => period > 2, // Regular soccer: 2 halves
        Sport::Tennis => false,         // Tennis doesn't have overtime
        Sport::MMA => false,            // MMA doesn't have overtime
    }
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

/// Format seconds into a time remaining string like "12:34" or "5:00"
fn format_time_remaining(seconds: u32) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}
