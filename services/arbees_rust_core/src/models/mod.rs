// Shared models for Arbees Rust services
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Platform & Sport Enums
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Kalshi,
    Polymarket,
    Paper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Yes,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Sport {
    NBA,
    NCAAB,
    NFL,
    NCAAF,
    NHL,
    MLB,
    MLS,
    #[serde(rename = "SOCCER")]
    Soccer,
    Tennis,
    MMA,
}

impl Sport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sport::NBA => "NBA",
            Sport::NCAAB => "NCAAB",
            Sport::NFL => "NFL",
            Sport::NCAAF => "NCAAF",
            Sport::NHL => "NHL",
            Sport::MLB => "MLB",
            Sport::MLS => "MLS",
            Sport::Soccer => "SOCCER",
            Sport::Tennis => "TENNIS",
            Sport::MMA => "MMA",
        }
    }
}

// ============================================================================
// Signal Types & Directions
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    CrossMarketArb,
    CrossMarketArbNo,
    ModelEdgeYes,
    ModelEdgeNo,
    WinProbShift,
    ScoringPlay,
    Turnover,
    MomentumShift,
    MeanReversion,
    Overreaction,
    LaggingMarket,
    LiquidityOpportunity,
    MarketMispricing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalDirection {
    Buy,
    Sell,
    Hold,
}

// ============================================================================
// Trading Signal
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingSignal {
    pub signal_id: String,
    pub signal_type: SignalType,
    pub game_id: String,
    pub sport: Sport,
    pub team: String,
    pub direction: SignalDirection,

    // Probabilities
    pub model_prob: f64,
    pub market_prob: Option<f64>,

    // Edge calculation
    pub edge_pct: f64,
    pub confidence: f64,

    // Execution details
    pub platform_buy: Option<Platform>,
    pub platform_sell: Option<Platform>,
    pub buy_price: Option<f64>,
    pub sell_price: Option<f64>,
    pub liquidity_available: f64,

    // Metadata
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub play_id: Option<String>,
}

impl TradingSignal {
    pub fn is_risk_free(&self) -> bool {
        matches!(
            self.signal_type,
            SignalType::CrossMarketArb | SignalType::CrossMarketArbNo
        )
    }

    pub fn kelly_fraction(&self) -> f64 {
        if self.edge_pct <= 0.0 || self.market_prob.is_none() {
            return 0.0;
        }
        let p = self.model_prob;
        let q = 1.0 - p;
        let market_prob = self.market_prob.unwrap_or(0.0);
        if market_prob <= 0.0 {
            return 0.0;
        }
        let b = (1.0 / market_prob) - 1.0;
        if b <= 0.0 {
            return 0.0;
        }
        ((p * b - q) / b).max(0.0)
    }
}

// ============================================================================
// Execution Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionSide {
    Yes,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Pending,
    Accepted,
    Rejected,
    Filled,
    Partial,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub request_id: String,
    pub idempotency_key: String,
    pub game_id: String,
    pub sport: Sport,
    pub platform: Platform,
    pub market_id: String,
    pub contract_team: Option<String>,
    pub side: ExecutionSide,
    pub limit_price: f64,
    pub size: f64,
    pub signal_id: String,
    pub signal_type: String,
    pub edge_pct: f64,
    pub model_prob: f64,
    pub market_prob: Option<f64>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub request_id: String,
    pub idempotency_key: String,
    pub status: ExecutionStatus,
    pub rejection_reason: Option<String>,
    pub order_id: Option<String>,
    pub filled_qty: f64,
    pub avg_price: f64,
    pub fees: f64,
    pub platform: Platform,
    pub market_id: String,
    pub contract_team: Option<String>,
    pub game_id: String,
    pub sport: Sport,
    pub signal_id: String,
    pub signal_type: String,
    pub edge_pct: f64,
    pub side: ExecutionSide,
    pub requested_at: DateTime<Utc>,
    pub executed_at: DateTime<Utc>,
    pub latency_ms: f64,
}

