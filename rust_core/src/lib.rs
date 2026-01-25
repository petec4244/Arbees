//! Arbees Core - High-performance arbitrage detection and win probability.
//!
//! This module provides:
//! - Cross-market arbitrage detection (Kalshi vs Polymarket)
//! - Same-platform arbitrage detection (Kalshi-Kalshi, Poly-Poly)
//! - Model edge detection (model probability vs market prices)
//! - Win probability calculation for multiple sports
//! - Batch processing with SIMD optimization via rayon
//! - Lock-free atomic orderbook for real-time price tracking
//! - SIMD-accelerated arbitrage detection
//! - Circuit breaker for risk management
//! - Execution tracking and deduplication
//! - Position tracking and P&L calculation

mod types;
pub mod win_prob;

// Service modules (from arbees_rust_core merge)
pub mod clients;
pub mod db;
pub mod models;
pub mod redis;
pub mod utils;

// Advanced modules from terauss integration
pub mod atomic_orderbook;
pub mod circuit_breaker;
pub mod execution;
pub mod league_config;
pub mod position_tracker;
pub mod simd;
pub mod team_cache;

#[cfg(feature = "python")]
use pyo3::prelude::*;

use rayon::prelude::*;
use std::collections::HashMap;

pub use types::*;
pub use win_prob::*;

/// Find cross-platform arbitrage opportunities (Kalshi vs Polymarket).
///
/// CORRECT LOGIC: Looks for situations where YES + NO < $1.00:
/// - Buy YES on Platform A + Buy NO on Platform B
/// - Or: Buy YES on Platform B + Buy NO on Platform A
///
/// At expiry, exactly ONE side pays $1.00, guaranteeing profit.
///
/// Returns a list of `ArbitrageOpportunity` objects.
#[cfg_attr(feature = "python", pyfunction)]
fn find_cross_market_arbitrage(
    market_a: &MarketPrice,
    market_b: &MarketPrice,
    event_id: String,
    sport: Sport,
    market_title: String,
) -> Vec<ArbitrageOpportunity> {
    let mut opportunities = Vec::new();

    // NO ask = 1.0 - YES bid (to buy NO, we effectively sell YES at the bid price)
    let no_ask_a = 1.0 - market_a.yes_bid;
    let no_ask_b = 1.0 - market_b.yes_bid;

    // Strategy 1: Buy YES on A + Buy NO on B
    let total_cost_1 = market_a.yes_ask + no_ask_b;
    if total_cost_1 < 1.0 {
        let profit = 1.0 - total_cost_1;
        let edge_pct = profit * 100.0;

        let mut opp = ArbitrageOpportunity::new(
            "cross_platform_arb".to_string(),
            market_a.platform,
            market_b.platform,
            event_id.clone(),
            sport,
            market_title.clone(),
            edge_pct,
            market_a.yes_ask, // Buy YES at ask
            no_ask_b,         // Buy NO at ask
            market_a.liquidity,
            market_b.liquidity,
            true,
        );
        opp.description = format!(
            "Buy YES {:?} @ {:.3} + Buy NO {:?} @ {:.3} = {:.3} < 1.00 (profit: {:.3})",
            market_a.platform, market_a.yes_ask, market_b.platform, no_ask_b, total_cost_1, profit
        );
        opportunities.push(opp);
    }

    // Strategy 2: Buy YES on B + Buy NO on A
    let total_cost_2 = market_b.yes_ask + no_ask_a;
    if total_cost_2 < 1.0 {
        let profit = 1.0 - total_cost_2;
        let edge_pct = profit * 100.0;

        let mut opp = ArbitrageOpportunity::new(
            "cross_platform_arb".to_string(),
            market_b.platform,
            market_a.platform,
            event_id.clone(),
            sport,
            market_title.clone(),
            edge_pct,
            market_b.yes_ask, // Buy YES at ask
            no_ask_a,         // Buy NO at ask
            market_b.liquidity,
            market_a.liquidity,
            true,
        );
        opp.description = format!(
            "Buy YES {:?} @ {:.3} + Buy NO {:?} @ {:.3} = {:.3} < 1.00 (profit: {:.3})",
            market_b.platform, market_b.yes_ask, market_a.platform, no_ask_a, total_cost_2, profit
        );
        opportunities.push(opp);
    }

    opportunities
}

