use anyhow::Result;
use arbees_rust_core::atomic_orderbook::kalshi_fee_cents;
use arbees_rust_core::clients::espn::{EspnClient, Game as EspnGame};
use arbees_rust_core::models::{
    channels, GameState, Platform, SignalDirection, SignalType, Sport, TradingSignal,
};
use arbees_rust_core::redis::bus::RedisBus;
use arbees_rust_core::simd::{check_arbs_simd, calculate_profit_cents, decode_arb_mask, ARB_POLY_YES_KALSHI_NO, ARB_KALSHI_YES_POLY_NO};
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

/// Default minimum edge percentage to generate a signal (can be overridden via MIN_EDGE_PCT env var)
/// Matches signal_processor_rust default (5.0%) to ensure consistency across services
const DEFAULT_MIN_EDGE_PCT: f64 = 5.0;
/// Maximum probability to buy (avoid buying near-certain outcomes)
const MAX_BUY_PROB: f64 = 0.95;
/// Minimum probability to buy (avoid buying very unlikely outcomes)
const MIN_BUY_PROB: f64 = 0.05;
/// Polymarket fee rate (2% per side)
const POLYMARKET_FEE_RATE: f64 = 0.02;

/// Calculate fee-adjusted edge percentage, accounting for round-trip trading fees.
///
/// For buying (model_prob > market_prob):
///   - Entry cost: market_ask + entry_fee
///   - Expected exit at model_prob with exit_fee
///   - Net edge = model_prob - market_ask - entry_fee - exit_fee
///
/// For selling (model_prob < market_prob):
///   - Entry proceeds: market_bid - entry_fee (selling YES)
///   - Expected cost at model_prob with exit_fee
///   - Net edge = market_bid - model_prob - entry_fee - exit_fee
fn calculate_fee_adjusted_edge(
    market_price: f64,
    model_prob: f64,
    platform: Platform,
) -> f64 {
    // Calculate entry fee based on the market price we're entering at
    let entry_price_cents = (market_price * 100.0).round() as u16;
    let entry_fee = match platform {
        Platform::Kalshi | Platform::Paper => {
            kalshi_fee_cents(entry_price_cents) as f64 / 100.0
        }
        Platform::Polymarket => market_price * POLYMARKET_FEE_RATE,
    };

    // Calculate exit fee based on expected exit price (model probability)
    let exit_price_cents = (model_prob * 100.0).round() as u16;
    let exit_fee = match platform {
        Platform::Kalshi | Platform::Paper => {
            kalshi_fee_cents(exit_price_cents) as f64 / 100.0
        }
        Platform::Polymarket => model_prob * POLYMARKET_FEE_RATE,
    };

    // Gross edge before fees
    let gross_edge = model_prob - market_price;

    // Net edge after round-trip fees (as percentage)
    (gross_edge - entry_fee - exit_fee) * 100.0
}

#[derive(Clone)]
pub struct GameShard {
    shard_id: String,
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    games: Arc<Mutex<HashMap<String, GameEntry>>>,
    /// Shared market prices: game_id -> (team|platform -> MarketPrice)
    /// Key format for inner map: "{team}|{platform}" to support prices from multiple platforms
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
    /// Liquidity available at the yes bid (contracts available to sell)
    pub yes_bid_size: Option<f64>,
    /// Liquidity available at the yes ask (contracts available to buy)
    pub yes_ask_size: Option<f64>,
    /// Total liquidity in the market (if reported)
    pub total_liquidity: Option<f64>,
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
    /// Liquidity at the yes bid (contracts available to sell)
    yes_bid_size: Option<f64>,
    /// Liquidity at the yes ask (contracts available to buy)
    yes_ask_size: Option<f64>,
    /// Total market liquidity (optional)
    liquidity: Option<f64>,
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
            .unwrap_or(DEFAULT_MIN_EDGE_PCT);

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