// ============================================================================
// Trade & Position Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeStatus {
    Pending,
    Open,
    Closed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeOutcome {
    Win,
    Loss,
    Push,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositionState {
    Open,
    Closing,
    Closed,
    Settled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTrade {
    pub trade_id: String,
    pub signal_id: String,
    pub game_id: String,
    pub sport: Sport,
    pub platform: Platform,
    pub market_id: String,
    pub market_title: String,

    // Trade details
    pub side: TradeSide,
    pub signal_type: SignalType,
    pub entry_price: f64,
    pub exit_price: Option<f64>,
    pub size: f64,

    // Risk metrics at entry
    pub model_prob: f64,
    pub edge_at_entry: f64,
    pub kelly_fraction: f64,

    // Execution details
    pub entry_time: DateTime<Utc>,
    pub exit_time: Option<DateTime<Utc>>,
    pub status: TradeStatus,
    pub outcome: TradeOutcome,

    // Fee tracking
    pub entry_fees: f64,
    pub exit_fees: f64,
}

impl PaperTrade {
    pub fn risk_amount(&self) -> f64 {
        match self.side {
            TradeSide::Buy => self.size * self.entry_price,
            TradeSide::Sell => self.size * (1.0 - self.entry_price),
        }
    }

    pub fn pnl(&self) -> Option<f64> {
        if self.exit_price.is_none() || self.status != TradeStatus::Closed {
            return None;
        }
        let exit = self.exit_price.unwrap();
        let gross_pnl = match self.side {
            TradeSide::Buy => self.size * (exit - self.entry_price),
            TradeSide::Sell => self.size * (self.entry_price - exit),
        };
        Some(gross_pnl - self.entry_fees - self.exit_fees)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub position_id: String,
    pub trade_id: String,
    pub state: PositionState,
    pub game_id: String,
    pub sport: Sport,
    pub platform: Platform,
    pub market_id: String,
    pub contract_team: Option<String>,
    pub side: ExecutionSide,
    pub entry_price: f64,
    pub current_price: Option<f64>,
    pub size: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub fees_paid: f64,
    pub exit_price: Option<f64>,
    pub exit_reason: Option<String>,
    pub stop_loss_price: Option<f64>,
    pub take_profit_price: Option<f64>,
    pub opened_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

// ============================================================================
// Market Price
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketPrice {
    pub market_id: String,
    pub platform: Platform,
    pub market_title: Option<String>,
    pub contract_team: Option<String>,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub yes_bid_size: f64,
    pub yes_ask_size: f64,
    pub volume: f64,
    pub liquidity: f64,
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Rule Evaluation
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecisionType {
    Allowed,
    Rejected,
    ThresholdOverride,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDecision {
    pub allowed: bool,
    pub decision_type: RuleDecisionType,
    pub rule_id: Option<String>,
    pub reason: Option<String>,
    pub override_min_edge: Option<f64>,
}

impl Default for RuleDecision {
    fn default() -> Self {
        Self {
            allowed: true,
            decision_type: RuleDecisionType::Allowed,
            rule_id: None,
            reason: None,
            override_min_edge: None,
        }
    }
}

// ============================================================================
// Redis Channel Names
// ============================================================================

pub mod channels {
    pub const SIGNALS_NEW: &str = "signals:new";
    pub const EXECUTION_REQUESTS: &str = "execution:requests";
    pub const EXECUTION_RESULTS: &str = "execution:results";
    pub const POSITION_UPDATES: &str = "positions:updates";
    pub const GAMES_ENDED: &str = "games:ended";
    pub const FEEDBACK_RULES: &str = "feedback:rules";
    pub const HEALTH_HEARTBEATS: &str = "health:heartbeats";
}

// ============================================================================
// Sport-Specific Stop Loss Defaults
// ============================================================================

pub fn get_stop_loss_for_sport(sport: &Sport) -> f64 {
    match sport {
        Sport::NBA | Sport::NCAAB => 3.0,
        Sport::NFL | Sport::NCAAF => 5.0,
        Sport::NHL => 7.0,
        Sport::MLB => 6.0,
        Sport::MLS | Sport::Soccer => 7.0,
        Sport::Tennis => 4.0,
        Sport::MMA => 8.0,
    }
}
