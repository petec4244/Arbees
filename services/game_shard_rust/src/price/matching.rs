//! Price matching and platform selection logic
//!
//! This module handles:
//! - Finding best prices for a team across platforms
//! - Selecting optimal platform based on model edge
//! - Team matching with fuzzy scoring

use crate::price::data::MarketPriceData;
use arbees_rust_core::models::{Platform, Sport};
use arbees_rust_core::utils::matching::match_team_in_text;
use chrono::Utc;
use std::collections::HashMap;
use super::super::signals::edge::compute_team_net_edge;

/// Find the best Kalshi and Polymarket prices for a given team
///
/// Returns (kalshi_price, polymarket_price) using fuzzy team matching.
/// Prices older than max_age_secs are filtered out.
pub fn find_team_prices<'a>(
    prices: &'a HashMap<String, MarketPriceData>,
    team: &str,
    sport: Sport,
    max_age_secs: i64,
) -> (Option<&'a MarketPriceData>, Option<&'a MarketPriceData>) {
    let mut best_kalshi: Option<(&MarketPriceData, f64)> = None;
    let mut best_poly: Option<(&MarketPriceData, f64)> = None;
    let now = Utc::now();

    for (_key, price) in prices {
        // Skip stale prices
        let age_secs = (now - price.timestamp).num_seconds();
        if age_secs > max_age_secs {
            continue;
        }

        let platform = price.platform.to_lowercase();
        let result = match_team_in_text(team, &price.contract_team, sport.as_str());
        if !result.is_match() {
            continue;
        }

        let score = result.score;
        if platform.contains("kalshi") {
            if best_kalshi.map(|(_, s)| s).unwrap_or(0.0) < score {
                best_kalshi = Some((price, score));
            }
        } else if platform.contains("polymarket") {
            if best_poly.map(|(_, s)| s).unwrap_or(0.0) < score {
                best_poly = Some((price, score));
            }
        }
    }

    (best_kalshi.map(|(p, _)| p), best_poly.map(|(p, _)| p))
}

/// Select the best platform for a *tradeable* model-edge signal on a team.
/// Uses executable entry prices + fees (YES ask for BUY, NO ask for SELL).
pub fn select_best_platform_for_team<'a>(
    model_yes_prob: f64,
    kalshi_price: Option<&'a MarketPriceData>,
    poly_price: Option<&'a MarketPriceData>,
) -> Option<(&'a MarketPriceData, Platform, f64)> {
    let mut best: Option<(&MarketPriceData, Platform, f64)> = None;

    if let Some(k) = kalshi_price {
        let (_dir, _ty, net_edge_pct, _gross_abs, _mid) =
            compute_team_net_edge(model_yes_prob, k, Platform::Kalshi);
        best = Some((k, Platform::Kalshi, net_edge_pct));
    }

    if let Some(p) = poly_price {
        let (_dir, _ty, net_edge_pct, _gross_abs, _mid) =
            compute_team_net_edge(model_yes_prob, p, Platform::Polymarket);
        match best {
            Some((_, _, best_edge)) if best_edge >= net_edge_pct => {}
            _ => best = Some((p, Platform::Polymarket, net_edge_pct)),
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_price(
        market_id: &str,
        platform: &str,
        team: &str,
        yes_bid: f64,
        yes_ask: f64,
    ) -> MarketPriceData {
        MarketPriceData {
            market_id: market_id.to_string(),
            platform: platform.to_string(),
            contract_team: team.to_string(),
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
    fn test_find_team_prices_exact_match() {
        let mut prices = HashMap::new();
        prices.insert(
            "kalshi_celtics_yes".to_string(),
            make_price("m1", "kalshi", "Boston Celtics", 0.48, 0.52),
        );
        prices.insert(
            "poly_celtics_yes".to_string(),
            make_price("m2", "polymarket", "Celtics", 0.47, 0.53),
        );

        let (kalshi, poly) = find_team_prices(&prices, "Celtics", Sport::NBA, 60);

        assert!(kalshi.is_some());
        assert!(poly.is_some());
        assert_eq!(kalshi.unwrap().platform, "kalshi");
        assert_eq!(poly.unwrap().platform, "polymarket");
    }

    #[test]
    fn test_find_team_prices_stale_ignored() {
        let mut prices = HashMap::new();
        let mut stale_price = make_price("m1", "kalshi", "Celtics", 0.48, 0.52);
        stale_price.timestamp = Utc::now() - chrono::Duration::seconds(120);
        prices.insert("stale".to_string(), stale_price);

        let (kalshi, _poly) = find_team_prices(&prices, "Celtics", Sport::NBA, 60);

        assert!(kalshi.is_none()); // Stale prices should be ignored
    }

    #[test]
    fn test_select_best_platform_higher_edge() {
        let kalshi = make_price("m1", "kalshi", "Celtics", 0.40, 0.45); // Lower ask
        let poly = make_price("m2", "polymarket", "Celtics", 0.48, 0.52); // Higher ask

        let result = select_best_platform_for_team(0.60, Some(&kalshi), Some(&poly));

        // Kalshi should be better because it has lower YES ask (better entry)
        assert!(result.is_some());
        let (selected, platform, _edge) = result.unwrap();
        assert_eq!(selected.market_id, "m1");
        assert_eq!(platform, Platform::Kalshi);
    }
}
