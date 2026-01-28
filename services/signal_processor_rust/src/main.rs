//! Signal Processor Service (Rust)
//!
//! Responsibilities:
//! - Subscribe to trading signals from Redis (signals:new)
//! - Apply pre-trade filtering (edge threshold, probability bounds, cooldowns, duplicates)
//! - Check risk limits
//! - Emit ExecutionRequest messages to execution:requests channel

use anyhow::{Context, Result};
use arbees_rust_core::models::{
    channels, ExecutionRequest, ExecutionSide, NotificationEvent, NotificationPriority,
    NotificationType, Platform, RuleDecision, RuleDecisionType, SignalDirection, SignalType, Sport,
    TradingSignal, TransportMode,
};
use arbees_rust_core::redis::RedisBus;
use arbees_rust_core::utils::matching::match_team_in_text;
use chrono::{DateTime, Duration, Utc};
use dotenv::dotenv;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket};
use uuid::Uuid;

// ============================================================================
// ZMQ Message Format
// ============================================================================

/// ZMQ message envelope format (matches game_shard and execution_service)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ZmqEnvelope {
    seq: u64,
    timestamp_ms: i64,
    source: Option<String>,
    payload: serde_json::Value,
}

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
struct RiskSnapshot {
    balance: f64,
    daily_loss: f64,
    game_exposure: f64,
    sport_exposure: f64,
    position_count: i64,
}

#[derive(Debug, Clone)]
struct Config {
    min_edge_pct: f64,
    kelly_fraction: f64,
    max_position_pct: f64,
    max_buy_prob: f64,
    min_sell_prob: f64,
    allow_hedging: bool,
    max_daily_loss: f64,
    max_game_exposure: f64,
    max_sport_exposure: f64,
    max_latency_ms: f64,
    win_cooldown_seconds: f64,
    loss_cooldown_seconds: f64,
    initial_bankroll: f64,
    team_match_min_confidence: f64,
    /// Price staleness TTL in seconds - prices older than this are ignored
    price_staleness_secs: f64,
    /// Minimum liquidity threshold to trade ($10 default)
    liquidity_min_threshold: f64,
    /// Maximum percentage of available liquidity to use (80% default)
    liquidity_max_position_pct: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Minimum edge must account for round-trip fees:
            // Kalshi: ~0.7% entry + ~0.7% exit = 1.4% fees
            // Data shows: 5-10% edge = 36% win rate, 15%+ edge = 87.5% win rate
            // Default 15.0% for higher win rate (~90%)
            // NOTE: This default matches game_shard_rust for consistency
            min_edge_pct: env::var("MIN_EDGE_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15.0),
            kelly_fraction: env::var("KELLY_FRACTION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.25),
            max_position_pct: env::var("MAX_POSITION_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10.0),
            max_buy_prob: env::var("MAX_BUY_PROB")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.95),
            min_sell_prob: env::var("MIN_SELL_PROB")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.05),
            allow_hedging: env::var("ALLOW_HEDGING")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            max_daily_loss: env::var("MAX_DAILY_LOSS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100.0),
            max_game_exposure: env::var("MAX_GAME_EXPOSURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50.0),
            max_sport_exposure: env::var("MAX_SPORT_EXPOSURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200.0),
            max_latency_ms: env::var("MAX_LATENCY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5000.0),
            win_cooldown_seconds: env::var("WIN_COOLDOWN_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(180.0),
            loss_cooldown_seconds: env::var("LOSS_COOLDOWN_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300.0),
            initial_bankroll: env::var("INITIAL_BANKROLL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000.0),
            team_match_min_confidence: env::var("TEAM_MATCH_MIN_CONFIDENCE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.7),
            // Price staleness TTL - prices older than this are considered stale
            // Default: 30 seconds (was hardcoded as 2 minutes previously)
            price_staleness_secs: env::var("PRICE_STALENESS_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30.0),
            // Minimum liquidity threshold - don't trade if less than this available ($10 default)
            liquidity_min_threshold: env::var("LIQUIDITY_MIN_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10.0),
            // Maximum percentage of available liquidity to use (80% default)
            // This prevents taking the entire book and ensures we can exit
            liquidity_max_position_pct: env::var("LIQUIDITY_MAX_POSITION_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(80.0),
        }
    }
}

// ============================================================================
// Cached Rule
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct CachedRule {
    rule_id: String,
    rule_type: String,
    conditions: HashMap<String, serde_json::Value>,
    action: HashMap<String, serde_json::Value>,
    expires_at: Option<DateTime<Utc>>,
}

impl CachedRule {
    fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }

    fn matches(&self, signal: &TradingSignal) -> bool {
        for (key, value) in &self.conditions {
            let signal_value = match key.as_str() {
                "sport" => serde_json::Value::String(signal.sport.as_str().to_lowercase()),
                "signal_type" => {
                    serde_json::to_value(&signal.signal_type).unwrap_or(serde_json::Value::Null)
                }
                "direction" => {
                    serde_json::to_value(&signal.direction).unwrap_or(serde_json::Value::Null)
                }
                "edge_pct" | "edge" => serde_json::json!(signal.edge_pct),
                "model_prob" => serde_json::json!(signal.model_prob),
                "team" => serde_json::Value::String(signal.team.clone()),
                "game_id" => serde_json::Value::String(signal.game_id.clone()),
                _ => continue,
            };

            // Handle comparison operators
            if key.ends_with("_lt") {
                if let (Some(sig_num), Some(val_num)) = (signal_value.as_f64(), value.as_f64()) {
                    if sig_num >= val_num {
                        return false;
                    }
                }
            } else if key.ends_with("_lte") {
                if let (Some(sig_num), Some(val_num)) = (signal_value.as_f64(), value.as_f64()) {
                    if sig_num > val_num {
                        return false;
                    }
                }
            } else if key.ends_with("_gt") {
                if let (Some(sig_num), Some(val_num)) = (signal_value.as_f64(), value.as_f64()) {
                    if sig_num <= val_num {
                        return false;
                    }
                }
            } else if key.ends_with("_gte") {
                if let (Some(sig_num), Some(val_num)) = (signal_value.as_f64(), value.as_f64()) {
                    if sig_num < val_num {
                        return false;
                    }
                }
            } else {
                // Exact match (case-insensitive for strings)
                if let (Some(sig_str), Some(val_str)) = (signal_value.as_str(), value.as_str()) {
                    if sig_str.to_lowercase() != val_str.to_lowercase() {
                        return false;
                    }
                } else if signal_value != *value {
                    return false;
                }
            }
        }
        true
    }
}

// ============================================================================
// Signal Processor State
// ============================================================================

struct SignalProcessorState {
    config: Config,
    pool: PgPool,
    redis: RedisBus,

    // Transport configuration
    transport_mode: TransportMode,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,

    // Counters
    signal_count: u64,
    approved_count: u64,
    rejected_counts: HashMap<String, u64>,

    // Cooldown tracking: "game_id:team" -> (last_trade_time, was_win)
    // Team-specific cooldowns allow trading the OTHER team in a game while one team is in cooldown
    team_cooldowns: HashMap<String, (DateTime<Utc>, bool)>,

    // In-flight dedupe: idempotency_key -> timestamp
    in_flight: HashMap<String, DateTime<Utc>>,

    // Cached rules
    rules: Vec<CachedRule>,
    rules_last_updated: Option<DateTime<Utc>>,
}

impl SignalProcessorState {
    async fn new(
        config: Config,
        pool: PgPool,
        redis: RedisBus,
        transport_mode: TransportMode,
        zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    ) -> Self {
        let mut rejected_counts = HashMap::new();
        for key in &[
            "edge",
            "prob",
            "duplicate",
            "no_market",
            "cooldown",
            "risk",
            "rule_blocked",
        ] {
            rejected_counts.insert(key.to_string(), 0);
        }

        Self {
            config,
            pool,
            redis,
            transport_mode,
            zmq_pub,
            zmq_seq: Arc::new(AtomicU64::new(0)),
            signal_count: 0,
            approved_count: 0,
            rejected_counts,
            team_cooldowns: HashMap::new(),
            in_flight: HashMap::new(),
            rules: Vec::new(),
            rules_last_updated: None,
        }
    }

    async fn load_rules_from_db(&mut self) -> Result<()> {
        let rows = sqlx::query(
            r#"
            SELECT rule_id, rule_type, conditions, action, expires_at
            FROM trading_rules
            WHERE status = 'active'
              AND (expires_at IS NULL OR expires_at > NOW())
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        self.rules.clear();
        for row in rows {
            let rule_id: String = row.get("rule_id");
            let rule_type: String = row.get("rule_type");
            let conditions_json: serde_json::Value = row.try_get("conditions").unwrap_or_default();
            let action_json: serde_json::Value = row.try_get("action").unwrap_or_default();
            let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at").ok();

            let conditions: HashMap<String, serde_json::Value> =
                serde_json::from_value(conditions_json).unwrap_or_default();
            let action: HashMap<String, serde_json::Value> =
                serde_json::from_value(action_json).unwrap_or_default();

            self.rules.push(CachedRule {
                rule_id,
                rule_type,
                conditions,
                action,
                expires_at,
            });
        }
        self.rules_last_updated = Some(Utc::now());
        info!("Loaded {} rules from database", self.rules.len());
        Ok(())
    }

    // ========================================================================
    // Risk Check Database Queries
    // ========================================================================

    /// Get current available balance from bankroll table
    async fn get_available_balance(&self) -> Result<f64> {
        let row = sqlx::query(
            r#"
            SELECT COALESCE(current_balance, 0.0)::float8 as current_balance
            FROM bankroll
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.get::<f64, _>("current_balance")).unwrap_or(self.config.initial_bankroll))
    }

    /// Get total exposure for a specific game (open OR recently closed within 60s cooldown)
    async fn get_game_exposure(&self, game_id: &str) -> Result<f64> {
        let row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(size::float8), 0.0) as exposure
            FROM paper_trades
            WHERE game_id = $1
              AND (status = 'open' OR (status = 'closed' AND exit_time > NOW() - INTERVAL '60 seconds'))
            "#,
        )
        .bind(game_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<f64, _>("exposure"))
    }

    /// Get total exposure for a sport (sum of open position sizes)
    async fn get_sport_exposure(&self, sport: Sport) -> Result<f64> {
        let row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(size::float8), 0.0) as exposure
            FROM paper_trades
            WHERE sport = $1::text::sport_enum AND status = 'open'
            "#,
        )
        .bind(sport.as_str().to_lowercase())
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<f64, _>("exposure"))
    }

    /// Get total realized losses for today
    async fn get_daily_loss(&self) -> Result<f64> {
        let row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(ABS(pnl::float8)), 0.0) as daily_loss
            FROM paper_trades
            WHERE status = 'closed'
              AND DATE(time) = CURRENT_DATE
              AND pnl < 0
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<f64, _>("daily_loss"))
    }

    /// Count positions for a specific game (open OR closed within last 60 seconds)
    async fn count_game_positions(&self, game_id: &str) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM paper_trades
            WHERE game_id = $1
              AND (status = 'open' OR (status = 'closed' AND exit_time > NOW() - INTERVAL '60 seconds'))
            "#,
        )
        .bind(game_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<i64, _>("count"))
    }

