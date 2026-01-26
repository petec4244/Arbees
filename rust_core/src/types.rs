//! Native Rust types with PyO3 bindings for zero-copy Python interop.

#[cfg(feature = "python")]
use pyo3::prelude::*;

use serde::{Deserialize, Serialize};

/// Supported sports
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Sport {
    NFL,
    NBA,
    NHL,
    MLB,
    NCAAF,
    NCAAB,
    MLS,
    #[serde(rename = "SOCCER")]
    Soccer,
    Tennis,
    MMA,
}

#[cfg_attr(feature = "python", pymethods)]
impl Sport {
    /// Get total regulation time in seconds
    #[cfg_attr(feature = "python", getter)]
    pub fn total_seconds(&self) -> u32 {
        match self {
            Sport::NFL | Sport::NCAAF => 3600,  // 60 min (4 x 15 min quarters)
            Sport::NBA => 2880,                 // 48 min (4 x 12 min quarters)
            Sport::NCAAB => 2400,               // 40 min (2 x 20 min halves)
            Sport::NHL => 3600,                 // 60 min (3 x 20 min periods)
            Sport::MLB => 10800,                // ~3 hours avg (9 innings)
            Sport::MLS | Sport::Soccer => 5400, // 90 min (2 x 45 min halves)
            Sport::Tennis => 7200,              // ~2 hours avg
            Sport::MMA => 1500,                 // 25 min (5 x 5 min rounds)
        }
    }

    /// Get number of periods
    #[cfg_attr(feature = "python", getter)]
    pub fn periods(&self) -> u8 {
        match self {
            Sport::NFL | Sport::NCAAF => 4,  // 4 quarters
            Sport::NBA => 4,                 // 4 quarters
            Sport::NCAAB => 2,               // 2 halves
            Sport::NHL => 3,                 // 3 periods
            Sport::MLB => 9,                 // 9 innings
            Sport::MLS | Sport::Soccer => 2, // 2 halves
            Sport::Tennis => 3,              // Best of 3 or 5 sets
            Sport::MMA => 5,                 // Max 5 rounds
        }
    }
}

/// Platform identifier
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Kalshi,
    Polymarket,
    Sportsbook,
    Paper,
}

/// Current game state
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub game_id: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub sport: Sport,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub home_team: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub away_team: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub home_score: u16,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub away_score: u16,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub period: u8,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub time_remaining_seconds: u32,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub possession: Option<String>,
    // NFL/NCAAF specific
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub down: Option<u8>,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub yards_to_go: Option<u8>,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub yard_line: Option<u8>,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub is_redzone: bool,
}

#[cfg_attr(feature = "python", pymethods)]
impl GameState {
    #[cfg_attr(feature = "python", new)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game_id: String,
        sport: Sport,
        home_team: String,
        away_team: String,
        home_score: u16,
        away_score: u16,
        period: u8,
        time_remaining_seconds: u32,
    ) -> Self {
        Self {
            game_id,
            sport,
            home_team,
            away_team,
            home_score,
            away_score,
            period,
            time_remaining_seconds,
            possession: None,
            down: None,
            yards_to_go: None,
            yard_line: None,
            is_redzone: false,
        }
    }

    /// Get total seconds remaining in regulation
    pub fn total_time_remaining(&self) -> u32 {
        // Use the sport's periods() method for correct period count per sport
        let total_periods = self.sport.periods() as u32;
        let periods_remaining = total_periods.saturating_sub(self.period as u32);

        let period_length = self.sport.total_seconds() / total_periods;
        self.time_remaining_seconds + (periods_remaining * period_length)
    }

    /// Fraction of game remaining (0.0 = game over, 1.0 = full game)
    pub fn game_progress(&self) -> f64 {
        let total = self.sport.total_seconds() as f64;
        let remaining = self.total_time_remaining() as f64;
        (remaining / total).clamp(0.0, 1.0)
    }

    /// Score differential (positive = home winning)
    pub fn score_diff(&self) -> i16 {
        self.home_score as i16 - self.away_score as i16
    }
}

/// Market price snapshot
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketPrice {
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub platform: Platform,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub market_id: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub yes_bid: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub yes_ask: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub volume: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub liquidity: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub timestamp_ms: i64,
}