/// Find same-platform arbitrage opportunities (Poly-Poly or Kalshi-Kalshi).
///
/// CORRECT LOGIC: YES + NO < $1.00 on the SAME market.
/// Buy BOTH YES and NO, guaranteed $1.00 payout at expiry.
///
/// This is rare but happens during market inefficiencies.
#[cfg_attr(feature = "python", pyfunction)]
fn find_same_platform_arbitrage(
    market: &MarketPrice,
    event_id: String,
    sport: Sport,
    market_title: String,
) -> Option<ArbitrageOpportunity> {
    let no_ask = 1.0 - market.yes_bid;
    let total_cost = market.yes_ask + no_ask;

    if total_cost < 1.0 {
        let profit = 1.0 - total_cost;
        let edge_pct = profit * 100.0;

        let mut opp = ArbitrageOpportunity::new(
            "same_platform_arb".to_string(),
            market.platform,
            market.platform,
            event_id,
            sport,
            market_title,
            edge_pct,
            market.yes_ask,
            no_ask,
            market.liquidity,
            market.liquidity,
            true,
        );
        opp.description = format!(
            "Buy YES @ {:.3} + Buy NO @ {:.3} = {:.3} < 1.00 (profit: {:.3}) on {:?}",
            market.yes_ask, no_ask, total_cost, profit, market.platform
        );
        return Some(opp);
    }

    None
}

/// Find model edge opportunities comparing model probability to market prices.
///
/// Generates signals when model probability significantly differs from market prices.
#[cfg_attr(feature = "python", pyfunction)]
fn find_model_edges(
    market: &MarketPrice,
    model_prob: f64,
    event_id: String,
    sport: Sport,
    market_title: String,
    min_edge_pct: f64,
) -> Vec<ArbitrageOpportunity> {
    let mut opportunities = Vec::new();

    let market_mid = market.mid_price();

    // Model says YES is underpriced
    if model_prob > market_mid {
        let edge_pct = (model_prob - market_mid) * 100.0;
        if edge_pct >= min_edge_pct {
            let mut opp = ArbitrageOpportunity::new(
                "model_edge_yes".to_string(),
                market.platform,
                Platform::Sportsbook, // Represents model
                event_id.clone(),
                sport,
                market_title.clone(),
                edge_pct,
                market.yes_ask, // Buy at ask
                model_prob,     // "Sell" to model at fair value
                market.liquidity,
                0.0,
                false,
            );
            opp.model_probability = Some(model_prob);
            opp.description = format!(
                "Model {:.1}% vs Market {:.1}% - BUY YES",
                model_prob * 100.0,
                market_mid * 100.0
            );
            opportunities.push(opp);
        }
    }

    // Model says NO is underpriced (YES is overpriced)
    let model_no_prob = 1.0 - model_prob;
    let market_no_mid = 1.0 - market_mid;

    if model_no_prob > market_no_mid {
        let edge_pct = (model_no_prob - market_no_mid) * 100.0;
        if edge_pct >= min_edge_pct {
            let no_ask = market.no_ask();
            let mut opp = ArbitrageOpportunity::new(
                "model_edge_no".to_string(),
                market.platform,
                Platform::Sportsbook,
                event_id,
                sport,
                market_title,
                edge_pct,
                no_ask,
                model_no_prob,
                market.liquidity,
                0.0,
                false,
            );
            opp.model_probability = Some(model_no_prob);
            opp.description = format!(
                "Model {:.1}% vs Market {:.1}% - BUY NO",
                model_no_prob * 100.0,
                market_no_mid * 100.0
            );
            opportunities.push(opp);
        }
    }

    opportunities
}