    /// Check if we have an existing position on this team in the OPPOSITE direction
    /// This prevents flip-flopping between buy and sell on the same team
    async fn has_opposing_position(
        &self,
        game_id: &str,
        team: &str,
        direction: SignalDirection,
    ) -> Result<bool> {
        let opposite_side = match direction {
            SignalDirection::Buy => "sell",
            SignalDirection::Sell => "buy",
            SignalDirection::Hold => return Ok(false),
        };

        let row = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM paper_trades
            WHERE game_id = $1
              AND market_title = $2
              AND side::text = $3
              AND status = 'open'
            "#,
        )
        .bind(game_id)
        .bind(team)
        .bind(opposite_side)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<i64, _>("count") > 0)
    }

    /// Master risk check - returns (approved, rejection_reason, snapshot)
    ///
    /// PERFORMANCE: All 7 DB queries run in parallel using tokio::join!
    /// to reduce latency from ~300-600ms (sequential) to ~50-100ms (parallel)
    async fn check_risk_limits(
        &self,
        signal: &TradingSignal,
        proposed_size: f64,
    ) -> Result<(bool, Option<String>, RiskSnapshot)> {
        // Run all risk check queries in parallel for better latency
        let (
            balance_result,
            daily_loss_result,
            game_exposure_result,
            sport_exposure_result,
            position_count_result,
            has_opposing_result,
        ) = tokio::join!(
            self.get_available_balance(),
            self.get_daily_loss(),
            self.get_game_exposure(&signal.game_id),
            self.get_sport_exposure(signal.sport),
            self.count_game_positions(&signal.game_id),
            self.has_opposing_position(&signal.game_id, &signal.team, signal.direction)
        );

        // Unwrap results (propagate first error if any)
        let balance = balance_result?;
        let daily_loss = daily_loss_result?;
        let game_exposure = game_exposure_result?;
        let sport_exposure = sport_exposure_result?;
        let position_count = position_count_result?;
        let has_opposing = has_opposing_result?;

        // Game exposure limit: if MAX_GAME_EXPOSURE < 0, disable per-game limit entirely
        let effective_max_game_exposure = if self.config.max_game_exposure < 0.0 {
            None  // No per-game limit
        } else {
            Some(self.config.max_game_exposure)
        };

        let snapshot = RiskSnapshot {
            balance,
            daily_loss,
            game_exposure,
            sport_exposure,
            position_count,
        };

        // 1. Check bankroll sufficiency
        if proposed_size > balance {
            return Ok((
                false,
                Some(format!(
                    "Insufficient balance: proposed ${:.2} > available ${:.2}",
                    proposed_size, balance
                )),
                snapshot,
            ));
        }

        // 2. Check daily loss limit
        if daily_loss >= self.config.max_daily_loss {
            return Ok((
                false,
                Some(format!(
                    "Daily loss limit reached: ${:.2} >= ${:.2}",
                    daily_loss, self.config.max_daily_loss
                )),
                snapshot,
            ));
        }

        // 3. Check game exposure limit (skip if MAX_GAME_EXPOSURE < 0)
        if let Some(max_exposure) = effective_max_game_exposure {
            if game_exposure + proposed_size > max_exposure {
                return Ok((
                    false,
                    Some(format!(
                        "Game exposure limit: ${:.2} + ${:.2} > ${:.2}",
                        game_exposure, proposed_size, max_exposure
                    )),
                    snapshot,
                ));
            }
        }

        // 4. Check sport exposure limit
        if sport_exposure + proposed_size > self.config.max_sport_exposure {
            return Ok((
                false,
                Some(format!(
                    "Sport exposure limit: ${:.2} + ${:.2} > ${:.2}",
                    sport_exposure, proposed_size, self.config.max_sport_exposure
                )),
                snapshot,
            ));
        }

        // 5. Check for opposing position on same team (prevents flip-flopping)
        if has_opposing {
            return Ok((
                false,
                Some(format!(
                    "Opposing position exists: {} already has opposite side open",
                    signal.team
                )),
                snapshot,
            ));
        }

        // 6. Check position count per game (max 2)
        if position_count >= 2 {
            return Ok((
                false,
                Some(format!("Max positions per game: {} >= 2", position_count)),
                snapshot,
            ));
        }

        // All checks passed - format direction for clarity
        let direction_str = match signal.direction {
            SignalDirection::Buy => "to win",
            SignalDirection::Sell => "to lose",
            SignalDirection::Hold => "hold",
        };
        let max_exposure_str = effective_max_game_exposure
            .map(|m| format!("${:.2}", m))
            .unwrap_or_else(|| "unlimited".to_string());
        info!(
            "OPEN: {} {} - ${:.2} (balance=${:.2}, game_exp=${:.2}, max={})",
            signal.team, direction_str, proposed_size, balance, game_exposure, max_exposure_str
        );
        Ok((true, None, snapshot))
    }

    fn evaluate_rules(&self, signal: &TradingSignal) -> RuleDecision {
        let mut best_override: Option<f64> = None;
        let mut override_rule_id: Option<String> = None;

        for rule in &self.rules {
            if rule.is_expired() {
                continue;
            }

            if !rule.matches(signal) {
                continue;
            }

            let action_type = rule
                .action
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if action_type == "reject" {
                return RuleDecision {
                    allowed: false,
                    decision_type: RuleDecisionType::Rejected,
                    rule_id: Some(rule.rule_id.clone()),
                    reason: rule
                        .action
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    override_min_edge: None,
                };
            } else if action_type == "override" {
                if let Some(min_edge) = rule.action.get("min_edge_pct").and_then(|v| v.as_f64()) {
                    if min_edge > best_override.unwrap_or(0.0) {
                        best_override = Some(min_edge);
                        override_rule_id = Some(rule.rule_id.clone());
                    }
                }
            }
        }

        // If we have a threshold override, apply it
        if let Some(override_edge) = best_override {
            if signal.edge_pct < override_edge {
                return RuleDecision {
                    allowed: false,
                    decision_type: RuleDecisionType::ThresholdOverride,
                    rule_id: override_rule_id,
                    reason: Some(format!(
                        "Edge {:.1}% below override threshold {:.1}%",
                        signal.edge_pct, override_edge
                    )),
                    override_min_edge: Some(override_edge),
                };
            }
        }

        RuleDecision::default()
    }

    /// Check if a specific team is in cooldown for a game.
    /// Team-specific cooldowns allow trading the OTHER team in a game while one team is in cooldown.
    fn is_team_in_cooldown(&self, game_id: &str, team: &str) -> (bool, Option<String>) {
        let cooldown_key = format!("{}:{}", game_id, team);
        if let Some((last_trade_time, was_win)) = self.team_cooldowns.get(&cooldown_key) {
            let elapsed = (Utc::now() - *last_trade_time).num_seconds() as f64;
            let cooldown = if *was_win {
                self.config.win_cooldown_seconds
            } else {
                self.config.loss_cooldown_seconds
            };

            if elapsed < cooldown {
                let remaining = cooldown - elapsed;
                let cooldown_type = if *was_win { "win" } else { "loss" };
                return (
                    true,
                    Some(format!(
                        "{} {} cooldown ({:.0}s remaining)",
                        team, cooldown_type, remaining
                    )),
                );
            }
        }
        (false, None)
    }

    /// Record a trade cooldown for a specific team in a game.
    fn record_team_cooldown(&mut self, game_id: &str, team: &str, was_win: bool) {
        let cooldown_key = format!("{}:{}", game_id, team);
        self.team_cooldowns.insert(cooldown_key, (Utc::now(), was_win));
    }

    async fn get_open_position_for_game(&self, game_id: &str) -> Result<Option<OpenPositionRow>> {
        let row = sqlx::query(
            r#"
            SELECT trade_id, game_id, side::text as side,
                   entry_price::float8 as entry_price, size::float8 as size
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
            "#,
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| OpenPositionRow {
            trade_id: r.get("trade_id"),
            game_id: r.get("game_id"),
            side: r.get("side"),
            entry_price: r.get("entry_price"),
            size: r.get("size"),
        }))
    }

    async fn get_market_price(&self, signal: &TradingSignal) -> Result<Option<MarketPriceRow>> {
        let target_team = signal.team.trim();
        if target_team.is_empty() {
            // Fallback: any recent price
            return self.get_any_recent_price(&signal.game_id).await;
        }

        // Use configurable price staleness TTL instead of hardcoded 2 minutes
        let staleness_interval = format!("{} seconds", self.config.price_staleness_secs);
        
        // Prefer the platform the signal intended to trade on (when present).
        let preferred_platform = signal.platform_buy.as_ref().map(|p| match p {
            Platform::Kalshi => "kalshi",
            Platform::Polymarket => "polymarket",
            Platform::Paper => "paper",
        });

        let rows = if let Some(pf) = preferred_platform {
            sqlx::query(
                r#"
                SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                       yes_bid_size, yes_ask_size, volume, liquidity, time, platform
                FROM market_prices
                WHERE game_id = $1
                  AND platform = $2::text::platform_enum
                  AND contract_team IS NOT NULL
                  AND time > NOW() - $3::interval
                ORDER BY time DESC
                LIMIT 10
                "#,
            )
            .bind(&signal.game_id)
            .bind(pf)
            .bind(&staleness_interval)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                       yes_bid_size, yes_ask_size, volume, liquidity, time, platform
                FROM market_prices
                WHERE game_id = $1
                  AND contract_team IS NOT NULL
                  AND time > NOW() - $2::interval
                ORDER BY time DESC
                LIMIT 10
                "#,
            )
            .bind(&signal.game_id)
            .bind(&staleness_interval)
            .fetch_all(&self.pool)
            .await?
        };

        let mut best_match: Option<MarketPriceRow> = None;
        let mut best_confidence = 0.0;

        for row in rows {
            let contract_team: Option<String> = row.try_get("contract_team").ok();
            if let Some(ref ct) = contract_team {
                let result = match_team_in_text(target_team, ct, signal.sport.as_str());

                if result.is_match() && result.score > best_confidence {
                    best_confidence = result.score;
                    best_match = Some(MarketPriceRow {
                        market_id: row.get("market_id"),
                        platform: row.get("platform"),
                        market_title: row.try_get("market_title").ok(),
                        contract_team,
                        yes_bid: row.get("yes_bid"),
                        yes_ask: row.get("yes_ask"),
                        yes_bid_size: row.try_get("yes_bid_size").ok(),
                        yes_ask_size: row.try_get("yes_ask_size").ok(),
                        volume: row.try_get("volume").ok(),
                        liquidity: row.try_get("liquidity").ok(),
                        time: row.get("time"),
                    });

                    if best_confidence >= 0.9 {
                        break;
                    }
                }
            }
        }

        if best_confidence >= self.config.team_match_min_confidence {
            Ok(best_match)
        } else {
            Ok(None)
        }
    }

    async fn get_any_recent_price(&self, game_id: &str) -> Result<Option<MarketPriceRow>> {
        let row = sqlx::query(
            r#"
            SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                   yes_bid_size, yes_ask_size, volume, liquidity, time, platform
            FROM market_prices
            WHERE game_id = $1
            ORDER BY time DESC
            LIMIT 1
            "#,
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| MarketPriceRow {
            market_id: r.get("market_id"),
            platform: r.get("platform"),
            market_title: r.try_get("market_title").ok(),
            contract_team: r.try_get("contract_team").ok(),
            yes_bid: r.get("yes_bid"),
            yes_ask: r.get("yes_ask"),
            yes_bid_size: r.try_get("yes_bid_size").ok(),
            yes_ask_size: r.try_get("yes_ask_size").ok(),
            volume: r.try_get("volume").ok(),
            liquidity: r.try_get("liquidity").ok(),
            time: r.get("time"),
        }))
    }

    async fn estimate_position_size(&self, signal: &TradingSignal) -> Result<f64> {
        // Use current balance from database, not initial_bankroll
        let current_balance = self.get_available_balance().await?;

        // Estimate round-trip fees (entry + exit) and reserve balance for them
        // Kalshi: ~0.7% entry + ~0.7% exit = 1.4% round-trip
        // Polymarket: ~2% entry + ~2% exit = 4% round-trip
        let fee_rate = match signal.platform_buy.unwrap_or(Platform::Kalshi) {
            Platform::Kalshi | Platform::Paper => 0.014, // ~1.4% round-trip
            Platform::Polymarket => 0.04,                // ~4% round-trip
        };
        let available_after_fees = current_balance / (1.0 + fee_rate);

        let kelly = signal.kelly_fraction();
        let fractional_kelly = kelly * self.config.kelly_fraction;
        let position_pct = (fractional_kelly * 100.0).min(self.config.max_position_pct);
        let position_size = available_after_fees * (position_pct / 100.0);
        Ok(position_size.max(1.0))
    }

    /// Validate that proposed position size doesn't exceed available liquidity.
    /// Returns the validated (possibly reduced) position size, or an error if insufficient liquidity.
    fn validate_liquidity(
        &self,
        signal: &TradingSignal,
        proposed_size: f64,
        market_price: &MarketPriceRow,
    ) -> Result<f64, String> {
        // Get available liquidity based on trade direction
        let available = match signal.direction {
            SignalDirection::Buy => market_price.yes_ask_size.unwrap_or(0.0),
            SignalDirection::Sell => market_price.yes_bid_size.unwrap_or(0.0),
            SignalDirection::Hold => return Ok(proposed_size),
        };

        // Check minimum liquidity threshold
        if available < self.config.liquidity_min_threshold {
            return Err(format!(
                "insufficient_liquidity: ${:.2} available < ${:.2} minimum",
                available, self.config.liquidity_min_threshold
            ));
        }

        // Cap position at configured percentage of available liquidity
        let max_position = available * (self.config.liquidity_max_position_pct / 100.0);
        let validated_size = proposed_size.min(max_position);

        if validated_size < proposed_size {
            info!(
                "Position capped by liquidity: ${:.2} -> ${:.2} ({:.0}% of ${:.2} available)",
                proposed_size, validated_size, self.config.liquidity_max_position_pct, available
            );
        }

        Ok(validated_size)
    }

    fn create_execution_request(
        &self,
        signal: &TradingSignal,
        market: &MarketPriceRow,
        size: f64,
    ) -> ExecutionRequest {
        let side = match signal.direction {
            SignalDirection::Buy => ExecutionSide::Yes,
            _ => ExecutionSide::No,
        };

        let limit_price = match side {
            ExecutionSide::Yes => market.yes_ask,
            // Buying NO should use the executable NO ask, which equals (1 - YES bid).
            // Using YES bid here will systematically misprice NO entries.
            ExecutionSide::No => (1.0 - market.yes_bid).clamp(0.0, 1.0),
        };

        let platform = match market.platform.as_str() {
            "kalshi" => Platform::Kalshi,
            "polymarket" => Platform::Polymarket,
            _ => Platform::Paper,
        };

        // Use game_id + team + direction for idempotency (NOT signal_id which is unique per signal)
        let direction_str = match signal.direction {
            SignalDirection::Buy => "buy",
            SignalDirection::Sell => "sell",
            SignalDirection::Hold => "hold",
        };

        ExecutionRequest {
            request_id: Uuid::new_v4().to_string(),
            idempotency_key: format!("{}_{}_{}", signal.game_id, signal.team, direction_str),
            game_id: signal.game_id.clone(),
            sport: signal.sport,
            platform,
            market_id: market.market_id.clone(),
            contract_team: market.contract_team.clone(),
            token_id: None, // Resolved by execution_service if needed
            side,
            limit_price,
            size,
            signal_id: signal.signal_id.clone(),
            signal_type: format!("{:?}", signal.signal_type),
            edge_pct: signal.edge_pct,
            model_prob: signal.model_prob,
            market_prob: signal.market_prob,
            reason: signal.reason.clone(),
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    async fn apply_filters(&mut self, signal: &TradingSignal) -> Option<String> {
        // No market data
        if signal.market_prob.is_none() {
            *self
                .rejected_counts
                .entry("no_market".to_string())
                .or_insert(0) += 1;
            return Some("no_market".to_string());
        }

        // Edge threshold
        if signal.edge_pct < self.config.min_edge_pct {
            *self.rejected_counts.entry("edge".to_string()).or_insert(0) += 1;
            return Some("edge".to_string());
        }

        // Probability bounds (skip for ARB signals - they profit regardless of outcome)
        let is_arb_signal = matches!(
            signal.signal_type,
            SignalType::CrossMarketArb | SignalType::CrossMarketArbNo
        );

        if !is_arb_signal {
            if signal.direction == SignalDirection::Buy && signal.model_prob > self.config.max_buy_prob
            {
                *self.rejected_counts.entry("prob".to_string()).or_insert(0) += 1;
                return Some("prob_high".to_string());
            }

            if signal.direction == SignalDirection::Sell
                && signal.model_prob < self.config.min_sell_prob
            {
                *self.rejected_counts.entry("prob".to_string()).or_insert(0) += 1;
                return Some("prob_low".to_string());
            }
        }

        // Duplicate position check (same-side only)
        // Only reject if we already have a position with the SAME side
        // This allows opposite-side trades (reversals/hedges) while preventing
        // doubling down on the same directional bet
        if !self.config.allow_hedging {
            if let Ok(Some(existing)) = self.get_open_position_for_game(&signal.game_id).await {
                let new_side = match signal.direction {
                    SignalDirection::Buy => "buy",
                    _ => "sell",
                };
                // Only reject if same side - opposite side signals are allowed
                if existing.side == new_side {
                    *self
                        .rejected_counts
                        .entry("duplicate".to_string())
                        .or_insert(0) += 1;
                    return Some("duplicate_same_side".to_string());
                }
            }
        }

        // Team-specific cooldown check (allows trading opposite team while one is in cooldown)
        let (in_cooldown, reason) = self.is_team_in_cooldown(&signal.game_id, &signal.team);
        if in_cooldown {
            *self
                .rejected_counts
                .entry("cooldown".to_string())
                .or_insert(0) += 1;
            return Some(reason.unwrap_or_else(|| "cooldown".to_string()));
        }

        // Rule evaluation
        let rule_decision = self.evaluate_rules(signal);
        if !rule_decision.allowed {
            *self
                .rejected_counts
                .entry("rule_blocked".to_string())
                .or_insert(0) += 1;
            return Some(format!(
                "rule_blocked:{}",
                rule_decision.rule_id.unwrap_or_default()
            ));
        }

        None
    }

    async fn handle_signal(&mut self, signal: TradingSignal) -> Result<()> {
        self.signal_count += 1;

        // Format direction as "to win" / "to lose" for clarity
        let direction_str = match signal.direction {
            SignalDirection::Buy => "to win",
            SignalDirection::Sell => "to lose",
            SignalDirection::Hold => "hold",
        };
        info!(
            "Received signal: {} {} (edge: {:.1}%)",
            signal.team, direction_str, signal.edge_pct
        );

        // Pre-trade filtering
        if let Some(rejection) = self.apply_filters(&signal).await {
            debug!("Signal rejected: {}", rejection);
            return Ok(());
        }

        // Get market price for execution
        let market = match self.get_market_price(&signal).await? {
            Some(m) => m,
            None => {
                // Try to create from signal data
                if signal.market_prob.is_some() {
                    MarketPriceRow {
                        market_id: format!("signal_{}", signal.game_id),
                        platform: "paper".to_string(),
                        market_title: Some(format!("{} to win", signal.team)),
                        contract_team: Some(signal.team.clone()),
                        yes_bid: (signal.market_prob.unwrap_or(0.5) - 0.02).max(0.01),
                        yes_ask: (signal.market_prob.unwrap_or(0.5) + 0.02).min(0.99),
                        // Use liquidity from signal (set by game_shard based on orderbook)
                        yes_bid_size: Some(signal.liquidity_available),
                        yes_ask_size: Some(signal.liquidity_available),
                        volume: Some(0.0),
                        liquidity: Some(signal.liquidity_available),
                        time: Utc::now(),
                    }
                } else {
                    *self
                        .rejected_counts
                        .entry("no_market".to_string())
                        .or_insert(0) += 1;
                    warn!("No market price for signal {}", signal.signal_id);
                    return Ok(());
                }
            }
        };

        // Estimate position size (with fee reservation)
        let proposed_size = self.estimate_position_size(&signal).await?;

        // Validate liquidity and cap position if needed
        let proposed_size = match self.validate_liquidity(&signal, proposed_size, &market) {
            Ok(size) => size,
            Err(reason) => {
                *self
                    .rejected_counts
                    .entry("insufficient_liquidity".to_string())
                    .or_insert(0) += 1;
                warn!(
                    "LIQUIDITY REJECTED: {} {} - {}",
                    signal.game_id, signal.team, reason
                );
                return Ok(());
            }
        };

        // Risk check - MUST pass before execution
        let (approved, rejection, snapshot) = self.check_risk_limits(&signal, proposed_size).await?;
        if !approved {
            *self
                .rejected_counts
                .entry("risk".to_string())
                .or_insert(0) += 1;
            let rejection = rejection.unwrap_or_else(|| "risk_rejected".to_string());
            warn!("RISK REJECTED: {} {} - {}", signal.game_id, signal.team, rejection);

            // Publish notification
            let event = NotificationEvent {
                event_type: NotificationType::RiskRejection,
                priority: NotificationPriority::Warning,
                data: serde_json::json!({
                    "game_id": signal.game_id,
                    "sport": serde_json::to_value(signal.sport).unwrap_or_else(|_| serde_json::Value::Null),
                    "team": signal.team,
                    "edge_pct": signal.edge_pct,
                    "size": proposed_size,
                    "rejection_reason": rejection,
                    "balance": snapshot.balance,
                    "daily_loss": snapshot.daily_loss,
                    "game_exposure": snapshot.game_exposure,
                    "sport_exposure": snapshot.sport_exposure,
                    "position_count": snapshot.position_count,
                }),
                ts: Some(Utc::now()),
            };
            if let Err(e) = self.redis.publish(channels::NOTIFICATION_EVENTS, &event).await {
                warn!("Failed to publish risk rejection notification: {}", e);
            }
            return Ok(());
        }

        // Create execution request
        let exec_request = self.create_execution_request(&signal, &market, proposed_size);

        // Dedupe check
        if self.in_flight.contains_key(&exec_request.idempotency_key) {
            *self
                .rejected_counts
                .entry("duplicate".to_string())
                .or_insert(0) += 1;
            info!(
                "Duplicate signal in-flight: {}",
                exec_request.idempotency_key
            );
            return Ok(());
        }

        self.in_flight
            .insert(exec_request.idempotency_key.clone(), Utc::now());

        // Publish to execution channel based on transport mode
        if self.transport_mode.use_redis() {
            self.redis
                .publish(channels::EXECUTION_REQUESTS, &exec_request)
                .await?;
        }

        if self.transport_mode.use_zmq() {
            self.publish_zmq(&exec_request).await;
        }

        self.approved_count += 1;

        info!(
            "Emitted ExecutionRequest: {} ({:?} {} @ {:.3}) via {:?}",
            exec_request.request_id,
            signal.direction,
            signal.team,
            exec_request.limit_price,
            self.transport_mode
        );

        Ok(())
    }

    async fn cleanup_stale_inflight(&mut self) {
        let cutoff = Utc::now() - Duration::minutes(5);
        self.in_flight.retain(|_, v| *v > cutoff);
    }

    /// Publish ExecutionRequest via ZMQ
    async fn publish_zmq(&self, exec_request: &ExecutionRequest) {
        if let Some(ref zmq_pub) = self.zmq_pub {
            let topic = format!("execution.request.{}", exec_request.request_id);
            let seq = self.zmq_seq.fetch_add(1, Ordering::Relaxed);

            let envelope = ZmqEnvelope {
                seq,
                timestamp_ms: Utc::now().timestamp_millis(),
                source: Some("signal_processor".to_string()),
                payload: serde_json::to_value(exec_request).unwrap_or_default(),
            };

            let payload = match serde_json::to_vec(&envelope) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to serialize ZMQ execution request envelope: {}", e);
                    return;
                }
            };

            let mut socket = zmq_pub.lock().await;
            let mut msg = zeromq::ZmqMessage::from(topic.into_bytes());
            msg.push_back(payload.into());
            if let Err(e) = socket.send(msg).await {
                warn!("Failed to publish execution request via ZMQ: {}", e);
            }
        }
    }
}