            // Store the price (but filter out invalid prices with no real liquidity)
            if let Some(team) = &price.contract_team {
                // Skip prices with no real liquidity (bid=0, ask=1 gives fake 50% mid)
                let has_liquidity = price.yes_bid > 0.01 || price.yes_ask < 0.99;
                if !has_liquidity {
                    debug!(
                        "Skipping price with no liquidity: game={} team={} bid={} ask={}",
                        game_id, team, price.yes_bid, price.yes_ask
                    );
                    continue;
                }

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
                    yes_bid_size: price.yes_bid_size,
                    yes_ask_size: price.yes_ask_size,
                    total_liquidity: price.liquidity,
                };

                let mut prices = self.market_prices.write().await;
                let game_prices = prices.entry(game_id.clone()).or_insert_with(HashMap::new);
                // Store with team|platform key to support multiple platforms per team
                let key = format!("{}|{}", team, price.platform.to_lowercase());
                game_prices.insert(key, data);
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

            // Skip signal generation if score is 0-0 (no real game information yet)
            // At 0-0, our model only has home advantage - not team strength
            // This leads to bad signals against favored away teams
            if game.home_score == 0 && game.away_score == 0 {
                debug!("SCORELESS: Skipping signals for {} (0-0)", game_id);
                continue;
            }

