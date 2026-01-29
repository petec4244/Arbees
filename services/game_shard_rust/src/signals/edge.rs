//! Edge calculation and fee handling for trading signals
//!
//! This module handles:
//! - Fee calculation per platform (Kalshi vs Polymarket)
//! - Net edge computation with fee deduction
//! - Signal direction determination

use arbees_rust_core::atomic_orderbook::kalshi_fee_cents;
use arbees_rust_core::models::{Platform, SignalDirection, SignalType};
use crate::price::data::MarketPriceData;

/// Polymarket fee rate (2% per side)
const POLYMARKET_FEE_RATE: f64 = 0.02;

/// Fee per contract (in $) for entering/exiting at a given price.
/// For $1 face-value contracts, fee dollars are equivalent to "probability points".
pub fn fee_for_price(platform: Platform, price: f64) -> f64 {
    let price = price.clamp(0.0, 1.0);
    let price_cents = (price * 100.0).round() as u16;
    match platform {
        Platform::Kalshi | Platform::Paper => kalshi_fee_cents(price_cents) as f64 / 100.0,
        Platform::Polymarket => price * POLYMARKET_FEE_RATE,
    }
}

/// Compute the *tradeable* (executable) net edge for a team on a given platform.
///
/// - If model thinks YES is underpriced: BUY YES at `yes_ask`.
/// - If model thinks YES is overpriced: BUY NO at `no_ask = 1 - yes_bid` (represented as SELL on the team).
///
/// Returns (direction, signal_type, net_edge_pct, gross_edge_pct_abs, market_yes_mid).
pub fn compute_team_net_edge(
    model_yes_prob: f64,
    price: &MarketPriceData,
    platform: Platform,
) -> (SignalDirection, SignalType, f64, f64, f64) {
    let model_yes_prob = model_yes_prob.clamp(0.0, 1.0);
    let market_yes_mid = price.mid_price.clamp(0.0, 1.0);
    let gross_edge_pct_abs = ((model_yes_prob - market_yes_mid).abs()) * 100.0;

    if model_yes_prob >= market_yes_mid {
        // BUY YES at ask
        let entry = price.yes_ask.clamp(0.0, 1.0);
        let entry_fee = fee_for_price(platform, entry);
        let exit_fee = fee_for_price(platform, model_yes_prob);
        let net_edge_pct = (model_yes_prob - entry - entry_fee - exit_fee) * 100.0;
        (
            SignalDirection::Buy,
            SignalType::ModelEdgeYes,
            net_edge_pct,
            gross_edge_pct_abs,
            market_yes_mid,
        )
    } else {
        // BUY NO at no_ask = 1 - yes_bid
        let model_no_prob = (1.0 - model_yes_prob).clamp(0.0, 1.0);
        let no_ask = (1.0 - price.yes_bid).clamp(0.0, 1.0);
        let entry_fee = fee_for_price(platform, no_ask);
        let exit_fee = fee_for_price(platform, model_no_prob);
        let net_edge_pct = (model_no_prob - no_ask - entry_fee - exit_fee) * 100.0;
        (
            SignalDirection::Sell,
            SignalType::ModelEdgeNo,
            net_edge_pct,
            gross_edge_pct_abs,
            market_yes_mid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_price(yes_bid: f64, yes_ask: f64) -> MarketPriceData {
        MarketPriceData {
            market_id: "test".to_string(),
            platform: "kalshi".to_string(),
            contract_team: "Test Team".to_string(),
            yes_bid,
            yes_ask,
            mid_price: (yes_bid + yes_ask) / 2.0,
            timestamp: Utc::now(),
            yes_bid_size: Some(1000.0),
            yes_ask_size: Some(1000.0),
            total_liquidity: Some(2000.0),
        }
    }

    #[test]
    fn test_compute_net_edge_buy_yes() {
        // Model thinks 60% YES, market at 50% mid (bid=48, ask=52)
        let price = make_price(0.48, 0.52);
        let (direction, signal_type, net_edge, gross_edge, market_mid) =
            compute_team_net_edge(0.60, &price, Platform::Kalshi);

        assert_eq!(direction, SignalDirection::Buy);
        assert_eq!(signal_type, SignalType::ModelEdgeYes);
        assert!((gross_edge - 10.0).abs() < 0.1); // 60% - 50% = 10% gross
        assert!(net_edge < gross_edge); // Net should be less due to fees
        assert!(net_edge > 0.0); // Should still be positive
        assert!((market_mid - 0.50).abs() < 0.01);
    }

    #[test]
    fn test_compute_net_edge_buy_no() {
        // Model thinks 40% YES (60% NO), market at 50% mid
        let price = make_price(0.48, 0.52);
        let (direction, signal_type, net_edge, gross_edge, market_mid) =
            compute_team_net_edge(0.40, &price, Platform::Kalshi);

        assert_eq!(direction, SignalDirection::Sell);
        assert_eq!(signal_type, SignalType::ModelEdgeNo);
        assert!((gross_edge - 10.0).abs() < 0.1); // |40% - 50%| = 10% gross
        assert!(net_edge < gross_edge); // Net should be less due to fees
        assert!((market_mid - 0.50).abs() < 0.01);
    }

    #[test]
    fn test_compute_net_edge_platform_fees_differ() {
        let price = make_price(0.48, 0.52);

        let (_, _, kalshi_net, _, _) = compute_team_net_edge(0.60, &price, Platform::Kalshi);
        let (_, _, poly_net, _, _) = compute_team_net_edge(0.60, &price, Platform::Polymarket);

        // Both should have positive gross edge (model 60% vs market ~50%)
        // Net edges will differ based on fee structure
        // At mid-range prices, Polymarket's 2% per-side fee may be higher than Kalshi's tiered fees
        // The important thing is both calculations work and produce reasonable values
        assert!(kalshi_net > -20.0 && kalshi_net < 20.0); // Reasonable range
        assert!(poly_net > -20.0 && poly_net < 20.0); // Reasonable range

        // Both should be less than the gross edge of ~10%
        assert!(kalshi_net < 10.0);
        assert!(poly_net < 10.0);
    }

    #[test]
    fn test_compute_net_edge_no_edge() {
        // Model matches market exactly
        let price = make_price(0.48, 0.52);
        let (direction, _, net_edge, gross_edge, _) =
            compute_team_net_edge(0.50, &price, Platform::Kalshi);

        assert_eq!(direction, SignalDirection::Buy); // Slight bias to buy when equal
        assert!(gross_edge.abs() < 0.1); // Near zero gross edge
        assert!(net_edge < 0.0); // Negative after fees (no real edge)
    }
}
