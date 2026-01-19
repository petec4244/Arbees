//! Arbees Core - High-performance arbitrage detection and win probability.
//!
//! This module provides:
//! - Cross-market arbitrage detection (Kalshi vs Polymarket)
//! - Model edge detection (model probability vs market prices)
//! - Win probability calculation for multiple sports
//! - Batch processing with SIMD optimization via rayon

mod types;
mod win_prob;

use pyo3::prelude::*;
use rayon::prelude::*;
use std::collections::HashMap;

pub use types::*;
pub use win_prob::*;

/// Find cross-market arbitrage opportunities between two platforms.
///
/// Looks for situations where:
/// - Platform A's YES ask < Platform B's YES bid (buy A, sell B)
/// - Or vice versa
///
/// Returns a list of `ArbitrageOpportunity` objects.
#[pyfunction]
fn find_cross_market_arbitrage(
    market_a: &MarketPrice,
    market_b: &MarketPrice,
    event_id: String,
    sport: Sport,
    market_title: String,
) -> Vec<ArbitrageOpportunity> {
    let mut opportunities = Vec::new();

    // Check if we can buy YES on A and sell YES on B
    if market_a.yes_ask < market_b.yes_bid {
        let edge_pct = (market_b.yes_bid - market_a.yes_ask) * 100.0;
        opportunities.push(ArbitrageOpportunity::new(
            "cross_market_arb".to_string(),
            market_a.platform,
            market_b.platform,
            event_id.clone(),
            sport,
            market_title.clone(),
            edge_pct,
            market_a.yes_ask,
            market_b.yes_bid,
            market_a.liquidity,
            market_b.liquidity,
            true,
        ));
    }

    // Check if we can buy YES on B and sell YES on A
    if market_b.yes_ask < market_a.yes_bid {
        let edge_pct = (market_a.yes_bid - market_b.yes_ask) * 100.0;
        opportunities.push(ArbitrageOpportunity::new(
            "cross_market_arb".to_string(),
            market_b.platform,
            market_a.platform,
            event_id.clone(),
            sport,
            market_title.clone(),
            edge_pct,
            market_b.yes_ask,
            market_a.yes_bid,
            market_b.liquidity,
            market_a.liquidity,
            true,
        ));
    }

    // Check NO side arbitrage (buy NO on A = sell YES on A, etc.)
    // NO bid = 1 - YES ask, NO ask = 1 - YES bid
    let no_ask_a = 1.0 - market_a.yes_bid;
    let no_bid_a = 1.0 - market_a.yes_ask;
    let no_ask_b = 1.0 - market_b.yes_bid;
    let no_bid_b = 1.0 - market_b.yes_ask;

    // Buy NO on A, sell NO on B
    if no_ask_a < no_bid_b {
        let edge_pct = (no_bid_b - no_ask_a) * 100.0;
        let mut opp = ArbitrageOpportunity::new(
            "cross_market_arb_no".to_string(),
            market_a.platform,
            market_b.platform,
            event_id.clone(),
            sport,
            market_title.clone(),
            edge_pct,
            no_ask_a,
            no_bid_b,
            market_a.liquidity,
            market_b.liquidity,
            true,
        );
        opp.description = format!(
            "Buy NO {:?} @ {:.3}, Sell NO {:?} @ {:.3}",
            market_a.platform, no_ask_a, market_b.platform, no_bid_b
        );
        opportunities.push(opp);
    }

    opportunities
}

/// Find model edge opportunities comparing model probability to market prices.
///
/// Generates signals when model probability significantly differs from market prices.
#[pyfunction]
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
                Platform::Sportsbook,  // Represents model
                event_id.clone(),
                sport,
                market_title.clone(),
                edge_pct,
                market.yes_ask,  // Buy at ask
                model_prob,      // "Sell" to model at fair value
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
#[pyfunction]
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

/// Calculate win probability for a team.
///
/// Returns probability (0.0 to 1.0) that the specified team wins.
#[pyfunction]
#[pyo3(name = "calculate_win_probability")]
fn py_calculate_win_probability(state: &GameState, for_home: bool) -> f64 {
    win_prob::calculate_win_probability(state, for_home)
}

/// Batch calculate win probabilities for multiple game states.
///
/// Uses parallel processing for optimal performance.
#[pyfunction]
#[pyo3(name = "batch_calculate_win_probs")]
fn py_batch_calculate_win_probs(states: Vec<GameState>, for_home: bool) -> Vec<f64> {
    win_prob::batch_calculate_win_probs(&states, for_home)
}

/// Calculate win probability change from a play.
#[pyfunction]
#[pyo3(name = "calculate_win_prob_delta")]
fn py_calculate_win_prob_delta(
    old_state: &GameState,
    new_state: &GameState,
    for_home: bool,
) -> f64 {
    win_prob::calculate_win_prob_delta(old_state, new_state, for_home)
}

/// Calculate expected points from NFL field position.
#[pyfunction]
#[pyo3(name = "expected_points")]
fn py_expected_points(yard_line: u8, down: u8, yards_to_go: u8) -> f64 {
    win_prob::expected_points_from_field_position(yard_line, down, yards_to_go)
}

/// Generate trading signal from win probability change.
#[pyfunction]
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

    let direction = if new_prob > market_prob { "BUY" } else { "SELL" };
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
#[pyfunction]
fn batch_scan_arbitrage(
    markets: HashMap<String, Vec<MarketPrice>>,
    sport: Sport,
) -> Vec<ArbitrageOpportunity> {
    markets
        .par_iter()
        .flat_map(|(event_id, prices)| {
            let mut opps = Vec::new();
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
            opps
        })
        .collect()
}

/// Python module definition
#[pymodule]
fn arbees_core(_py: Python, m: &PyModule) -> PyResult<()> {
    // Types
    m.add_class::<Sport>()?;
    m.add_class::<Platform>()?;
    m.add_class::<GameState>()?;
    m.add_class::<MarketPrice>()?;
    m.add_class::<ArbitrageOpportunity>()?;
    m.add_class::<TradingSignal>()?;

    // Arbitrage functions
    m.add_function(wrap_pyfunction!(find_cross_market_arbitrage, m)?)?;
    m.add_function(wrap_pyfunction!(find_model_edges, m)?)?;
    m.add_function(wrap_pyfunction!(detect_lagging_market, m)?)?;
    m.add_function(wrap_pyfunction!(batch_scan_arbitrage, m)?)?;

    // Win probability functions
    m.add_function(wrap_pyfunction!(py_calculate_win_probability, m)?)?;
    m.add_function(wrap_pyfunction!(py_batch_calculate_win_probs, m)?)?;
    m.add_function(wrap_pyfunction!(py_calculate_win_prob_delta, m)?)?;
    m.add_function(wrap_pyfunction!(py_expected_points, m)?)?;

    // Signal generation
    m.add_function(wrap_pyfunction!(generate_signal_from_prob_change, m)?)?;

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
    fn test_no_arbitrage() {
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
    fn test_arbitrage_found() {
        // Kalshi ask 0.48, Poly bid 0.52 = 4% edge
        let kalshi = make_market(Platform::Kalshi, 0.46, 0.48);
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
        assert!((opp.edge_pct - 4.0).abs() < 0.1);
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