/// Detect lagging/stale markets that haven't updated recently.
#[cfg_attr(feature = "python", pyfunction)]
fn detect_lagging_market(
    market: &MarketPrice,
    current_time_ms: i64,
    stale_threshold_ms: i64,
    event_id: String,
    sport: Sport,
    market_title: String,
) -> Option<ArbitrageOpportunity> {
    let age_ms = current_time_ms - market.timestamp_ms;

    if age_ms > stale_threshold_ms {
        let mut opp = ArbitrageOpportunity::new(
            "lagging_market".to_string(),
            market.platform,
            Platform::Paper,
            event_id,
            sport,
            market_title,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
        );
        opp.description = format!(
            "Market {:?} stale ({:.1}s old)",
            market.platform,
            age_ms as f64 / 1000.0
        );
        Some(opp)
    } else {
        None
    }
}

// Helper function to convert types::GameState to models::GameState
fn convert_game_state(state: &GameState) -> models::GameState {
    let sport = match state.sport {
        Sport::NFL => models::Sport::NFL,
        Sport::NBA => models::Sport::NBA,
        Sport::NHL => models::Sport::NHL,
        Sport::MLB => models::Sport::MLB,
        Sport::NCAAF => models::Sport::NCAAF,
        Sport::NCAAB => models::Sport::NCAAB,
        Sport::MLS => models::Sport::MLS,
        Sport::Soccer => models::Sport::Soccer,
        Sport::Tennis => models::Sport::Tennis,
        Sport::MMA => models::Sport::MMA,
    };

    models::GameState {
        game_id: state.game_id.clone(),
        sport,
        home_team: state.home_team.clone(),
        away_team: state.away_team.clone(),
        home_score: state.home_score,
        away_score: state.away_score,
        period: state.period,
        time_remaining_seconds: state.time_remaining_seconds,
        possession: state.possession.clone(),
        down: state.down,
        yards_to_go: state.yards_to_go,
        yard_line: state.yard_line,
        is_redzone: state.is_redzone,
    }
}

/// Calculate win probability for a team.
///
/// Returns probability (0.0 to 1.0) that the specified team wins.
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "calculate_win_probability"))]
fn py_calculate_win_probability(state: &GameState, for_home: bool) -> f64 {
    let models_state = convert_game_state(state);
    win_prob::calculate_win_probability(&models_state, for_home)
}

/// Batch calculate win probabilities for multiple game states.
///
/// Uses parallel processing for optimal performance.
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "batch_calculate_win_probs"))]
fn py_batch_calculate_win_probs(states: Vec<GameState>, for_home: bool) -> Vec<f64> {
    let models_states: Vec<models::GameState> = states.iter().map(convert_game_state).collect();
    win_prob::batch_calculate_win_probs(&models_states, for_home)
}

/// Calculate win probability change from a play.
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "calculate_win_prob_delta"))]
fn py_calculate_win_prob_delta(
    old_state: &GameState,
    new_state: &GameState,
    for_home: bool,
) -> f64 {
    let old_models = convert_game_state(old_state);
    let new_models = convert_game_state(new_state);
    win_prob::calculate_win_prob_delta(&old_models, &new_models, for_home)
}

/// Calculate expected points from NFL field position.
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "expected_points"))]
fn py_expected_points(yard_line: u8, down: u8, yards_to_go: u8) -> f64 {
    win_prob::expected_points_from_field_position(yard_line, down, yards_to_go)
}

/// Generate trading signal from win probability change.
#[cfg_attr(feature = "python", pyfunction)]
fn generate_signal_from_prob_change(
    game_id: String,
    sport: Sport,
    team: String,
    old_prob: f64,
    new_prob: f64,
    market_prob: f64,
    min_edge_pct: f64,
    timestamp_ms: i64,
) -> Option<TradingSignal> {
    let prob_change = new_prob - old_prob;
    let edge = (new_prob - market_prob) * 100.0;

    // Only signal if:
    // 1. Significant probability change (> 2%)
    // 2. Edge exceeds minimum threshold
    if prob_change.abs() < 0.02 || edge.abs() < min_edge_pct {
        return None;
    }

    let direction = if new_prob > market_prob {
        "BUY"
    } else {
        "SELL"
    };
    let confidence = (prob_change.abs() * 50.0).min(1.0); // Scale to 0-1

    let reason = format!(
        "Win prob changed {:.1}% ({:.1}% → {:.1}%), market at {:.1}%",
        prob_change * 100.0,
        old_prob * 100.0,
        new_prob * 100.0,
        market_prob * 100.0
    );

    Some(TradingSignal::new(
        "win_prob_shift".to_string(),
        game_id,
        sport,
        team,
        direction.to_string(),
        new_prob,
        market_prob,
        confidence,
        reason,
        timestamp_ms,
    ))
}