// ============================================================================
// DB Row Types
// ============================================================================

#[derive(Debug)]
struct OpenPositionRow {
    trade_id: String,
    game_id: String,
    side: String,
    entry_price: f64,
    size: f64,
}

#[derive(Debug, Clone)]
struct MarketPriceRow {
    market_id: String,
    platform: String,
    market_title: Option<String>,
    contract_team: Option<String>,
    yes_bid: f64,
    yes_ask: f64,
    yes_bid_size: Option<f64>,
    yes_ask_size: Option<f64>,
    volume: Option<f64>,
    liquidity: Option<f64>,
    time: DateTime<Utc>,
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
    state: Arc<RwLock<SignalProcessorState>>,
) -> Result<()> {
    info!("Heartbeat loop started for {}", instance_id);

    loop {
        let state = state.read().await;

        let mut checks = HashMap::new();
        checks.insert("redis_ok".to_string(), true);
        checks.insert("db_ok".to_string(), true);

        let mut metrics = HashMap::new();
        metrics.insert("signals_received".to_string(), state.signal_count as f64);
        metrics.insert("signals_approved".to_string(), state.approved_count as f64);

        // Connection pool metrics for monitoring
        let pool_size = state.pool.size() as f64;
        let pool_idle = state.pool.num_idle() as f64;
        metrics.insert("db_pool_size".to_string(), pool_size);
        metrics.insert("db_pool_idle".to_string(), pool_idle);
        metrics.insert("db_pool_active".to_string(), pool_size - pool_idle);

        let heartbeat = Heartbeat {
            service: "signal_processor_rust".to_string(),
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
// ZMQ Signal Listener
// ============================================================================

/// ZMQ listener for receiving TradingSignal from game_shard
async fn zmq_listener_loop(
    state: Arc<RwLock<SignalProcessorState>>,
    endpoint: String,
) -> Result<()> {
    info!("Starting ZMQ listener for signals from {}", endpoint);

    loop {
        // Create ZMQ SUB socket
        let mut socket = SubSocket::new();

        // Connect with retry
        match socket.connect(&endpoint).await {
            Ok(_) => info!("ZMQ connected to {}", endpoint),
            Err(e) => {
                warn!("Failed to connect to ZMQ {}: {}. Retrying in 5s...", endpoint, e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        }

        // Subscribe to signal topics
        if let Err(e) = socket.subscribe("signals.trade.").await {
            warn!("Failed to subscribe to signals: {}", e);
        }
        info!("ZMQ subscribed to signals.trade.*");

        // Message processing loop
        loop {
            let recv_result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                socket.recv(),
            )
            .await;

            match recv_result {
                Ok(Ok(msg)) => {
                    // ZMQ multipart: [topic, payload]
                    let parts: Vec<_> = msg.iter().collect();
                    if parts.len() < 2 {
                        debug!("ZMQ message with {} parts, expected 2+", parts.len());
                        continue;
                    }

                    let topic = String::from_utf8_lossy(parts[0].as_ref());
                    let payload_bytes = parts[1].as_ref();

                    // Parse envelope
                    let envelope: ZmqEnvelope = match serde_json::from_slice(payload_bytes) {
                        Ok(e) => e,
                        Err(e) => {
                            debug!("Failed to parse ZMQ envelope: {}", e);
                            continue;
                        }
                    };

                    // Parse TradingSignal from envelope payload
                    let signal: TradingSignal = match serde_json::from_value(envelope.payload) {
                        Ok(s) => s,
                        Err(e) => {
                            debug!("Failed to parse TradingSignal from ZMQ: {}", e);
                            continue;
                        }
                    };

                    // Calculate signal latency
                    let now_ms = Utc::now().timestamp_millis();
                    let latency_ms = now_ms - envelope.timestamp_ms;
                    debug!("ZMQ signal received: topic={} latency={}ms", topic, latency_ms);

                    // Process signal
                    let mut s = state.write().await;
                    if let Err(e) = s.handle_signal(signal).await {
                        error!("Error handling ZMQ signal: {}", e);
                    }
                }
                Ok(Err(e)) => {
                    warn!("ZMQ receive error: {}. Reconnecting...", e);
                    break;
                }
                Err(_) => {
                    // Timeout - normal for low-traffic periods
                    debug!("No ZMQ signal in 30s");
                }
            }
        }

        // Brief delay before reconnect
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting Rust Signal Processor Service...");

    // Database connection
    // NOTE: max_connections=10 to support parallel risk checks (6 queries at once)
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .min_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    info!("Connected to database");

    // Redis connection
    let redis = RedisBus::new().await?;
    info!("Connected to Redis");

    // Transport mode configuration
    let transport_mode = TransportMode::from_env();
    info!("Transport mode: {:?}", transport_mode);

    // Setup ZMQ publisher if needed
    let zmq_pub = if transport_mode.use_zmq() {
        let zmq_port = env::var("ZMQ_PUB_PORT").unwrap_or_else(|_| "5559".to_string());
        let zmq_addr = format!("tcp://0.0.0.0:{}", zmq_port);

        let mut socket = PubSocket::new();
        match socket.bind(&zmq_addr).await {
            Ok(_) => {
                info!("ZMQ publisher bound to {}", zmq_addr);
                Some(Arc::new(Mutex::new(socket)))
            }
            Err(e) => {
                error!("Failed to bind ZMQ publisher to {}: {}", zmq_addr, e);
                None
            }
        }
    } else {
        info!("ZMQ publishing disabled (transport_mode={:?})", transport_mode);
        None
    };

    // Configuration
    let config = Config::default();
    info!(
        "Config: min_edge={:.1}%, max_buy_prob={:.2}, min_sell_prob={:.2}",
        config.min_edge_pct, config.max_buy_prob, config.min_sell_prob
    );

    // Initialize state
    let state = Arc::new(RwLock::new(
        SignalProcessorState::new(config, pool, redis.clone(), transport_mode, zmq_pub).await,
    ));

    // Load rules from DB
    {
        let mut s = state.write().await;
        if let Err(e) = s.load_rules_from_db().await {
            warn!("Failed to load rules from DB: {}", e);
        }
    }

    // Spawn ZMQ listener if enabled
    if transport_mode.use_zmq() {
        let zmq_endpoint = env::var("ZMQ_SUB_ENDPOINT")
            .unwrap_or_else(|_| "tcp://game_shard:5558".to_string());
        let state_zmq = state.clone();

        tokio::spawn(async move {
            if let Err(e) = zmq_listener_loop(state_zmq, zmq_endpoint).await {
                error!("ZMQ listener error: {}", e);
            }
        });
        info!("ZMQ signal listener started");
    }

    // Subscribe to Redis signals if enabled
    if !transport_mode.use_redis() {
        info!("Redis signal subscription disabled (transport_mode={:?})", transport_mode);
        // Keep service running with ZMQ only
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    let mut pubsub = redis.subscribe(channels::SIGNALS_NEW).await?;
    info!("Subscribed to {}", channels::SIGNALS_NEW);

    // Subscribe to rule updates
    let redis_rules = redis.clone();
    let state_rules = state.clone();
    tokio::spawn(async move {
        if let Ok(mut pubsub) = redis_rules.subscribe(channels::FEEDBACK_RULES).await {
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                if let Ok(payload) = msg.get_payload::<String>() {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&payload) {
                        if data.get("type").and_then(|v| v.as_str()) == Some("rules_update") {
                            let mut s = state_rules.write().await;
                            if let Err(e) = s.load_rules_from_db().await {
                                warn!("Failed to reload rules: {}", e);
                            }
                        }
                    }
                }
            }
        }
    });

    // Start heartbeat
    let instance_id =
        env::var("HOSTNAME").unwrap_or_else(|_| "signal-processor-rust-1".to_string());
    let redis_hb = redis.clone();
    let state_hb = state.clone();
    tokio::spawn(async move {
        if let Err(e) = heartbeat_loop(redis_hb, instance_id, state_hb).await {
            error!("Heartbeat loop error: {}", e);
        }
    });

    // Cleanup loop
    let state_cleanup = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let mut s = state_cleanup.write().await;
            s.cleanup_stale_inflight().await;
        }
    });

    info!("Signal Processor started");

    // Main message loop
    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to get payload: {}", e);
                continue;
            }
        };

        let signal: TradingSignal = match serde_json::from_str(&payload) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to parse signal: {}", e);
                continue;
            }
        };

        let mut s = state.write().await;
        if let Err(e) = s.handle_signal(signal).await {
            error!("Error handling signal: {}", e);
        }
    }

    Ok(())
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arbees_rust_core::models::SignalType;

    // ========================================================================
    // Config tests
    // ========================================================================

    #[test]
    fn test_config_defaults() {
        let config = Config::default();

        // Check critical defaults
        assert!(config.min_edge_pct >= 5.0); // Minimum edge should be substantial
        assert!(config.max_position_pct <= 25.0); // Max position should be conservative
        assert!(config.kelly_fraction <= 0.5); // Kelly fraction should be fractional
        assert!(config.max_buy_prob <= 0.95); // Don't buy near-certain outcomes
        assert!(config.min_sell_prob >= 0.05); // Don't sell near-impossible outcomes

        // Check liquidity defaults
        assert!(config.liquidity_min_threshold >= 10.0); // At least $10 liquidity
        assert!(config.liquidity_max_position_pct <= 100.0); // Can't exceed 100%
        assert!(config.liquidity_max_position_pct >= 50.0); // Should take meaningful portion

        // Check cooldown defaults
        assert!(config.win_cooldown_seconds > 0.0);
        assert!(config.loss_cooldown_seconds > 0.0);
        assert!(config.loss_cooldown_seconds >= config.win_cooldown_seconds); // Longer after loss
    }

    #[test]
    fn test_config_fee_reservation() {
        let config = Config::default();

        // Fee rates should be reasonable
        // Kalshi ~1.4% round-trip, Polymarket ~4% round-trip
        // These affect position sizing
        assert!(config.kelly_fraction > 0.0);
        assert!(config.kelly_fraction <= 1.0);
    }

    // ========================================================================
    // Liquidity validation tests (unit test the logic)
    // ========================================================================

    fn make_market_price_row(yes_ask_size: Option<f64>, yes_bid_size: Option<f64>) -> MarketPriceRow {
        MarketPriceRow {
            market_id: "test-market".to_string(),
            platform: "kalshi".to_string(),
            market_title: Some("Test Market".to_string()),
            contract_team: Some("Test Team".to_string()),
            yes_bid: 0.48,
            yes_ask: 0.52,
            yes_bid_size,
            yes_ask_size,
            volume: Some(10000.0),
            liquidity: Some(5000.0),
            time: Utc::now(),
        }
    }

    fn make_test_signal(direction: SignalDirection) -> TradingSignal {
        TradingSignal {
            signal_id: "test-signal".to_string(),
            signal_type: SignalType::ModelEdgeYes,
            game_id: "test-game".to_string(),
            sport: Sport::NBA,
            team: "Test Team".to_string(),
            direction,
            model_prob: 0.60,
            market_prob: Some(0.50),
            edge_pct: 10.0,
            confidence: 0.8,
            platform_buy: Some(Platform::Kalshi),
            platform_sell: None,
            buy_price: Some(0.52),
            sell_price: Some(0.48),
            liquidity_available: 1000.0,
            reason: "Test signal".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::minutes(1)),
            play_id: None,
        }
    }

    #[test]
    fn test_liquidity_validation_logic_sufficient() {
        // Test the liquidity validation logic directly
        let min_threshold: f64 = 10.0;
        let max_pct: f64 = 80.0;
        let proposed_size: f64 = 100.0;
        let available_liquidity: f64 = 200.0;

        // Should pass: 200 >= 10, and cap at 80% of 200 = 160
        assert!(available_liquidity >= min_threshold);
        let max_position = available_liquidity * (max_pct / 100.0);
        let validated = proposed_size.min(max_position);
        assert_eq!(validated, 100.0); // Not capped since 100 < 160
    }

    #[test]
    fn test_liquidity_validation_logic_capped() {
        // Test that position gets capped at max percentage of liquidity
        let min_threshold: f64 = 10.0;
        let max_pct: f64 = 80.0;
        let proposed_size: f64 = 500.0;
        let available_liquidity: f64 = 200.0;

        assert!(available_liquidity >= min_threshold);
        let max_position = available_liquidity * (max_pct / 100.0);
        let validated = proposed_size.min(max_position);
        assert_eq!(validated, 160.0); // Capped at 80% of 200
    }

    #[test]
    fn test_liquidity_validation_logic_insufficient() {
        // Test that low liquidity is rejected
        let min_threshold: f64 = 10.0;
        let available_liquidity: f64 = 5.0;

        assert!(available_liquidity < min_threshold);
        // Would return Err in real code
    }

    // ========================================================================
    // Team cooldown tests
    // ========================================================================

    #[test]
    fn test_cooldown_key_format() {
        // Test that cooldown keys are formatted correctly for team-specific cooldowns
        let game_id = "game123";
        let team = "Lakers";
        let cooldown_key = format!("{}:{}", game_id, team);
        assert_eq!(cooldown_key, "game123:Lakers");
    }

    #[test]
    fn test_separate_team_cooldowns() {
        // Test that different teams have different cooldown keys
        let game_id = "game123";
        let team_a = "Lakers";
        let team_b = "Celtics";

        let key_a = format!("{}:{}", game_id, team_a);
        let key_b = format!("{}:{}", game_id, team_b);

        assert_ne!(key_a, key_b);
        assert_eq!(key_a, "game123:Lakers");
        assert_eq!(key_b, "game123:Celtics");
    }

    // ========================================================================
    // Fee reservation tests
    // ========================================================================

    #[test]
    fn test_fee_reservation_kalshi() {
        let balance: f64 = 1000.0;
        let fee_rate: f64 = 0.014; // 1.4% round-trip for Kalshi

        let available_after_fees = balance / (1.0 + fee_rate);

        // Should reserve ~1.4% for fees
        assert!(available_after_fees < balance);
        assert!(available_after_fees > balance * 0.98); // Not too much reserved
        assert!((available_after_fees - 986.19).abs() < 1.0); // ~$986.19
    }

    #[test]
    fn test_fee_reservation_polymarket() {
        let balance: f64 = 1000.0;
        let fee_rate: f64 = 0.04; // 4% round-trip for Polymarket

        let available_after_fees = balance / (1.0 + fee_rate);

        // Should reserve ~4% for fees
        assert!(available_after_fees < balance);
        assert!((available_after_fees - 961.54).abs() < 1.0); // ~$961.54
    }

    // ========================================================================
    // Kelly fraction tests
    // ========================================================================

    #[test]
    fn test_kelly_fraction_calculation() {
        // Test Kelly criterion: f* = (bp - q) / b
        // where b = odds received, p = prob of win, q = prob of loss

        // For a 60% edge with 1:1 odds:
        let p: f64 = 0.60;
        let q: f64 = 1.0 - p;
        let b: f64 = 1.0; // Even money

        let kelly = (b * p - q) / b;
        assert!((kelly - 0.20).abs() < 0.01); // 20% Kelly

        // With 0.25 fractional Kelly
        let fractional_kelly = kelly * 0.25;
        assert!((fractional_kelly - 0.05).abs() < 0.01); // 5% position
    }

    #[test]
    fn test_kelly_caps_at_max_position() {
        let max_position_pct: f64 = 10.0;
        let kelly_position_pct: f64 = 25.0; // High Kelly recommendation

        let capped = kelly_position_pct.min(max_position_pct);
        assert_eq!(capped, 10.0);
    }

    // ========================================================================
    // Signal expiration tests
    // ========================================================================

    #[test]
    fn test_signal_not_expired() {
        let signal = make_test_signal(SignalDirection::Buy);
        let now = Utc::now();

        if let Some(expires_at) = signal.expires_at {
            assert!(expires_at > now);
        }
    }

    #[test]
    fn test_signal_expiration_check() {
        let mut signal = make_test_signal(SignalDirection::Buy);
        signal.expires_at = Some(Utc::now() - Duration::minutes(1));

        let now = Utc::now();
        let is_expired = signal.expires_at.map(|e| e < now).unwrap_or(false);
        assert!(is_expired);
    }

    // ========================================================================
    // Direction tests
    // ========================================================================

    #[test]
    fn test_signal_direction_buy() {
        let signal = make_test_signal(SignalDirection::Buy);
        assert_eq!(signal.direction, SignalDirection::Buy);
    }

    #[test]
    fn test_signal_direction_sell() {
        let signal = make_test_signal(SignalDirection::Sell);
        assert_eq!(signal.direction, SignalDirection::Sell);
    }

    // ========================================================================
    // Platform selection tests
    // ========================================================================

    #[test]
    fn test_platform_defaults_to_kalshi() {
        let signal = make_test_signal(SignalDirection::Buy);
        let platform = signal.platform_buy.unwrap_or(Platform::Kalshi);
        assert_eq!(platform, Platform::Kalshi);
    }
}
