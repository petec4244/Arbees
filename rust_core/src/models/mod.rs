// Shared models for Arbees Rust services
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Market type taxonomy for multi-market support
pub mod market_type;
pub use market_type::{CryptoPredictionType, EconomicIndicator, MarketType, PoliticsEventType};

// ============================================================================
// Notification Events (cross-service alerting)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum NotificationPriority {
    Info,
    Warning,
    Error,
    Critical,
}

impl NotificationPriority {
    pub fn rank(&self) -> u8 {
        match self {
            NotificationPriority::Info => 0,
            NotificationPriority::Warning => 1,
            NotificationPriority::Error => 2,
            NotificationPriority::Critical => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    TradeEntry,
    TradeExit,
    RiskRejection,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
    #[serde(rename = "type")]
    pub event_type: NotificationType,
    pub priority: NotificationPriority,
    pub data: serde_json::Value,
    #[serde(default)]
    pub ts: Option<DateTime<Utc>>,
}

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

    /// Total game duration in seconds (regulation time)
    pub fn total_seconds(&self) -> u32 {
        match self {
            Sport::NFL | Sport::NCAAF => 3600,      // 60 minutes
            Sport::NBA => 2880,                      // 48 minutes
            Sport::NCAAB => 2400,                    // 40 minutes
            Sport::NHL => 3600,                      // 60 minutes
            Sport::MLB => 32400,                     // ~9 innings (estimate)
            Sport::MLS | Sport::Soccer => 5400,     // 90 minutes
            Sport::Tennis => 7200,                   // Varies, ~2 hours avg
            Sport::MMA => 900,                       // 3x5 minute rounds
        }
    }
}

// ============================================================================
// Transport Mode Configuration
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    ZmqOnly,
    RedisOnly,
    Both,
}

impl TransportMode {
    pub fn from_env() -> Self {
        match std::env::var("ZMQ_TRANSPORT_MODE").ok().as_deref() {
            Some("zmq_only") => TransportMode::ZmqOnly,
            Some("both") => TransportMode::Both,
            _ => TransportMode::RedisOnly, // default
        }
    }

    pub fn use_zmq(&self) -> bool {
        matches!(self, TransportMode::ZmqOnly | TransportMode::Both)
    }

    pub fn use_redis(&self) -> bool {
        matches!(self, TransportMode::RedisOnly | TransportMode::Both)
    }
}