/// Scan multiple market pairs for arbitrage opportunities in parallel.
#[cfg_attr(feature = "python", pyfunction)]
fn batch_scan_arbitrage(
    markets: HashMap<String, Vec<MarketPrice>>,
    sport: Sport,
) -> Vec<ArbitrageOpportunity> {
    markets
        .par_iter()
        .flat_map(|(event_id, prices)| {
            let mut opps = Vec::new();

            // Cross-platform arbitrage (all pairs)
            for i in 0..prices.len() {
                for j in (i + 1)..prices.len() {
                    opps.extend(find_cross_market_arbitrage(
                        &prices[i],
                        &prices[j],
                        event_id.clone(),
                        sport,
                        format!("Event {}", event_id),
                    ));
                }
            }

            // Same-platform arbitrage (each market individually)
            for price in prices {
                if let Some(opp) = find_same_platform_arbitrage(
                    price,
                    event_id.clone(),
                    sport,
                    format!("Event {}", event_id),
                ) {
                    opps.push(opp);
                }
            }

            opps
        })
        .collect()
}

/// Python module definition
#[cfg(feature = "python")]
#[pymodule]
fn arbees_core(_py: Python, m: &PyModule) -> PyResult<()> {
    // ============================================================================
    // Original Types
    // ============================================================================
    m.add_class::<Sport>()?;
    m.add_class::<Platform>()?;
    m.add_class::<GameState>()?;
    m.add_class::<MarketPrice>()?;
    m.add_class::<ArbitrageOpportunity>()?;
    m.add_class::<TradingSignal>()?;

    // ============================================================================
    // Original Arbitrage Functions
    // ============================================================================
    m.add_function(wrap_pyfunction!(find_cross_market_arbitrage, m)?)?;
    m.add_function(wrap_pyfunction!(find_same_platform_arbitrage, m)?)?;
    m.add_function(wrap_pyfunction!(find_model_edges, m)?)?;
    m.add_function(wrap_pyfunction!(detect_lagging_market, m)?)?;
    m.add_function(wrap_pyfunction!(batch_scan_arbitrage, m)?)?;

    // ============================================================================
    // Win Probability Functions
    // ============================================================================
    m.add_function(wrap_pyfunction!(py_calculate_win_probability, m)?)?;
    m.add_function(wrap_pyfunction!(py_batch_calculate_win_probs, m)?)?;
    m.add_function(wrap_pyfunction!(py_calculate_win_prob_delta, m)?)?;
    m.add_function(wrap_pyfunction!(py_expected_points, m)?)?;

    // ============================================================================
    // Signal Generation
    // ============================================================================
    m.add_function(wrap_pyfunction!(generate_signal_from_prob_change, m)?)?;

    // ============================================================================
    // NEW: Atomic Orderbook (terauss integration)
    // ============================================================================
    m.add_class::<atomic_orderbook::PyAtomicOrderbook>()?;
    m.add_class::<atomic_orderbook::PyGlobalState>()?;
    m.add_function(wrap_pyfunction!(atomic_orderbook::py_kalshi_fee_cents, m)?)?;

    // ============================================================================
    // NEW: SIMD Arbitrage Detection (terauss integration)
    // ============================================================================
    m.add_function(wrap_pyfunction!(simd::py_simd_check_arbs, m)?)?;
    m.add_function(wrap_pyfunction!(simd::py_simd_batch_scan, m)?)?;
    m.add_function(wrap_pyfunction!(simd::py_simd_calculate_profit, m)?)?;
    m.add_function(wrap_pyfunction!(simd::py_simd_decode_mask, m)?)?;

    // ============================================================================
    // NEW: Circuit Breaker (terauss integration)
    // ============================================================================
    m.add_class::<circuit_breaker::PyCircuitBreakerConfig>()?;
    m.add_class::<circuit_breaker::PyCircuitBreaker>()?;

    // ============================================================================
    // NEW: Execution Tracker (terauss integration)
    // ============================================================================
    m.add_class::<execution::PyExecutionTracker>()?;
    m.add_class::<execution::PyFastExecutionRequest>()?;

    // ============================================================================
    // NEW: Position Tracker (terauss integration)
    // ============================================================================
    m.add_class::<position_tracker::PyArbPosition>()?;
    m.add_class::<position_tracker::PyPositionTracker>()?;

    // ============================================================================
    // NEW: Team Cache (terauss integration)
    // ============================================================================
    m.add_class::<team_cache::PyTeamCache>()?;

    // ============================================================================
    // NEW: League Config (terauss integration)
    // ============================================================================
    m.add_class::<league_config::PyLeagueConfig>()?;
    m.add_function(wrap_pyfunction!(league_config::py_get_league_configs, m)?)?;
    m.add_function(wrap_pyfunction!(league_config::py_get_league_config, m)?)?;
    m.add_function(wrap_pyfunction!(league_config::py_get_league_codes, m)?)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_market(platform: Platform, yes_bid: f64, yes_ask: f64) -> MarketPrice {
        MarketPrice::new(
            platform,
            "test-market".to_string(),
            yes_bid,
            yes_ask,
            1000.0,
            500.0,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    #[test]
    fn test_cross_platform_arbitrage_found() {
        // Kalshi YES ask: 0.48, NO ask: 0.50 (from bid of 0.50)
        // Poly YES ask: 0.54, NO ask: 0.48 (from bid of 0.52)
        // Strategy: Buy Kalshi YES (0.48) + Buy Poly NO (0.48) = 0.96 < 1.00 ✓
        let kalshi = make_market(Platform::Kalshi, 0.50, 0.48);
        let poly = make_market(Platform::Polymarket, 0.52, 0.54);

        let opps = find_cross_market_arbitrage(
            &kalshi,
            &poly,
            "event-1".to_string(),
            Sport::NFL,
            "KC vs SF".to_string(),
        );

        assert!(!opps.is_empty());
        let opp = &opps[0];
        assert_eq!(opp.platform_buy, Platform::Kalshi);
        assert_eq!(opp.platform_sell, Platform::Polymarket);
        assert!((opp.edge_pct - 4.0).abs() < 0.1); // 4% profit
    }

    #[test]
    fn test_same_platform_arbitrage() {
        // Market: YES ask 0.48, YES bid 0.50
        // NO ask = 1.0 - YES bid = 1.0 - 0.50 = 0.50
        // Total: 0.48 + 0.50 = 0.98 < 1.00 ✓
        let market = make_market(Platform::Kalshi, 0.50, 0.48);

        let opp = find_same_platform_arbitrage(
            &market,
            "event-1".to_string(),
            Sport::NFL,
            "KC wins".to_string(),
        );

        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert_eq!(opp.platform_buy, Platform::Kalshi);
        assert_eq!(opp.platform_sell, Platform::Kalshi);
        assert!((opp.edge_pct - 2.0).abs() < 0.1); // 2% profit
    }

    #[test]
    fn test_no_arbitrage() {
        // Efficient market: YES ask 0.52, YES bid 0.50
        // NO ask = 1.0 - 0.50 = 0.50
        // Total: 0.52 + 0.50 = 1.02 > 1.00 ✗
        let kalshi = make_market(Platform::Kalshi, 0.50, 0.52);
        let poly = make_market(Platform::Polymarket, 0.49, 0.51);

        let opps = find_cross_market_arbitrage(
            &kalshi,
            &poly,
            "event-1".to_string(),
            Sport::NFL,
            "KC vs SF".to_string(),
        );

        assert!(opps.is_empty());
    }

    #[test]
    fn test_model_edge() {
        let market = make_market(Platform::Kalshi, 0.45, 0.47);
        let model_prob = 0.55; // Model thinks YES is underpriced

        let opps = find_model_edges(
            &market,
            model_prob,
            "event-1".to_string(),
            Sport::NFL,
            "KC wins".to_string(),
            2.0,
        );

        assert!(!opps.is_empty());
        let opp = &opps[0];
        assert_eq!(opp.opportunity_type, "model_edge_yes");
        assert!(opp.edge_pct > 5.0); // 55% vs 46% mid
    }
}
