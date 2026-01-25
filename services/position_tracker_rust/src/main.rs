//! Position Tracker Service (Rust)
//!
//! Responsibilities:
//! - Subscribe to ExecutionResult messages (positions opened)
//! - Track open positions in memory and database
//! - Monitor positions for exit conditions (take-profit, stop-loss)
//! - Handle game endings (forced settlement)
//! - Emit PositionUpdate messages for UI/monitoring

use anyhow::{Context, Result};
use arbees_rust_core::models::{
    channels, get_stop_loss_for_sport, ExecutionResult, ExecutionSide, ExecutionStatus, Platform,
    PositionState, PositionUpdate, Sport, TradeOutcome, TradeSide, TradeStatus,
};
use arbees_rust_core::redis::RedisBus;
use chrono::{DateTime, Duration, Utc};
use dotenv::dotenv;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
struct Config {
    take_profit_pct: f64,
    default_stop_loss_pct: f64,
    exit_check_interval_secs: f64,
    initial_bankroll: f64,
    min_hold_seconds: f64,
    price_staleness_ttl: f64,
    require_valid_book: bool,
    debounce_exit_checks: u32,
    exit_team_match_min_confidence: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            take_profit_pct: env::var("TAKE_PROFIT_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3.0),
            default_stop_loss_pct: env::var("DEFAULT_STOP_LOSS_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5.0),
            exit_check_interval_secs: env::var("EXIT_CHECK_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            initial_bankroll: env::var("INITIAL_BANKROLL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000.0),
            min_hold_seconds: env::var("MIN_HOLD_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10.0),
            price_staleness_ttl: env::var("PRICE_STALENESS_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30.0),
            require_valid_book: env::var("REQUIRE_VALID_BOOK")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(true),
            debounce_exit_checks: env::var("DEBOUNCE_EXIT_CHECKS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            exit_team_match_min_confidence: env::var("EXIT_TEAM_MATCH_MIN_CONFIDENCE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.7),
        }
    }
}

// ============================================================================
// Open Position
// ============================================================================

#[derive(Debug, Clone)]
struct OpenPosition {
    trade_id: String,
    signal_id: String,
    game_id: String,
    sport: Sport,
    platform: Platform,
    market_id: String,
    market_title: String,
    side: TradeSide,
    entry_price: f64,
    size: f64,
    entry_time: DateTime<Utc>,
    contract_team: Option<String>,
}

// ============================================================================
// Position Tracker State
// ============================================================================

struct PositionTrackerState {
    config: Config,
    pool: PgPool,
    redis: RedisBus,

    // Open positions in memory
    open_positions: Vec<OpenPosition>,

    // Counters
    positions_opened: u64,
    positions_closed: u64,

    // Exit trigger debounce: trade_id -> count
    exit_trigger_counts: HashMap<String, u32>,

    // Game cooldowns for signal processor: game_id -> (time, was_win)
    game_cooldowns: HashMap<String, (DateTime<Utc>, bool)>,

    // Bankroll state
    current_balance: f64,
    piggybank_balance: f64,

    // Cached market prices from Redis: game_id -> (team -> price_data)
    price_cache: Arc<RwLock<HashMap<String, HashMap<String, CachedPrice>>>>,
}

#[derive(Debug, Clone)]
struct CachedPrice {
    yes_bid: f64,
    yes_ask: f64,
    mid_price: f64,
    updated_at: DateTime<Utc>,
}

impl PositionTrackerState {
    async fn new(config: Config, pool: PgPool, redis: RedisBus, price_cache: Arc<RwLock<HashMap<String, HashMap<String, CachedPrice>>>>) -> Result<Self> {
        let initial_bankroll = config.initial_bankroll;

        let mut state = Self {
            config,
            pool,
            redis,
            open_positions: Vec::new(),
            positions_opened: 0,
            positions_closed: 0,
            exit_trigger_counts: HashMap::new(),
            game_cooldowns: HashMap::new(),
            current_balance: initial_bankroll,
            piggybank_balance: 0.0,
            price_cache,
        };

        // Load bankroll from DB
        state.load_bankroll().await?;

        // Load open positions from DB
        state.load_open_positions().await?;

        Ok(state)
    }

    async fn load_bankroll(&mut self) -> Result<()> {
        let row = sqlx::query(
            r#"
            SELECT current_balance::float8, piggybank_balance::float8
            FROM bankroll
            WHERE account_name = 'default'
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            self.current_balance = row
                .try_get::<f64, _>("current_balance")
                .unwrap_or(self.config.initial_bankroll);
            self.piggybank_balance = row.try_get::<f64, _>("piggybank_balance").unwrap_or(0.0);
            info!("Loaded bankroll: ${:.2} (piggybank: ${:.2})", self.current_balance, self.piggybank_balance);
        }

        Ok(())
    }

    async fn save_bankroll(&self) -> Result<()> {
        // Update bankroll, also update peak if we hit a new high
        let total_balance = self.current_balance + self.piggybank_balance;
        sqlx::query(
            r#"
            UPDATE bankroll
            SET current_balance = $1,
                piggybank_balance = $2,
                peak_balance = GREATEST(peak_balance, $3),
                trough_balance = LEAST(trough_balance, $1),
                updated_at = NOW()
            WHERE account_name = 'default'
            "#,
        )
        .bind(self.current_balance)
        .bind(self.piggybank_balance)
        .bind(total_balance)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_open_positions(&mut self) -> Result<()> {
        let rows = sqlx::query(
            r#"
            SELECT trade_id, signal_id, game_id, sport::text, platform::text, market_id, market_title,
                   side::text, entry_price::float8, size::float8, time as entry_time
            FROM paper_trades
            WHERE status = 'open'
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        for row in rows {
            let sport_str: Option<String> = row.try_get("sport").ok();
            let sport = match sport_str.as_deref() {
                Some("nba") | Some("NBA") => Sport::NBA,
                Some("ncaab") | Some("NCAAB") => Sport::NCAAB,
                Some("nfl") | Some("NFL") => Sport::NFL,
                Some("ncaaf") | Some("NCAAF") => Sport::NCAAF,
                Some("nhl") | Some("NHL") => Sport::NHL,
                Some("mlb") | Some("MLB") => Sport::MLB,
                Some("mls") | Some("MLS") => Sport::MLS,
                Some("soccer") | Some("SOCCER") => Sport::Soccer,
                _ => Sport::NBA,
            };

            let platform_str: String = row.try_get("platform").unwrap_or_default();
            let platform = match platform_str.as_str() {
                "kalshi" => Platform::Kalshi,
                "polymarket" => Platform::Polymarket,
                _ => Platform::Paper,
            };

            let side_str: String = row.get("side");
            let side = match side_str.as_str() {
                "buy" => TradeSide::Buy,
                _ => TradeSide::Sell,
            };

            self.open_positions.push(OpenPosition {
                trade_id: row.get("trade_id"),
                signal_id: row.try_get("signal_id").unwrap_or_default(),
                game_id: row.try_get("game_id").unwrap_or_default(),
                sport,
                platform,
                market_id: row.get("market_id"),
                market_title: row.try_get("market_title").unwrap_or_default(),
                side,
                entry_price: row.get("entry_price"),
                size: row.get("size"),
                entry_time: row.try_get("entry_time").unwrap_or_else(|_| Utc::now()),
                contract_team: None,
            });
        }

        info!(
            "Loaded {} open positions from database",
            self.open_positions.len()
        );
        Ok(())
    }

    async fn handle_execution_result(&mut self, result: ExecutionResult) -> Result<()> {
        info!(
            "Received ExecutionResult: {} status={:?}",
            result.request_id, result.status
        );

        if result.status == ExecutionStatus::Filled {
            self.positions_opened += 1;

            // Add to open positions
            let trade_id = result.order_id.clone().unwrap_or(result.request_id.clone());

            let side = match result.side {
                ExecutionSide::Yes => TradeSide::Buy,
                ExecutionSide::No => TradeSide::Sell,
            };

            let position = OpenPosition {
                trade_id: trade_id.clone(),
                signal_id: result.signal_id.clone(),
                game_id: result.game_id.clone(),
                sport: result.sport,
                platform: result.platform,
                market_id: result.market_id.clone(),
                market_title: result.contract_team.clone().unwrap_or_default(),
                side,
                entry_price: result.avg_price,
                size: result.filled_qty,
                entry_time: result.executed_at,
                contract_team: result.contract_team.clone(),
            };

            self.open_positions.push(position);

            // INSERT into paper_trades
            let sport_str = match result.sport {
                Sport::NBA => "nba",
                Sport::NCAAB => "ncaab",
                Sport::NFL => "nfl",
                Sport::NCAAF => "ncaaf",
                Sport::NHL => "nhl",
                Sport::MLB => "mlb",
                Sport::MLS => "mls",
                Sport::Soccer => "soccer",
                Sport::Tennis => "tennis",
                Sport::MMA => "mma",
            };
            let platform_str = match result.platform {
                Platform::Kalshi => "kalshi",
                Platform::Polymarket => "polymarket",
                Platform::Paper => "paper",
            };
            let side_str = match side {
                TradeSide::Buy => "buy",
                TradeSide::Sell => "sell",
            };

            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO paper_trades (trade_id, signal_id, game_id, sport, platform, market_id, market_title, side, entry_price, size, time, entry_time, status)
                VALUES ($1, $2, $3, $4::sport_enum, $5::platform_enum, $6, $7, $8::trade_side_enum, $9, $10, $11, $11, 'open')
                "#,
            )
            .bind(&trade_id)
            .bind(&result.signal_id)
            .bind(&result.game_id)
            .bind(sport_str)
            .bind(platform_str)
            .bind(&result.market_id)
            .bind(result.contract_team.as_deref().unwrap_or(""))
            .bind(side_str)
            .bind(result.avg_price)
            .bind(result.filled_qty)
            .bind(result.executed_at)
            .execute(&self.pool)
            .await
            {
                error!("Failed to insert paper_trade: {}", e);
            } else {
                info!("Inserted paper_trade {} into database", trade_id);
            }

            // Emit position update
            let update = PositionUpdate {
                position_id: Uuid::new_v4().to_string(),
                trade_id,
                state: PositionState::Open,
                game_id: result.game_id,
                sport: result.sport,
                platform: result.platform,
                market_id: result.market_id,
                contract_team: result.contract_team,
                side: result.side,
                entry_price: result.avg_price,
                current_price: None,
                size: result.filled_qty,
                unrealized_pnl: 0.0,
                realized_pnl: 0.0,
                fees_paid: result.fees,
                exit_price: None,
                exit_reason: None,
                stop_loss_price: None,
                take_profit_price: None,
                opened_at: result.executed_at,
                updated_at: Utc::now(),
                closed_at: None,
            };

            self.redis
                .publish(channels::POSITION_UPDATES, &update)
                .await?;

            // Format direction as "to win" / "to lose" for clarity
            let direction_str = match update.side {
                ExecutionSide::Yes => "to win",
                ExecutionSide::No => "to lose",
            };
            let team_name = update.contract_team.as_deref().unwrap_or(&update.game_id);
            info!(
                "OPEN: {} {} @ {:.3} x ${:.2}",
                team_name, direction_str, update.entry_price, update.size
            );
        }

        Ok(())
    }

    async fn handle_game_ended(&mut self, data: serde_json::Value) -> Result<()> {
        let game_id = data.get("game_id").and_then(|v| v.as_str()).unwrap_or("");
        if game_id.is_empty() {
            return Ok(());
        }

        let home_score = data.get("home_score").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let away_score = data.get("away_score").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let home_team = data.get("home_team").and_then(|v| v.as_str()).unwrap_or("");
        let away_team = data.get("away_team").and_then(|v| v.as_str()).unwrap_or("");
        let home_won = home_score > away_score;

        info!(
            "Game {} ended: {} {} - {} {}",
            game_id, home_team, home_score, away_score, away_team
        );

        // Find open positions for this game
        let mut positions_to_close: Vec<(usize, f64, bool)> = Vec::new();

        for (idx, position) in self.open_positions.iter().enumerate() {
            if position.game_id == game_id {
                // Determine if trade was on winning team
                let title = position.market_title.to_lowercase();
                let trade_on_home = !home_team.is_empty() && self.teams_match(home_team, &title);
                let trade_on_away = !away_team.is_empty() && self.teams_match(away_team, &title);

                let team_won = if trade_on_home {
                    home_won
                } else if trade_on_away {
                    !home_won
                } else {
                    home_won // Fallback
                };

                let exit_price = if team_won { 1.0 } else { 0.0 };
                positions_to_close.push((idx, exit_price, team_won));
            }
        }

        // Close positions (in reverse order to preserve indices)
        for (idx, exit_price, was_win) in positions_to_close.into_iter().rev() {
            let position = self.open_positions.remove(idx);
            self.close_position(&position, exit_price, "game_settlement", was_win)
                .await?;
        }

        Ok(())
    }

    fn teams_match(&self, team1: &str, team2: &str) -> bool {
        if team1.is_empty() || team2.is_empty() {
            return false;
        }

        let t1 = team1.to_lowercase();
        let t2 = team2.to_lowercase();

        if t1 == t2 {
            return true;
        }
        if t1.contains(&t2) || t2.contains(&t1) {
            return true;
        }

        // Check last word match (team name without city)
        let t1_words: Vec<&str> = t1.split_whitespace().collect();
        let t2_words: Vec<&str> = t2.split_whitespace().collect();
        if !t1_words.is_empty() && !t2_words.is_empty() {
            if t1_words.last() == t2_words.last() {
                return true;
            }
        }

        false
    }

    async fn close_position(
        &mut self,
        position: &OpenPosition,
        exit_price: f64,
        exit_reason: &str,
        was_win: bool,
    ) -> Result<()> {
        self.positions_closed += 1;

        // Calculate PnL
        let gross_pnl = match position.side {
            TradeSide::Buy => position.size * (exit_price - position.entry_price),
            TradeSide::Sell => position.size * (position.entry_price - exit_price),
        };

        // Piggybank: 50% of profit goes to savings
        if gross_pnl > 0.0 {
            let to_piggybank = gross_pnl * 0.5;
            self.piggybank_balance += to_piggybank;
            self.current_balance += gross_pnl - to_piggybank;
        } else {
            self.current_balance += gross_pnl;
        }

        // Update trade in DB with PnL
        let outcome = if was_win { "win" } else { "loss" };
        let pnl_pct = match position.side {
            TradeSide::Buy => (exit_price - position.entry_price) / position.entry_price * 100.0,
            TradeSide::Sell => (position.entry_price - exit_price) / position.entry_price * 100.0,
        };

        sqlx::query(
            r#"
            UPDATE paper_trades
            SET status = 'closed',
                exit_price = $1,
                exit_time = NOW(),
                outcome = $2::trade_outcome_enum,
                pnl = $3,
                pnl_pct = $4
            WHERE trade_id = $5
            "#,
        )
        .bind(exit_price)
        .bind(outcome)
        .bind(gross_pnl)
        .bind(pnl_pct)
        .bind(&position.trade_id)
        .execute(&self.pool)
        .await?;

        // Save bankroll
        self.save_bankroll().await?;

        // Record cooldown
        self.game_cooldowns
            .insert(position.game_id.clone(), (Utc::now(), was_win));

        // Emit position update
        let update = PositionUpdate {
            position_id: Uuid::new_v4().to_string(),
            trade_id: position.trade_id.clone(),
            state: PositionState::Closed,
            game_id: position.game_id.clone(),
            sport: position.sport,
            platform: position.platform,
            market_id: position.market_id.clone(),
            contract_team: position.contract_team.clone(),
            side: match position.side {
                TradeSide::Buy => ExecutionSide::Yes,
                TradeSide::Sell => ExecutionSide::No,
            },
            entry_price: position.entry_price,
            current_price: Some(exit_price),
            size: position.size,
            unrealized_pnl: 0.0,
            realized_pnl: gross_pnl,
            fees_paid: 0.0,
            exit_price: Some(exit_price),
            exit_reason: Some(exit_reason.to_string()),
            stop_loss_price: None,
            take_profit_price: None,
            opened_at: position.entry_time,
            updated_at: Utc::now(),
            closed_at: Some(Utc::now()),
        };

        self.redis
            .publish(channels::POSITION_UPDATES, &update)
            .await?;

        // Format direction as "to win" / "to lose" for clarity
        let direction_str = match position.side {
            TradeSide::Buy => "to win",
            TradeSide::Sell => "to lose",
        };
        let pnl_sign = if gross_pnl >= 0.0 { "+" } else { "" };
        info!(
            "CLOSE: {} {} - entry={:.3} exit={:.3} pnl={}{:.2} ({})",
            position.market_title, direction_str, position.entry_price, exit_price, pnl_sign, gross_pnl, exit_reason
        );

        Ok(())
    }

    async fn check_exit_conditions(&mut self) -> Result<()> {
        let now = Utc::now();
        let min_hold = Duration::seconds(self.config.min_hold_seconds as i64);

        let mut positions_to_exit: Vec<(usize, f64, String)> = Vec::new();

        for (idx, position) in self.open_positions.iter().enumerate() {
            // Don't exit too soon after entry
            if now - position.entry_time < min_hold {
                continue;
            }

            // Get current market price
            let price_row = self.get_current_price(position).await?;
            if price_row.is_none() {
                continue;
            }
            let price = price_row.unwrap();

            // Check staleness
            let price_age_ms = (now - price.time).num_milliseconds() as f64;
            if price_age_ms > self.config.price_staleness_ttl * 1000.0 {
                debug!("Skipping exit check for {}: stale price", position.trade_id);
                continue;
            }

            // Check pathological book
            if self.config.require_valid_book {
                if price.yes_bid <= 0.0 && price.yes_ask >= 1.0 {
                    debug!(
                        "Skipping exit check for {}: pathological book",
                        position.trade_id
                    );
                    continue;
                }
                let spread = price.yes_ask - price.yes_bid;
                if spread > 0.5 {
                    debug!(
                        "Skipping exit check for {}: extreme spread",
                        position.trade_id
                    );
                    continue;
                }
            }

            // Calculate mark price (mid for P&L, bid/ask for execution)
            let mark_price = (price.yes_bid + price.yes_ask) / 2.0;
            let exec_price = match position.side {
                TradeSide::Buy => price.yes_bid,  // Sell at bid
                TradeSide::Sell => price.yes_ask, // Cover at ask
            };

            // Evaluate exit
            let stop_loss_pct = get_stop_loss_for_sport(&position.sport);
            let (should_exit, reason) = self.evaluate_exit(position, mark_price, stop_loss_pct);

            if should_exit {
                // Debounce
                if self.config.debounce_exit_checks > 0 {
                    let count = self
                        .exit_trigger_counts
                        .entry(position.trade_id.clone())
                        .or_insert(0);
                    *count += 1;

                    if *count < self.config.debounce_exit_checks {
                        debug!(
                            "Exit debounce {}: {}/{}",
                            position.trade_id, count, self.config.debounce_exit_checks
                        );
                        continue;
                    }
                }

                positions_to_exit.push((idx, exec_price, reason));
            } else {
                // Reset debounce
                self.exit_trigger_counts.remove(&position.trade_id);
            }
        }

        // Execute exits (in reverse order)
        for (idx, exec_price, reason) in positions_to_exit.into_iter().rev() {
            let position = self.open_positions.remove(idx);
            let was_win = match position.side {
                TradeSide::Buy => exec_price > position.entry_price,
                TradeSide::Sell => exec_price < position.entry_price,
            };
            self.close_position(&position, exec_price, &reason, was_win)
                .await?;
        }

        Ok(())
    }

    fn evaluate_exit(
        &self,
        position: &OpenPosition,
        current_price: f64,
        stop_loss_pct: f64,
    ) -> (bool, String) {
        let entry_price = position.entry_price;
        let take_profit_threshold = self.config.take_profit_pct / 100.0;
        let stop_loss_threshold = stop_loss_pct / 100.0;

        let price_move = match position.side {
            TradeSide::Buy => current_price - entry_price,
            TradeSide::Sell => entry_price - current_price,
        };

        if price_move >= take_profit_threshold {
            return (true, format!("take_profit: +{:.1}%", price_move * 100.0));
        }
        if price_move <= -stop_loss_threshold {
            return (true, format!("stop_loss: {:.1}%", price_move * 100.0));
        }

        (false, String::new())
    }

    async fn get_current_price(&self, position: &OpenPosition) -> Result<Option<MarketPriceRow>> {
        // Try to get price from Redis cache first (by game_id + team)
        let cache = self.price_cache.read().await;

        if let Some(game_prices) = cache.get(&position.game_id) {
            // Try exact match on market_title (team name)
            if let Some(cached) = game_prices.get(&position.market_title) {
                return Ok(Some(MarketPriceRow {
                    market_id: position.market_id.clone(),
                    yes_bid: cached.yes_bid,
                    yes_ask: cached.yes_ask,
                    time: cached.updated_at,
                }));
            }

            // Try fuzzy match on team name
            for (team, cached) in game_prices.iter() {
                if self.teams_match(team, &position.market_title) {
                    return Ok(Some(MarketPriceRow {
                        market_id: position.market_id.clone(),
                        yes_bid: cached.yes_bid,
                        yes_ask: cached.yes_ask,
                        time: cached.updated_at,
                    }));
                }
            }
        }
        drop(cache);

        // Fallback to DB query (legacy, may be stale)
        let platform_str = match position.platform {
            Platform::Kalshi => "kalshi",
            Platform::Polymarket => "polymarket",
            Platform::Paper => "paper",
        };

        let row = sqlx::query(
            r#"
            SELECT market_id, yes_bid, yes_ask, time
            FROM market_prices
            WHERE market_id = $1 AND platform = $2::platform_enum
            ORDER BY time DESC
            LIMIT 1
            "#,
        )
        .bind(&position.market_id)
        .bind(platform_str)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| MarketPriceRow {
            market_id: r.get("market_id"),
            yes_bid: r.get("yes_bid"),
            yes_ask: r.get("yes_ask"),
            time: r.get("time"),
        }))
    }

    async fn sweep_orphaned_positions(&mut self) -> Result<()> {
        if self.open_positions.is_empty() {
            return Ok(());
        }

        let game_ids: Vec<String> = self
            .open_positions
            .iter()
            .filter(|p| !p.game_id.is_empty())
            .map(|p| p.game_id.clone())
            .collect();

        if game_ids.is_empty() {
            return Ok(());
        }

        // Query for ended games - using simple approach
        let mut ended_games: HashMap<String, GameEndRow> = HashMap::new();

        for game_id in &game_ids {
            let row = sqlx::query(
                r#"
                SELECT game_id, home_team, away_team, final_home_score, final_away_score, status
                FROM games
                WHERE game_id = $1
                  AND status IN ('final', 'complete', 'completed')
                "#,
            )
            .bind(game_id)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(r) = row {
                ended_games.insert(
                    game_id.clone(),
                    GameEndRow {
                        home_team: r.try_get("home_team").unwrap_or_default(),
                        away_team: r.try_get("away_team").unwrap_or_default(),
                        home_score: r.try_get("final_home_score").unwrap_or(0),
                        away_score: r.try_get("final_away_score").unwrap_or(0),
                    },
                );
            }
        }

        if !ended_games.is_empty() {
            info!(
                "Orphan sweep: found {} ended games with open positions",
                ended_games.len()
            );
        }

        let mut positions_to_close: Vec<(usize, f64, bool)> = Vec::new();

        for (idx, position) in self.open_positions.iter().enumerate() {
            if let Some(game_info) = ended_games.get(&position.game_id) {
                let home_won = game_info.home_score > game_info.away_score;
                let title = position.market_title.to_lowercase();
                let trade_on_home = self.teams_match(&game_info.home_team, &title);
                let trade_on_away = self.teams_match(&game_info.away_team, &title);

                let team_won = if trade_on_home {
                    home_won
                } else if trade_on_away {
                    !home_won
                } else {
                    home_won
                };

                let exit_price = if team_won { 1.0 } else { 0.0 };
                positions_to_close.push((idx, exit_price, team_won));

                warn!(
                    "Orphan sweep: settling {} for ended game {}",
                    position.trade_id, position.game_id
                );
            }
        }

        for (idx, exit_price, was_win) in positions_to_close.into_iter().rev() {
            let position = self.open_positions.remove(idx);
            self.close_position(&position, exit_price, "orphan_settlement", was_win)
                .await?;
        }

        Ok(())
    }
}

// ============================================================================
// DB Row Types
// ============================================================================

#[derive(Debug)]
struct MarketPriceRow {
    market_id: String,
    yes_bid: f64,
    yes_ask: f64,
    time: DateTime<Utc>,
}

struct GameEndRow {
    home_team: String,
    away_team: String,
    home_score: i32,
    away_score: i32,
}

// ============================================================================
// Heartbeat
// ============================================================================

#[derive(Debug, Serialize)]
struct Heartbeat {
    service: String,
    instance_id: String,
    status: String,
    timestamp: String,
    checks: HashMap<String, bool>,
    metrics: HashMap<String, f64>,
}

async fn heartbeat_loop(
    redis: RedisBus,
    instance_id: String,
    state: Arc<RwLock<PositionTrackerState>>,
) -> Result<()> {
    info!("Heartbeat loop started for {}", instance_id);

    loop {
        let state = state.read().await;

        let mut checks = HashMap::new();
        checks.insert("redis_ok".to_string(), true);
        checks.insert("db_ok".to_string(), true);

        let mut metrics = HashMap::new();
        metrics.insert(
            "positions_opened".to_string(),
            state.positions_opened as f64,
        );
        metrics.insert(
            "positions_closed".to_string(),
            state.positions_closed as f64,
        );
        metrics.insert(
            "open_positions".to_string(),
            state.open_positions.len() as f64,
        );
        metrics.insert("current_balance".to_string(), state.current_balance);
        metrics.insert("piggybank_balance".to_string(), state.piggybank_balance);

        let heartbeat = Heartbeat {
            service: "position_tracker_rust".to_string(),
            instance_id: instance_id.clone(),
            status: "healthy".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            checks,
            metrics,
        };

        drop(state);

        if let Err(e) = redis.publish(channels::HEALTH_HEARTBEATS, &heartbeat).await {
            warn!("Failed to publish heartbeat: {}", e);
        }

        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

// ============================================================================
// Price Listener
// ============================================================================

/// Incoming market price message from polymarket_monitor (same format as game_shard)
#[derive(Debug, Deserialize)]
struct MarketPriceMessage {
    #[serde(default)]
    game_id: String,
    #[serde(default)]
    contract_team: Option<String>,
    #[serde(default)]
    yes_bid: f64,
    #[serde(default)]
    yes_ask: f64,
    #[serde(default)]
    mid_price: Option<f64>,
    #[serde(default)]
    implied_probability: Option<f64>,
}

async fn price_listener_loop(
    redis: RedisBus,
    price_cache: Arc<RwLock<HashMap<String, HashMap<String, CachedPrice>>>>,
) -> Result<()> {
    // Subscribe to game:*:price pattern
    let mut pubsub = redis.psubscribe("game:*:price").await?;
    info!("Subscribed to game:*:price pattern for exit monitoring");

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

        // Get raw bytes for msgpack
        let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Try msgpack first, then JSON
        let price: MarketPriceMessage = match rmp_serde::from_slice(&payload) {
            Ok(p) => p,
            Err(_) => match serde_json::from_slice(&payload) {
                Ok(p) => p,
                Err(_) => continue,
            },
        };

        // Get team name
        let team = match &price.contract_team {
            Some(t) if !t.is_empty() => t.clone(),
            _ => continue,
        };

        // Calculate mid price
        let mid = price
            .mid_price
            .or(price.implied_probability)
            .unwrap_or((price.yes_bid + price.yes_ask) / 2.0);

        // Update cache
        let cached = CachedPrice {
            yes_bid: price.yes_bid,
            yes_ask: price.yes_ask,
            mid_price: mid,
            updated_at: Utc::now(),
        };

        let mut cache = price_cache.write().await;
        cache
            .entry(game_id)
            .or_insert_with(HashMap::new)
            .insert(team, cached);
    }

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting Rust Position Tracker Service...");

    // Database connection
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    info!("Connected to database");

    // Redis connection
    let redis = RedisBus::new().await?;
    info!("Connected to Redis");

    // Configuration
    let config = Config::default();
    info!(
        "Config: take_profit={:.1}%, stop_loss={:.1}%, exit_interval={:.1}s",
        config.take_profit_pct, config.default_stop_loss_pct, config.exit_check_interval_secs
    );

    // Shared price cache
    let price_cache: Arc<RwLock<HashMap<String, HashMap<String, CachedPrice>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Initialize state
    let state = Arc::new(RwLock::new(
        PositionTrackerState::new(config.clone(), pool, redis.clone(), price_cache.clone()).await?,
    ));

    // Price listener - subscribe to game:*:price and cache prices
    let redis_price = redis.clone();
    let price_cache_listener = price_cache.clone();
    tokio::spawn(async move {
        if let Err(e) = price_listener_loop(redis_price, price_cache_listener).await {
            error!("Price listener error: {}", e);
        }
    });

    // Subscribe to execution results
    let mut pubsub_exec = redis.subscribe(channels::EXECUTION_RESULTS).await?;
    info!("Subscribed to {}", channels::EXECUTION_RESULTS);

    // Subscribe to game endings
    let redis_games = redis.clone();
    let mut pubsub_games = redis_games.subscribe(channels::GAMES_ENDED).await?;

    // Spawn game ended handler
    let state_games = state.clone();
    tokio::spawn(async move {
        let mut stream = pubsub_games.on_message();
        while let Some(msg) = stream.next().await {
            if let Ok(payload) = msg.get_payload::<String>() {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&payload) {
                    let mut s = state_games.write().await;
                    if let Err(e) = s.handle_game_ended(data).await {
                        error!("Error handling game ended: {}", e);
                    }
                }
            }
        }
    });

    // Start heartbeat
    let instance_id =
        env::var("HOSTNAME").unwrap_or_else(|_| "position-tracker-rust-1".to_string());
    let redis_hb = redis.clone();
    let state_hb = state.clone();
    tokio::spawn(async move {
        if let Err(e) = heartbeat_loop(redis_hb, instance_id, state_hb).await {
            error!("Heartbeat loop error: {}", e);
        }
    });

    // Exit monitoring loop
    let state_exit = state.clone();
    let exit_interval = config.exit_check_interval_secs;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs_f64(exit_interval)).await;
            let mut s = state_exit.write().await;
            if let Err(e) = s.check_exit_conditions().await {
                error!("Exit check error: {}", e);
            }
        }
    });

    // Orphan sweep loop (every 5 minutes)
    let state_orphan = state.clone();
    tokio::spawn(async move {
        // Wait a bit before first sweep
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            {
                let mut s = state_orphan.write().await;
                if let Err(e) = s.sweep_orphaned_positions().await {
                    error!("Orphan sweep error: {}", e);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    });

    info!("Position Tracker started");

    // Main message loop for execution results
    let mut stream = pubsub_exec.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to get payload: {}", e);
                continue;
            }
        };

        let result: ExecutionResult = match serde_json::from_str(&payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to parse execution result: {}", e);
                continue;
            }
        };

        let mut s = state.write().await;
        if let Err(e) = s.handle_execution_result(result).await {
            error!("Error handling execution result: {}", e);
        }
    }

    Ok(())
}