// ============================================================================
// Game State (for win probability calculation)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FootballState {
    pub down: Option<u8>,
    pub yards_to_go: Option<u8>,
    pub yard_line: Option<u8>,
    pub is_redzone: bool,
    pub timeouts_home: u8,
    pub timeouts_away: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BasketballState {
    pub timeouts_home: u8,
    pub timeouts_away: u8,
    pub home_team_fouls: u8,
    pub away_team_fouls: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HockeyState {
    pub power_play_team: Option<String>,
    pub power_play_seconds_remaining: Option<u16>,
    pub home_goalie_pulled: bool,
    pub away_goalie_pulled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BaseballState {
    pub outs: u8,
    /// Bitmask: 1=1st, 2=2nd, 4=3rd
    pub base_runners: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoccerState {
    pub home_red_cards: u8,
    pub away_red_cards: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SportSpecificState {
    Football(FootballState),
    Basketball(BasketballState),
    Hockey(HockeyState),
    Baseball(BaseballState),
    Soccer(SoccerState),
    Other, // For sports without specific state
}

impl Default for SportSpecificState {
    fn default() -> Self {
        SportSpecificState::Other
    }
}

// ============================================================================
// Non-Sports Market States
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PoliticsState {
    /// Current probability from polling aggregators
    pub current_probability: Option<f64>,
    /// Last poll update timestamp
    pub last_poll_update: Option<DateTime<Utc>>,
    /// Number of polls in average
    pub poll_count: Option<u32>,
    /// Event date (election day, vote day, etc.)
    pub event_date: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EconomicsState {
    /// Current value of indicator (if released)
    pub current_value: Option<f64>,
    /// Consensus forecast value
    pub forecast_value: Option<f64>,
    /// Release/announcement date
    pub release_date: DateTime<Utc>,
    /// Previous value (for comparison)
    pub previous_value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CryptoState {
    /// Current price in USD
    pub current_price: f64,
    /// Target price for prediction
    pub target_price: f64,
    /// Target date for prediction
    pub target_date: DateTime<Utc>,
    /// 24-hour volatility (standard deviation)
    pub volatility_24h: f64,
    /// Current trading volume 24h
    pub volume_24h: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntertainmentState {
    /// Event description
    pub event_description: String,
    /// Event date
    pub event_date: DateTime<Utc>,
    /// Current probability if available
    pub current_probability: Option<f64>,
}

/// Universal market-specific state that encompasses all market types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "market_state_type", rename_all = "snake_case")]
pub enum MarketSpecificState {
    Sport(SportSpecificState),
    Politics(PoliticsState),
    Economics(EconomicsState),
    Crypto(CryptoState),
    Entertainment(EntertainmentState),
}

impl Default for MarketSpecificState {
    fn default() -> Self {
        MarketSpecificState::Sport(SportSpecificState::Other)
    }
}


/// Universal event/game state for all market types
///
/// This struct supports both sports markets (backward compatible) and new
/// market types (politics, economics, crypto, entertainment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameState {
    // ========== Universal Fields (New) ==========
    /// Universal event identifier
    #[serde(default)]
    pub event_id: String,

    /// Market type discriminator
    #[serde(default)]
    pub market_type: Option<MarketType>,

    /// Generic entity A (home_team, candidate_1, indicator, asset)
    #[serde(default)]
    pub entity_a: Option<String>,

    /// Generic entity B (away_team, candidate_2, opponent, null for single-entity)
    #[serde(default)]
    pub entity_b: Option<String>,

    /// Event start time
    #[serde(default)]
    pub event_start: Option<DateTime<Utc>>,

    /// Event end time (None for continuous markets)
    #[serde(default)]
    pub event_end: Option<DateTime<Utc>>,

    /// Market resolution criteria (for non-sports markets)
    #[serde(default)]
    pub resolution_criteria: Option<String>,

    // ========== Legacy Sports Fields (Deprecated) ==========
    /// @deprecated Use event_id instead
    pub game_id: String,

    /// @deprecated Extract from market_type.as_sport()
    pub sport: Sport,

    /// @deprecated Use entity_a instead
    pub home_team: String,

    /// @deprecated Use entity_b instead
    pub away_team: String,

    /// Score for entity A (optional for non-sports)
    #[serde(default)]
    pub home_score: u16,

    /// Score for entity B (optional for non-sports)
    #[serde(default)]
    pub away_score: u16,

    /// Period/quarter/inning (optional for non-sports)
    #[serde(default)]
    pub period: u8,

    /// Time remaining in current period (optional for non-sports)
    #[serde(default)]
    pub time_remaining_seconds: u32,

    /// Possession indicator (sports-specific)
    pub possession: Option<String>,

    // ========== Common Fields ==========
    /// Timestamp when this state was fetched
    #[serde(default = "default_timestamp")]
    pub fetched_at: DateTime<Utc>,

    /// Pre-event probability (home team for sports, entity_a for others)
    #[serde(default)]
    pub pregame_home_prob: Option<f64>,

    /// Market-specific state data
    #[serde(flatten, default)]
    pub sport_specific: SportSpecificState,

    /// Market-specific state (new, for all market types)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_specific: Option<MarketSpecificState>,
}

fn default_timestamp() -> DateTime<Utc> {
    Utc::now()
}

impl GameState {
    // ========== Backward Compatibility Helpers ==========

    /// Get event ID (new field or fall back to game_id)
    pub fn get_event_id(&self) -> &str {
        if !self.event_id.is_empty() {
            &self.event_id
        } else {
            &self.game_id
        }
    }

    /// Get sport (from market_type or legacy field)
    pub fn get_sport(&self) -> Option<Sport> {
        self.market_type
            .as_ref()
            .and_then(|mt| mt.as_sport())
            .or(Some(self.sport))
    }

    /// Get entity A name (new field or fall back to home_team)
    pub fn get_entity_a(&self) -> &str {
        self.entity_a
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(&self.home_team)
    }

    /// Get entity B name (new field or fall back to away_team)
    pub fn get_entity_b(&self) -> Option<&str> {
        self.entity_b
            .as_ref()
            .map(|s| s.as_str())
            .or(Some(self.away_team.as_str()))
    }

    /// Check if this is a sports market
    pub fn is_sport_market(&self) -> bool {
        self.market_type
            .as_ref()
            .map(|mt| mt.is_sport())
            .unwrap_or(true) // Default to true for backward compatibility
    }

    // ========== Existing Methods ==========

    /// Calculate total time remaining in the game (in seconds)
    /// Only applicable to sports markets
    pub fn total_time_remaining(&self) -> u32 {
        if !self.is_sport_market() {
            return 0;
        }

        let sport = match self.get_sport() {
            Some(s) => s,
            None => return 0,
        };

        let period_seconds = match sport {
            Sport::NFL | Sport::NCAAF => 900,   // 15 minutes per quarter
            Sport::NBA => 720,                   // 12 minutes per quarter
            Sport::NCAAB => 1200,                // 20 minutes per half
            Sport::NHL => 1200,                  // 20 minutes per period
            Sport::MLB => 0,                     // Innings-based
            Sport::MLS | Sport::Soccer => 2700, // 45 minutes per half
            Sport::Tennis => 0,                  // Sets-based
            Sport::MMA => 300,                   // 5 minutes per round
        };

        let periods_remaining = match sport {
            Sport::NFL | Sport::NCAAF | Sport::NBA => 4u32.saturating_sub(self.period as u32),
            Sport::NCAAB | Sport::MLS | Sport::Soccer => 2u32.saturating_sub(self.period as u32),
            Sport::NHL => 3u32.saturating_sub(self.period as u32),
            Sport::MMA => 3u32.saturating_sub(self.period as u32),
            Sport::MLB | Sport::Tennis => 0,
        };

        self.time_remaining_seconds + (periods_remaining * period_seconds)
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

    // ========== Universal Fields (New) ==========
    /// Universal event identifier (same as game_id for sports)
    #[serde(default)]
    pub event_id: Option<String>,

    /// Market type discriminator
    #[serde(default)]
    pub market_type: Option<MarketType>,

    /// Entity the signal applies to (team for sports, asset for crypto, etc.)
    #[serde(default)]
    pub entity: Option<String>,
}

impl TradingSignal {
    pub fn is_risk_free(&self) -> bool {
        matches!(
            self.signal_type,
            SignalType::CrossMarketArb | SignalType::CrossMarketArbNo
        )
    }

    // ========== Universal Field Accessors ==========

    /// Get event ID (new field or fallback to game_id)
    pub fn get_event_id(&self) -> &str {
        self.event_id.as_deref().unwrap_or(&self.game_id)
    }

    /// Get entity (new field or fallback to team)
    pub fn get_entity(&self) -> &str {
        self.entity.as_deref().unwrap_or(&self.team)
    }

    /// Get market type (new field or construct from sport)
    pub fn get_market_type(&self) -> MarketType {
        self.market_type
            .clone()
            .unwrap_or_else(|| MarketType::sport(self.sport))
    }

    /// Check if this is a sports signal
    pub fn is_sport_signal(&self) -> bool {
        self.get_market_type().is_sport()
    }

    pub fn kelly_fraction(&self) -> f64 {
        // Check if we have valid inputs
        if self.edge_pct.abs() <= 0.01 || self.market_prob.is_none() {
            return 0.0;
        }
        
        let market_prob = self.market_prob.unwrap_or(0.0);
        if market_prob <= 0.0 || market_prob >= 1.0 {
            return 0.0;
        }
        
        // For sell signals (negative edge), calculate Kelly for betting AGAINST
        // Use the complement probability
        let (p, odds_price) = match self.direction {
            SignalDirection::Sell => {
                // Selling YES = betting NO will happen
                // p = probability of NO, odds based on NO price
                (1.0 - self.model_prob, 1.0 - market_prob)
            }
            _ => {
                // Buying YES = betting YES will happen  
                // p = probability of YES, odds based on YES price
                (self.model_prob, market_prob)
            }
        };
        
        let q = 1.0 - p;
        
        if odds_price <= 0.0 || odds_price >= 1.0 {
            return 0.0;
        }
        
        // Kelly formula: f = (bp - q) / b
        // where b = odds (payout per dollar)
        let b = (1.0 / odds_price) - 1.0;
        if b <= 0.0 {
            return 0.0;
        }
        
        ((p * b - q) / b).max(0.0)
    }

    /// Calculate signal confidence level based on edge and liquidity
    pub fn confidence_level(&self) -> SignalConfidence {
        let edge = self.edge_pct.abs();
        let liquidity = self.liquidity_available;

        // Very high: edge >= 20% AND good liquidity
        if edge >= 20.0 && liquidity >= 500.0 {
            return SignalConfidence::VeryHigh;
        }
        // High: edge >= 15% AND decent liquidity
        if edge >= 15.0 && liquidity >= 200.0 {
            return SignalConfidence::High;
        }
        // Medium: edge >= 10% OR moderate liquidity
        if edge >= 10.0 && liquidity >= 50.0 {
            return SignalConfidence::Medium;
        }
        // Low: everything else
        SignalConfidence::Low
    }
}

// ============================================================================
// Signal Confidence Level
// ============================================================================

/// Confidence level for trading signals
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SignalConfidence {
    VeryHigh,
    High,
    Medium,
    Low,
}

impl SignalConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            SignalConfidence::VeryHigh => "VERY_HIGH",
            SignalConfidence::High => "HIGH",
            SignalConfidence::Medium => "MEDIUM",
            SignalConfidence::Low => "LOW",
        }
    }

    /// Numeric rank for sorting/comparison (higher = more confident)
    pub fn rank(&self) -> u8 {
        match self {
            SignalConfidence::VeryHigh => 4,
            SignalConfidence::High => 3,
            SignalConfidence::Medium => 2,
            SignalConfidence::Low => 1,
        }
    }
}

// ============================================================================
// Mean Reversion Signal (Analytics)
// ============================================================================

/// Tracks mean reversion opportunities based on price Z-scores
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeanReversionSignal {
    /// Z-score of current price vs rolling mean
    /// Positive = price above mean, negative = below
    pub z_score: f64,
    /// Price is significantly above mean (potential sell opportunity)
    pub is_overbought: bool,
    /// Price is significantly below mean (potential buy opportunity)
    pub is_oversold: bool,
    /// Rolling mean price used for comparison
    pub mean_price: f64,
    /// Standard deviation of prices in the window
    pub std_dev: f64,
    /// Suggested direction based on mean reversion theory
    pub suggested_direction: SignalDirection,
}

impl MeanReversionSignal {
    /// Create from price history
    /// Returns None if not enough data for meaningful calculation
    pub fn from_prices(prices: &[f64], current_price: f64, z_threshold: f64) -> Option<Self> {
        if prices.len() < 5 {
            return None;
        }

        let mean: f64 = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance: f64 = prices.iter()
            .map(|p| (p - mean).powi(2))
            .sum::<f64>() / prices.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 0.001 {
            return None; // Not enough variance
        }

        let z_score = (current_price - mean) / std_dev;
        let is_overbought = z_score > z_threshold;
        let is_oversold = z_score < -z_threshold;

        let suggested_direction = if is_overbought {
            SignalDirection::Sell
        } else if is_oversold {
            SignalDirection::Buy
        } else {
            SignalDirection::Hold
        };

        Some(Self {
            z_score,
            is_overbought,
            is_oversold,
            mean_price: mean,
            std_dev,
            suggested_direction,
        })
    }
}

// ============================================================================
// Impact Analysis (Analytics)
// ============================================================================

/// Analyzes the impact of a play/event on win probability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    /// Probability change caused by the event (positive = good for team)
    pub prob_change: f64,
    /// Whether this change is statistically significant
    pub is_significant: bool,
    /// Edge vs market at time of analysis
    pub market_edge: f64,
    /// The event/play that caused the change
    pub trigger_event: Option<String>,
    /// Time of the event
    pub event_time: Option<DateTime<Utc>>,
    /// Whether market has caught up (edge diminished)
    pub market_adjusted: bool,
}

impl ImpactAnalysis {
    /// Determine if a probability change is significant
    /// Based on sport-specific volatility expectations
    pub fn is_change_significant(prob_change: f64, sport: &Sport) -> bool {
        let threshold = match sport {
            Sport::NBA | Sport::NCAAB => 0.03,  // 3% for high-scoring
            Sport::NFL | Sport::NCAAF => 0.05,  // 5% for football
            Sport::NHL => 0.07,                  // 7% for hockey
            Sport::MLB => 0.05,                  // 5% for baseball
            Sport::MLS | Sport::Soccer => 0.07, // 7% for soccer
            Sport::Tennis => 0.04,               // 4% for tennis
            Sport::MMA => 0.10,                  // 10% for MMA
        };
        prob_change.abs() >= threshold
    }

    /// Create from before/after probabilities
    pub fn from_prob_change(
        prob_before: f64,
        prob_after: f64,
        market_prob: f64,
        sport: &Sport,
        trigger: Option<String>,
    ) -> Self {
        let prob_change = prob_after - prob_before;
        let is_significant = Self::is_change_significant(prob_change, sport);
        let market_edge = prob_after - market_prob;

        Self {
            prob_change,
            is_significant,
            market_edge,
            trigger_event: trigger,
            event_time: Some(Utc::now()),
            market_adjusted: market_edge.abs() < 0.02, // Within 2% = adjusted
        }
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
    /// Polymarket CLOB token ID (resolved from market_id + contract_team)
    #[serde(default)]
    pub token_id: Option<String>,
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

    /// Calculate P&L with proper rounding to avoid floating-point precision errors.
    ///
    /// P0-1 Fix: Uses integer arithmetic (cents) internally, then converts back to dollars.
    /// This prevents accumulated rounding errors when summing P&L across many trades.
    pub fn pnl(&self) -> Option<f64> {
        if self.exit_price.is_none() || self.status != TradeStatus::Closed {
            return None;
        }
        let exit = self.exit_price.unwrap();

        // Convert to cents for precision
        let size_cents = (self.size * 100.0).round() as i64;
        let entry_fee_cents = (self.entry_fees * 100.0).round() as i64;
        let exit_fee_cents = (self.exit_fees * 100.0).round() as i64;

        // Calculate gross P&L in cents
        // Price difference * size gives P&L (prices are probabilities 0-1)
        let price_diff = match self.side {
            TradeSide::Buy => exit - self.entry_price,
            TradeSide::Sell => self.entry_price - exit,
        };
        let gross_pnl_cents = (price_diff * size_cents as f64).round() as i64;

        // Net P&L in cents
        let net_pnl_cents = gross_pnl_cents - entry_fee_cents - exit_fee_cents;

        // Convert back to dollars
        Some(net_pnl_cents as f64 / 100.0)
    }

    /// Calculate holding time in seconds
    /// Returns None if trade is still open
    pub fn holding_time_seconds(&self) -> Option<i64> {
        self.exit_time.map(|exit| (exit - self.entry_time).num_seconds())
    }

    /// Calculate holding time as a human-readable duration string
    /// Returns None if trade is still open
    pub fn holding_time_display(&self) -> Option<String> {
        self.holding_time_seconds().map(|secs| {
            if secs < 60 {
                format!("{}s", secs)
            } else if secs < 3600 {
                format!("{}m {}s", secs / 60, secs % 60)
            } else {
                format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
            }
        })
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
    pub const NOTIFICATION_EVENTS: &str = "notification:events";

    // Fault Tolerance Notification Channels
    pub const NOTIFICATIONS_SERVICE_HEALTH: &str = "notifications:service_health";
    pub const NOTIFICATIONS_SERVICE_RESYNC: &str = "notifications:service_resync";
    pub const NOTIFICATIONS_CIRCUIT_BREAKER: &str = "notifications:circuit_breaker";
    pub const NOTIFICATIONS_DEGRADATION: &str = "notifications:degradation";
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_game_state(sport: Sport, period: u8, time_remaining_seconds: u32, sport_specific: SportSpecificState) -> GameState {
        GameState {
            // Universal fields (new)
            event_id: "test".to_string(),
            market_type: Some(MarketType::sport(sport)),
            entity_a: Some("HOME".to_string()),
            entity_b: Some("AWAY".to_string()),
            event_start: Some(Utc::now()),
            event_end: None,
            resolution_criteria: None,
            // Legacy fields
            game_id: "test".to_string(),
            sport,
            home_team: "HOME".to_string(),
            away_team: "AWAY".to_string(),
            home_score: 0,
            away_score: 0,
            period,
            time_remaining_seconds,
            possession: None,
            fetched_at: Utc::now(),
            pregame_home_prob: None,
            sport_specific,
            market_specific: None,
        }
    }

    #[test]
    fn test_nba_total_seconds() {
        // NBA: 4 quarters × 12 minutes = 48 minutes = 2880 seconds
        assert_eq!(Sport::NBA.total_seconds(), 2880);
    }

    #[test]
    fn test_ncaab_total_seconds() {
        // NCAAB: 2 halves × 20 minutes = 40 minutes = 2400 seconds
        assert_eq!(Sport::NCAAB.total_seconds(), 2400);
    }

    #[test]
    fn test_nhl_total_seconds() {
        // NHL: 3 periods × 20 minutes = 60 minutes = 3600 seconds
        assert_eq!(Sport::NHL.total_seconds(), 3600);
    }

    #[test]
    fn test_nfl_total_seconds() {
        // NFL: 4 quarters × 15 minutes = 60 minutes = 3600 seconds
        assert_eq!(Sport::NFL.total_seconds(), 3600);
    }

    #[test]
    fn test_nba_time_remaining_q1() {
        // NBA Q1 with 5:00 left → 5 min + Q2 (12) + Q3 (12) + Q4 (12) = 41 min = 2460 sec
        let state = make_game_state(Sport::NBA, 1, 300, SportSpecificState::Basketball(Default::default()));
        assert_eq!(state.total_time_remaining(), 300 + 3 * 720); // 2460
    }

    #[test]
    fn test_nba_time_remaining_q4() {
        // NBA Q4 with 2:00 left → just 2 minutes = 120 sec
        let state = make_game_state(Sport::NBA, 4, 120, SportSpecificState::Basketball(Default::default()));
        assert_eq!(state.total_time_remaining(), 120);
    }

    #[test]
    fn test_ncaab_time_remaining_1st_half() {
        // NCAAB 1st half with 10:00 left → 10 min + 2nd half (20) = 30 min = 1800 sec
        let state = make_game_state(Sport::NCAAB, 1, 600, SportSpecificState::Basketball(Default::default()));
        assert_eq!(state.total_time_remaining(), 600 + 1 * 1200); // 1800
    }

    #[test]
    fn test_ncaab_time_remaining_2nd_half() {
        // NCAAB 2nd half with 5:00 left → just 5 minutes = 300 sec
        let state = make_game_state(Sport::NCAAB, 2, 300, SportSpecificState::Basketball(Default::default()));
        assert_eq!(state.total_time_remaining(), 300);
    }

    #[test]
    fn test_nhl_time_remaining_1st_period() {
        // NHL 1st period with 10:00 left → 10 min + P2 (20) + P3 (20) = 50 min = 3000 sec
        let state = make_game_state(Sport::NHL, 1, 600, SportSpecificState::Hockey(Default::default()));
        assert_eq!(state.total_time_remaining(), 600 + 2 * 1200); // 3000
    }

    #[test]
    fn test_nhl_time_remaining_3rd_period() {
        // NHL 3rd period with 5:00 left → just 5 minutes = 300 sec
        let state = make_game_state(Sport::NHL, 3, 300, SportSpecificState::Hockey(Default::default()));
        assert_eq!(state.total_time_remaining(), 300);
    }

    #[test]
    fn test_nfl_time_remaining_q1() {
        // NFL Q1 with 10:00 left → 10 min + Q2 (15) + Q3 (15) + Q4 (15) = 55 min = 3300 sec
        let state = make_game_state(Sport::NFL, 1, 600, SportSpecificState::Football(Default::default()));
        assert_eq!(state.total_time_remaining(), 600 + 3 * 900); // 3300
    }

    #[test]
    fn test_nfl_time_remaining_q4() {
        // NFL Q4 with 2:00 left → just 2 minutes = 120 sec
        let state = make_game_state(Sport::NFL, 4, 120, SportSpecificState::Football(Default::default()));
        assert_eq!(state.total_time_remaining(), 120);
    }

    #[test]
    fn test_overtime_handling() {
        // Overtime periods should just use current time remaining (no future periods)
        // NBA OT (period 5)
        let nba_ot = make_game_state(Sport::NBA, 5, 180, SportSpecificState::Basketball(Default::default()));
        assert_eq!(nba_ot.total_time_remaining(), 180);

        // NHL OT (period 4)
        let nhl_ot = make_game_state(Sport::NHL, 4, 300, SportSpecificState::Hockey(Default::default()));
        assert_eq!(nhl_ot.total_time_remaining(), 300);

        // NCAAB OT (period 3)
        let ncaab_ot = make_game_state(Sport::NCAAB, 3, 300, SportSpecificState::Basketball(Default::default()));
        assert_eq!(ncaab_ot.total_time_remaining(), 300);
    }

    // ============================================================================
    // TradingSignal Universal Field Tests
    // ============================================================================

    fn make_test_signal(sport: Sport) -> TradingSignal {
        TradingSignal {
            signal_id: "test-signal-1".to_string(),
            signal_type: SignalType::ModelEdgeYes,
            game_id: "game-123".to_string(),
            sport,
            team: "Lakers".to_string(),
            direction: SignalDirection::Buy,
            model_prob: 0.65,
            market_prob: Some(0.55),
            edge_pct: 10.0,
            confidence: 0.8,
            platform_buy: Some(Platform::Kalshi),
            platform_sell: None,
            buy_price: Some(0.55),
            sell_price: None,
            liquidity_available: 1000.0,
            reason: "Model edge detected".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            play_id: None,
            // Universal fields (new)
            event_id: None,
            market_type: None,
            entity: None,
        }
    }

    #[test]
    fn test_trading_signal_legacy_fallback() {
        // Without universal fields, should fall back to legacy fields
        let signal = make_test_signal(Sport::NBA);

        assert_eq!(signal.get_event_id(), "game-123");
        assert_eq!(signal.get_entity(), "Lakers");
        assert!(signal.get_market_type().is_sport());
        assert!(signal.is_sport_signal());
    }

    #[test]
    fn test_trading_signal_universal_fields() {
        let mut signal = make_test_signal(Sport::NBA);
        signal.event_id = Some("btc-100k-2025".to_string());
        signal.market_type = Some(MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        });
        signal.entity = Some("bitcoin".to_string());

        assert_eq!(signal.get_event_id(), "btc-100k-2025");
        assert_eq!(signal.get_entity(), "bitcoin");
        assert!(!signal.get_market_type().is_sport());
        assert!(!signal.is_sport_signal());
    }

    #[test]
    fn test_trading_signal_serialization_with_universal_fields() {
        let mut signal = make_test_signal(Sport::NBA);
        signal.event_id = Some("event-456".to_string());
        signal.market_type = Some(MarketType::sport(Sport::NBA));
        signal.entity = Some("Lakers".to_string());

        let json = serde_json::to_string(&signal).unwrap();
        assert!(json.contains("\"event_id\":\"event-456\""));
        assert!(json.contains("\"entity\":\"Lakers\""));

        // Verify deserialization
        let deserialized: TradingSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_id, Some("event-456".to_string()));
        assert_eq!(deserialized.entity, Some("Lakers".to_string()));
    }

    #[test]
    fn test_trading_signal_backward_compatible_deserialization() {
        // Test that old signals without universal fields still deserialize
        let json = r#"{
            "signal_id": "old-signal",
            "signal_type": "model_edge_yes",
            "game_id": "game-old",
            "sport": "NBA",
            "team": "Celtics",
            "direction": "buy",
            "model_prob": 0.6,
            "market_prob": 0.5,
            "edge_pct": 10.0,
            "confidence": 0.7,
            "platform_buy": "kalshi",
            "liquidity_available": 500.0,
            "reason": "test",
            "created_at": "2025-01-15T12:00:00Z"
        }"#;

        let signal: TradingSignal = serde_json::from_str(json).unwrap();
        assert!(signal.event_id.is_none());
        assert!(signal.market_type.is_none());
        assert!(signal.entity.is_none());

        // Helpers should still work via fallback
        assert_eq!(signal.get_event_id(), "game-old");
        assert_eq!(signal.get_entity(), "Celtics");
    }
}