            // Get market prices for this game
            // FIX: Only emit ONE signal per game - the team with the strongest edge
            // This prevents betting on both teams to win the same game!
            let prices = market_prices.read().await;
            if let Some(game_prices) = prices.get(&game_id) {
                // Get prices from both platforms for each team
                let (home_kalshi, home_poly) = find_team_prices(game_prices, &game.home_team);
                let (away_kalshi, away_poly) = find_team_prices(game_prices, &game.away_team);

                // ===== CROSS-PLATFORM ARBITRAGE CHECK (SIMD) =====
                // Check for arbitrage opportunities when we have prices from both platforms
                // Arbs are higher priority than model-edge signals (guaranteed profit)
                const MIN_ARB_PROFIT_CENTS: i16 = 1;

                if let Some((arb_mask, profit)) = check_cross_platform_arb(home_kalshi, home_poly, MIN_ARB_PROFIT_CENTS) {
                    let arb_key = (game.home_team.clone(), "arb".to_string());
                    let should_emit_arb = match last_signal_times.get(&arb_key) {
                        Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                        None => true,
                    };
                    if should_emit_arb {
                        if emit_arb_signal(
                            &redis, &game_id, sport_enum, &game.home_team,
                            arb_mask, profit, home_kalshi.unwrap(), home_poly.unwrap()
                        ).await {
                            last_signal_times.insert(arb_key, Instant::now());
                        }
                    }
                }

                if let Some((arb_mask, profit)) = check_cross_platform_arb(away_kalshi, away_poly, MIN_ARB_PROFIT_CENTS) {
                    let arb_key = (game.away_team.clone(), "arb".to_string());
                    let should_emit_arb = match last_signal_times.get(&arb_key) {
                        Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                        None => true,
                    };
                    if should_emit_arb {
                        if emit_arb_signal(
                            &redis, &game_id, sport_enum, &game.away_team,
                            arb_mask, profit, away_kalshi.unwrap(), away_poly.unwrap()
                        ).await {
                            last_signal_times.insert(arb_key, Instant::now());
                        }
                    }
                }
                // ===== END ARBITRAGE CHECK =====

                // Select best price (lowest ask) for each team
                let home_best = select_best_price(home_kalshi, home_poly);
                let away_best = select_best_price(away_kalshi, away_poly);

                let away_win_prob = 1.0 - home_win_prob;

                // Calculate edges for both teams using best available price
                let home_edge_data = home_best.map(|(hp, platform)| {
                    let edge = (home_win_prob - hp.mid_price) * 100.0;
                    (edge, hp, platform, home_win_prob, &game.home_team, old_prob)
                });

                let away_edge_data = away_best.map(|(ap, platform)| {
                    let edge = (away_win_prob - ap.mid_price) * 100.0;
                    (edge, ap, platform, away_win_prob, &game.away_team, old_prob.map(|p| 1.0 - p))
                });

                // Determine which team has stronger edge (by absolute value)
                let stronger_signal = match (home_edge_data, away_edge_data) {
                    (Some(home), Some(away)) => {
                        if home.0.abs() >= away.0.abs() {
                            Some(home)
                        } else {
                            Some(away)
                        }
                    }
                    (Some(home), None) => Some(home),
                    (None, Some(away)) => Some(away),
                    (None, None) => None,
                };

                // Only emit signal for the team with stronger edge, using the best platform
                if let Some((edge, price, platform, prob, team, old_p)) = stronger_signal {
                    if edge.abs() >= min_edge_pct {
                        let direction = if edge > 0.0 { "buy" } else { "sell" };
                        let signal_key = (team.clone(), direction.to_string());

                        // Check debounce
                        let should_emit = match last_signal_times.get(&signal_key) {
                            Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                            None => true,
                        };

                        if should_emit {
                            // Log platform selection for debugging
                            debug!(
                                "Selected {:?} for {} (kalshi={}, poly={})",
                                platform,
                                team,
                                home_kalshi.map(|p| format!("{:.3}", p.yes_ask)).unwrap_or_else(|| "N/A".to_string()),
                                home_poly.map(|p| format!("{:.3}", p.yes_ask)).unwrap_or_else(|| "N/A".to_string())
                            );

                            if check_and_emit_signal(
                                &redis,
                                &game_id,
                                sport_enum,
                                team,
                                prob,
                                price,
                                platform,
                                old_p,
                                min_edge_pct,
                            )
                            .await
                            {
                                last_signal_times.insert(signal_key, Instant::now());
                            }
                        } else {
                            debug!(
                                "DEBOUNCE: {} {} - {}s remaining",
                                team,
                                direction,
                                signal_debounce_secs.saturating_sub(
                                    last_signal_times.get(&signal_key).map(|t| t.elapsed().as_secs()).unwrap_or(0)
                                )
                            );
                        }
                    }
                }
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Find all platform prices for a team (returns up to one price per platform)
fn find_team_prices<'a>(
    prices: &'a HashMap<String, MarketPriceData>,
    team: &str,
) -> (Option<&'a MarketPriceData>, Option<&'a MarketPriceData>) {
    let team_lower = team.to_lowercase();
    let mut kalshi_price: Option<&MarketPriceData> = None;
    let mut poly_price: Option<&MarketPriceData> = None;

    for (key, price) in prices {
        // Parse key: "team|platform"
        let parts: Vec<&str> = key.split('|').collect();
        if parts.is_empty() {
            continue;
        }
        let price_team = parts[0].to_lowercase();
        let platform = parts.get(1).map(|p| p.to_lowercase()).unwrap_or_default();

        // Check if team matches (exact or partial)
        let team_matches = price_team.contains(&team_lower) || team_lower.contains(&price_team);
        if !team_matches {
            continue;
        }

        // Assign to appropriate platform slot
        if platform.contains("kalshi") && kalshi_price.is_none() {
            kalshi_price = Some(price);
        } else if platform.contains("polymarket") && poly_price.is_none() {
            poly_price = Some(price);
        }

        // Early exit if we found both
        if kalshi_price.is_some() && poly_price.is_some() {
            break;
        }
    }

    (kalshi_price, poly_price)
}

/// Select the best price from available platform prices (lowest ask for buying)
fn select_best_price<'a>(
    kalshi_price: Option<&'a MarketPriceData>,
    poly_price: Option<&'a MarketPriceData>,
) -> Option<(&'a MarketPriceData, Platform)> {
    match (kalshi_price, poly_price) {
        (Some(k), Some(p)) => {
            // For buying, prefer lower ask price
            if k.yes_ask <= p.yes_ask {
                Some((k, Platform::Kalshi))
            } else {
                Some((p, Platform::Polymarket))
            }
        }
        (Some(k), None) => Some((k, Platform::Kalshi)),
        (None, Some(p)) => Some((p, Platform::Polymarket)),
        (None, None) => None,
    }
}

