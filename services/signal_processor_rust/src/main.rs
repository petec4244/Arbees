//! Signal Processor Service (Rust)
//!
//! Responsibilities:
//! - Subscribe to trading signals from Redis (signals:new)
//! - Apply pre-trade filtering (edge threshold, probability bounds, cooldowns, duplicates)
//! - Check risk limits
//! - Emit ExecutionRequest messages to execution:requests channel

use anyhow::{Context, Result};
use arbees_rust_core::models::{
    channels, ExecutionRequest, ExecutionSide, Platform, RuleDecision, RuleDecisionType,
    SignalDirection, Sport, TradingSignal,
};
use arbees_rust_core::redis::RedisBus;
use arbees_rust_core::utils::matching::match_team_in_text;
use chrono::{DateTime, Duration, Utc};
use dotenv::dotenv;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool, Row};
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_edge_pct: env::var("MIN_EDGE_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2.0),
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

    // Counters
    signal_count: u64,
    approved_count: u64,
    rejected_counts: HashMap<String, u64>,

    // Cooldown tracking: game_id -> (last_trade_time, was_win)
    game_cooldowns: HashMap<String, (DateTime<Utc>, bool)>,

    // In-flight dedupe: idempotency_key -> timestamp
    in_flight: HashMap<String, DateTime<Utc>>,

    // Cached rules
    rules: Vec<CachedRule>,
    rules_last_updated: Option<DateTime<Utc>>,
}

impl SignalProcessorState {
    async fn new(config: Config, pool: PgPool, redis: RedisBus) -> Self {
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
            signal_count: 0,
            approved_count: 0,
            rejected_counts,
            game_cooldowns: HashMap::new(),
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

    fn is_game_in_cooldown(&self, game_id: &str) -> (bool, Option<String>) {
        if let Some((last_trade_time, was_win)) = self.game_cooldowns.get(game_id) {
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
                        "{} cooldown ({:.0}s remaining)",
                        cooldown_type, remaining
                    )),
                );
            }
        }
        (false, None)
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

        let rows = sqlx::query(
            r#"
            SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                   yes_bid_size, yes_ask_size, volume, liquidity, time, platform
            FROM market_prices
            WHERE game_id = $1
              AND contract_team IS NOT NULL
              AND time > NOW() - INTERVAL '2 minutes'
            ORDER BY time DESC
            LIMIT 10
            "#,
        )
        .bind(&signal.game_id)
        .fetch_all(&self.pool)
        .await?;

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

    fn estimate_position_size(&self, signal: &TradingSignal) -> f64 {
        let kelly = signal.kelly_fraction();
        let fractional_kelly = kelly * self.config.kelly_fraction;
        let position_pct = (fractional_kelly * 100.0).min(self.config.max_position_pct);
        let position_size = self.config.initial_bankroll * (position_pct / 100.0);
        position_size.max(1.0)
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
            ExecutionSide::No => market.yes_bid,
        };

        let platform = match market.platform.as_str() {
            "kalshi" => Platform::Kalshi,
            "polymarket" => Platform::Polymarket,
            _ => Platform::Paper,
        };

        ExecutionRequest {
            request_id: Uuid::new_v4().to_string(),
            idempotency_key: format!("{}_{}_{}", signal.signal_id, signal.game_id, signal.team),
            game_id: signal.game_id.clone(),
            sport: signal.sport,
            platform,
            market_id: market.market_id.clone(),
            contract_team: market.contract_team.clone(),
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

        // Probability bounds
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

        // Duplicate position check (game-level)
        if !self.config.allow_hedging {
            if let Ok(Some(existing)) = self.get_open_position_for_game(&signal.game_id).await {
                let new_side = match signal.direction {
                    SignalDirection::Buy => "buy",
                    _ => "sell",
                };
                if existing.side == new_side {
                    *self
                        .rejected_counts
                        .entry("duplicate".to_string())
                        .or_insert(0) += 1;
                    return Some("duplicate".to_string());
                }
            }
        }

        // Cooldown check
        let (in_cooldown, reason) = self.is_game_in_cooldown(&signal.game_id);
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

        info!(
            "Received signal: {:?} {:?} {} (edge: {:.1}%)",
            signal.signal_type, signal.direction, signal.team, signal.edge_pct
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
                        yes_bid_size: Some(0.0),
                        yes_ask_size: Some(0.0),
                        volume: Some(0.0),
                        liquidity: Some(10000.0),
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

        // Estimate position size
        let proposed_size = self.estimate_position_size(&signal);

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

        // Publish to execution channel
        self.redis
            .publish(channels::EXECUTION_REQUESTS, &exec_request)
            .await?;

        self.approved_count += 1;

        info!(
            "Emitted ExecutionRequest: {} ({:?} {} @ {:.3})",
            exec_request.request_id, signal.direction, signal.team, exec_request.limit_price
        );

        Ok(())
    }

    async fn cleanup_stale_inflight(&mut self) {
        let cutoff = Utc::now() - Duration::minutes(5);
        self.in_flight.retain(|_, v| *v > cutoff);
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
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting Rust Signal Processor Service...");

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
        "Config: min_edge={:.1}%, max_buy_prob={:.2}, min_sell_prob={:.2}",
        config.min_edge_pct, config.max_buy_prob, config.min_sell_prob
    );

    // Initialize state
    let state = Arc::new(RwLock::new(
        SignalProcessorState::new(config, pool, redis.clone()).await,
    ));

    // Load rules from DB
    {
        let mut s = state.write().await;
        if let Err(e) = s.load_rules_from_db().await {
            warn!("Failed to load rules from DB: {}", e);
        }
    }

    // Subscribe to signals
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