#[cfg_attr(feature = "python", pymethods)]
impl MarketPrice {
    #[cfg_attr(feature = "python", new)]
    pub fn new(
        platform: Platform,
        market_id: String,
        yes_bid: f64,
        yes_ask: f64,
        volume: f64,
        liquidity: f64,
        timestamp_ms: i64,
    ) -> Self {
        Self {
            platform,
            market_id,
            yes_bid,
            yes_ask,
            volume,
            liquidity,
            timestamp_ms,
        }
    }

    /// Mid price
    pub fn mid_price(&self) -> f64 {
        (self.yes_bid + self.yes_ask) / 2.0
    }

    /// Spread in percentage points
    pub fn spread(&self) -> f64 {
        (self.yes_ask - self.yes_bid) * 100.0
    }

    /// No bid (1 - yes_ask)
    pub fn no_bid(&self) -> f64 {
        1.0 - self.yes_ask
    }

    /// No ask (1 - yes_bid)
    pub fn no_ask(&self) -> f64 {
        1.0 - self.yes_bid
    }
}

/// Arbitrage opportunity
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub opportunity_type: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub platform_buy: Platform,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub platform_sell: Platform,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub event_id: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub sport: Sport,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub market_title: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub edge_pct: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub buy_price: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub sell_price: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub implied_profit: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub liquidity_buy: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub liquidity_sell: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub is_risk_free: bool,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub description: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub model_probability: Option<f64>,
}

#[cfg_attr(feature = "python", pymethods)]
impl ArbitrageOpportunity {
    #[cfg_attr(feature = "python", new)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opportunity_type: String,
        platform_buy: Platform,
        platform_sell: Platform,
        event_id: String,
        sport: Sport,
        market_title: String,
        edge_pct: f64,
        buy_price: f64,
        sell_price: f64,
        liquidity_buy: f64,
        liquidity_sell: f64,
        is_risk_free: bool,
    ) -> Self {
        let implied_profit = (sell_price - buy_price) * 100.0;
        let description = format!(
            "Buy {:?} @ {:.3}, Sell {:?} @ {:.3}",
            platform_buy, buy_price, platform_sell, sell_price
        );
        Self {
            opportunity_type,
            platform_buy,
            platform_sell,
            event_id,
            sport,
            market_title,
            edge_pct,
            buy_price,
            sell_price,
            implied_profit,
            liquidity_buy,
            liquidity_sell,
            is_risk_free,
            description,
            model_probability: None,
        }
    }

    /// Max tradeable size based on liquidity
    #[cfg(feature = "python")]
    fn max_size(&self) -> f64 {
        self.liquidity_buy.min(self.liquidity_sell)
    }

    /// Expected profit for given size
    #[cfg(feature = "python")]
    fn expected_profit(&self, size: f64) -> f64 {
        size * (self.sell_price - self.buy_price)
    }
}

/// Trading signal generated from analysis
#[cfg_attr(feature = "python", pyclass)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradingSignal {
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub signal_type: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub game_id: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub sport: Sport,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub team: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub direction: String, // "BUY" or "SELL"
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub model_prob: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub market_prob: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub edge_pct: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub confidence: f64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub reason: String,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub timestamp_ms: i64,
}

#[cfg_attr(feature = "python", pymethods)]
impl TradingSignal {
    #[cfg_attr(feature = "python", new)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        signal_type: String,
        game_id: String,
        sport: Sport,
        team: String,
        direction: String,
        model_prob: f64,
        market_prob: f64,
        confidence: f64,
        reason: String,
        timestamp_ms: i64,
    ) -> Self {
        let edge_pct = (model_prob - market_prob).abs() * 100.0;
        Self {
            signal_type,
            game_id,
            sport,
            team,
            direction,
            model_prob,
            market_prob,
            edge_pct,
            confidence,
            reason,
            timestamp_ms,
        }
    }

    /// Kelly criterion optimal bet fraction
    #[cfg(feature = "python")]
    fn kelly_fraction(&self) -> f64 {
        if self.edge_pct <= 0.0 {
            return 0.0;
        }
        // Kelly = (p * b - q) / b where p=prob, q=1-p, b=odds-1
        let p = self.model_prob;
        let q = 1.0 - p;
        let b = (1.0 / self.market_prob) - 1.0;
        if b <= 0.0 {
            return 0.0;
        }
        ((p * b - q) / b).max(0.0)
    }
}