/// Legacy function for backward compatibility - returns best price among all platforms
fn find_team_price<'a>(
    prices: &'a HashMap<String, MarketPriceData>,
    team: &str,
) -> Option<&'a MarketPriceData> {
    let (kalshi, poly) = find_team_prices(prices, team);
    select_best_price(kalshi, poly).map(|(p, _)| p)
}

/// Returns true if a signal was emitted, false otherwise
async fn check_and_emit_signal(
    redis: &RedisBus,
    game_id: &str,
    sport: Sport,
    team: &str,
    model_prob: f64,
    market_price: &MarketPriceData,
    selected_platform: Platform,
    _old_prob: Option<f64>,
    min_edge_pct: f64,
) -> bool {
    let market_prob = market_price.mid_price;

    // Calculate gross edge (without fees) for direction determination
    let gross_edge_pct = (model_prob - market_prob) * 100.0;

    // Calculate fee-adjusted edge for the actual trading decision
    // This accounts for entry and exit fees based on the platform
    let fee_adjusted_edge_pct = calculate_fee_adjusted_edge(market_prob, model_prob, selected_platform);

    // Skip if fee-adjusted edge is too small (this is the key change)
    // We use the absolute value because sells have negative gross edge
    if fee_adjusted_edge_pct.abs() < min_edge_pct {
        debug!(
            "Skipping {} - gross edge {:.1}%, fee-adjusted edge {:.1}% < {:.1}% threshold ({:?})",
            team, gross_edge_pct.abs(), fee_adjusted_edge_pct.abs(), min_edge_pct, selected_platform
        );
        return false;
    }

    // Determine direction (based on gross edge direction)
    let (direction, signal_type) = if gross_edge_pct > 0.0 {
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

    // Create signal with the selected platform
    // Use fee-adjusted edge for the signal's edge_pct field
    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction,
        model_prob,
        market_prob: Some(market_prob),
        edge_pct: fee_adjusted_edge_pct.abs(), // Use fee-adjusted edge
        confidence: (fee_adjusted_edge_pct.abs() / 10.0).min(1.0), // Confidence based on fee-adjusted edge
        platform_buy: Some(selected_platform),
        platform_sell: None,
        buy_price: Some(market_price.yes_ask),
        sell_price: Some(market_price.yes_bid),
        // Use actual liquidity from market price, fallback to yes_ask_size, then total_liquidity
        liquidity_available: market_price
            .yes_ask_size
            .or(market_price.total_liquidity)
            .unwrap_or(10000.0),
        reason: format!(
            "Model: {:.1}% vs Market: {:.1}% = {:.1}% gross / {:.1}% net ({:?})",
            model_prob * 100.0,
            market_prob * 100.0,
            gross_edge_pct.abs(),
            fee_adjusted_edge_pct.abs(),
            selected_platform
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
        "SIGNAL: {} {} - model={:.1}% market={:.1}% gross={:.1}% net={:.1}% ({:?})",
        team,
        direction_str,
        model_prob * 100.0,
        market_prob * 100.0,
        gross_edge_pct.abs(),
        fee_adjusted_edge_pct.abs(),
        selected_platform
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

/// Check for cross-platform arbitrage opportunities using SIMD scanner.
///
/// Returns Some((arb_mask, profit_cents)) if an arb is found, None otherwise.
///
/// Arbitrage exists when:
/// - Kalshi YES + Poly NO < 100¢ (or vice versa)
/// - This means buying both sides guarantees profit
fn check_cross_platform_arb(
    kalshi_price: Option<&MarketPriceData>,
    poly_price: Option<&MarketPriceData>,
    min_profit_cents: i16,
) -> Option<(u8, i16)> {
    let (kalshi, poly) = match (kalshi_price, poly_price) {
        (Some(k), Some(p)) => (k, p),
        _ => return None, // Need both platforms for cross-platform arb
    };

    // Convert prices to cents (0-100 scale)
    let k_yes = (kalshi.yes_ask * 100.0).round() as u16;
    let k_no = ((1.0 - kalshi.yes_bid) * 100.0).round() as u16; // NO ask = 1 - YES bid
    let p_yes = (poly.yes_ask * 100.0).round() as u16;
    let p_no = ((1.0 - poly.yes_bid) * 100.0).round() as u16;

    // Use SIMD scanner to check for arbs (threshold 100 = $1.00)
    let arb_mask = check_arbs_simd(k_yes, k_no, p_yes, p_no, 100);

    if arb_mask == 0 {
        return None;
    }

    // Calculate profit for cross-platform arbs only
    let cross_platform_mask = arb_mask & (ARB_POLY_YES_KALSHI_NO | ARB_KALSHI_YES_POLY_NO);

    if cross_platform_mask == 0 {
        return None;
    }

    // Find the most profitable cross-platform arb
    let mut best_profit = 0i16;
    let mut best_mask = 0u8;

    if arb_mask & ARB_POLY_YES_KALSHI_NO != 0 {
        let profit = calculate_profit_cents(k_yes, k_no, p_yes, p_no, ARB_POLY_YES_KALSHI_NO);
        if profit > best_profit {
            best_profit = profit;
            best_mask = ARB_POLY_YES_KALSHI_NO;
        }
    }

    if arb_mask & ARB_KALSHI_YES_POLY_NO != 0 {
        let profit = calculate_profit_cents(k_yes, k_no, p_yes, p_no, ARB_KALSHI_YES_POLY_NO);
        if profit > best_profit {
            best_profit = profit;
            best_mask = ARB_KALSHI_YES_POLY_NO;
        }
    }

    if best_profit >= min_profit_cents {
        Some((best_mask, best_profit))
    } else {
        None
    }
}

/// Emit a cross-platform arbitrage signal
async fn emit_arb_signal(
    redis: &RedisBus,
    game_id: &str,
    sport: Sport,
    team: &str,
    arb_mask: u8,
    profit_cents: i16,
    kalshi_price: &MarketPriceData,
    poly_price: &MarketPriceData,
) -> bool {
    let arb_types = decode_arb_mask(arb_mask);
    let arb_type_str = arb_types.first().unwrap_or(&"Unknown");

    let (buy_platform, sell_platform) = if arb_mask == ARB_POLY_YES_KALSHI_NO {
        (Platform::Polymarket, Platform::Kalshi)
    } else {
        (Platform::Kalshi, Platform::Polymarket)
    };

    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type: SignalType::CrossMarketArb,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction: SignalDirection::Buy,
        model_prob: 0.0,
        market_prob: None,
        edge_pct: profit_cents as f64,
        confidence: 1.0,
        platform_buy: Some(buy_platform),
        platform_sell: Some(sell_platform),
        buy_price: Some(if arb_mask == ARB_POLY_YES_KALSHI_NO {
            poly_price.yes_ask
        } else {
            kalshi_price.yes_ask
        }),
        sell_price: Some(if arb_mask == ARB_POLY_YES_KALSHI_NO {
            1.0 - kalshi_price.yes_bid
        } else {
            1.0 - poly_price.yes_bid
        }),
        liquidity_available: kalshi_price.yes_ask_size.unwrap_or(100.0).min(
            poly_price.yes_ask_size.unwrap_or(100.0)
        ),
        reason: format!(
            "ARB: {} - profit={:.0}¢ (buy {:?} YES + {:?} NO)",
            arb_type_str, profit_cents, buy_platform, sell_platform
        ),
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::seconds(10)),
        play_id: None,
    };

    info!(
        "ARB SIGNAL: {} {} - profit={}¢ ({:?} YES + {:?} NO)",
        team, arb_type_str, profit_cents, buy_platform, sell_platform
    );

    match redis.publish(channels::SIGNALS_NEW, &signal).await {
        Ok(_) => true,
        Err(e) => {
            error!("Failed to publish arb signal: {}", e);
            false
        }
    }
}
