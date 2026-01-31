//! ESPN API parsing and game state utilities
//!
//! This module handles:
//! - Sport parsing from string representations
//! - Overtime detection based on period/quarter
//! - ESPN sport-to-league mapping
//! - Game time formatting
//! - Cross-platform arbitrage detection

use arbees_rust_core::models::Sport;
use arbees_rust_core::simd::{
    check_arbs_simd, calculate_profit_cents, decode_arb_mask, ARB_POLY_YES_KALSHI_NO,
    ARB_KALSHI_YES_POLY_NO,
};
use crate::price::data::MarketPriceData;

/// Parse sport string to Sport enum
pub fn parse_sport(sport: &str) -> Option<Sport> {
    match sport.to_lowercase().as_str() {
        "nfl" => Some(Sport::NFL),
        "ncaaf" => Some(Sport::NCAAF),
        "nba" => Some(Sport::NBA),
        "ncaab" => Some(Sport::NCAAB),
        "nhl" => Some(Sport::NHL),
        "mlb" => Some(Sport::MLB),
        "mls" => Some(Sport::MLS),
        "soccer" => Some(Sport::Soccer),
        "tennis" => Some(Sport::Tennis),
        "mma" => Some(Sport::MMA),
        _ => None,
    }
}

/// Check if a game is in overtime based on sport and period
/// Returns true if the game has exceeded regular periods/innings
pub fn is_overtime(sport: Sport, period: u8) -> bool {
    match sport {
        Sport::NHL => period > 3,       // Regular NHL: 3 periods
        Sport::NBA => period > 4,       // Regular NBA: 4 quarters
        Sport::NFL => period > 4,       // Regular NFL: 4 quarters
        Sport::NCAAF => period > 4,     // Regular NCAAF: 4 quarters
        Sport::NCAAB => period > 2,     // Regular NCAAB: 2 halves
        Sport::MLB => period > 9,       // Regular MLB: 9 innings
        Sport::MLS | Sport::Soccer => period > 2, // Regular soccer: 2 halves
        Sport::Tennis => false,         // Tennis doesn't have overtime
        Sport::MMA => false,            // MMA doesn't have overtime
    }
}

/// Map sport and league codes to ESPN API endpoints
pub fn espn_sport_league(sport: &str) -> Option<(&'static str, &'static str)> {
    match sport.to_lowercase().as_str() {
        "nfl" => Some(("football", "nfl")),
        "ncaaf" => Some(("football", "college-football")),
        "nba" => Some(("basketball", "nba")),
        "ncaab" => Some(("basketball", "mens-college-basketball")),
        "nhl" => Some(("hockey", "nhl")),
        "mlb" => Some(("baseball", "mlb")),
        "mls" => Some(("soccer", "usa.1")),
        "soccer" => Some(("soccer", "eng.1")),
        _ => None,
    }
}

/// Format seconds into a time remaining string like "12:34" or "5:00"
pub fn format_time_remaining(seconds: u32) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}

/// Check for cross-platform arbitrage opportunities using SIMD scanner.
///
/// Returns Some((arb_mask, profit_cents)) if an arb is found, None otherwise.
///
/// Arbitrage exists when:
/// - Kalshi YES + Poly NO < 100¢ (or vice versa)
/// - This means buying both sides guarantees profit
pub fn check_cross_platform_arb(
    kalshi_price: Option<&MarketPriceData>,
    poly_price: Option<&MarketPriceData>,
    min_profit_cents: i16,
) -> Option<(u8, i16)> {
    let (kalshi, poly) = match (kalshi_price, poly_price) {
        (Some(k), Some(p)) => (k, p),
        _ => return None, // Need both platforms for cross-platform arb
    };

    // Convert prices to cents (0-100 scale)
    let k_yes = (kalshi.yes_ask * 100.0).round() as u16;
    let k_no = ((1.0 - kalshi.yes_bid) * 100.0).round() as u16; // NO ask = 1 - YES bid
    let p_yes = (poly.yes_ask * 100.0).round() as u16;
    let p_no = ((1.0 - poly.yes_bid) * 100.0).round() as u16;

    // Use SIMD scanner to check for arbs (threshold 100 = $1.00)
    let arb_mask = check_arbs_simd(k_yes, k_no, p_yes, p_no, 100);

    if arb_mask == 0 {
        return None;
    }

    // Calculate profit for cross-platform arbs only
    let cross_platform_mask = arb_mask & (ARB_POLY_YES_KALSHI_NO | ARB_KALSHI_YES_POLY_NO);

    if cross_platform_mask == 0 {
        return None;
    }

    // Find the most profitable cross-platform arb
    let mut best_profit = 0i16;
    let mut best_mask = 0u8;

    if arb_mask & ARB_POLY_YES_KALSHI_NO != 0 {
        let profit = calculate_profit_cents(k_yes, k_no, p_yes, p_no, ARB_POLY_YES_KALSHI_NO);
        if profit > best_profit {
            best_profit = profit;
            best_mask = ARB_POLY_YES_KALSHI_NO;
        }
    }

    if arb_mask & ARB_KALSHI_YES_POLY_NO != 0 {
        let profit = calculate_profit_cents(k_yes, k_no, p_yes, p_no, ARB_KALSHI_YES_POLY_NO);
        if profit > best_profit {
            best_profit = profit;
            best_mask = ARB_KALSHI_YES_POLY_NO;
        }
    }

    if best_profit >= min_profit_cents {
        Some((best_mask, best_profit))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sport_valid() {
        assert_eq!(parse_sport("nfl"), Some(Sport::NFL));
        assert_eq!(parse_sport("NFL"), Some(Sport::NFL));
        assert_eq!(parse_sport("nba"), Some(Sport::NBA));
        assert_eq!(parse_sport("nhl"), Some(Sport::NHL));
        assert_eq!(parse_sport("mlb"), Some(Sport::MLB));
        assert_eq!(parse_sport("ncaaf"), Some(Sport::NCAAF));
        assert_eq!(parse_sport("ncaab"), Some(Sport::NCAAB));
        assert_eq!(parse_sport("mls"), Some(Sport::MLS));
        assert_eq!(parse_sport("soccer"), Some(Sport::Soccer));
    }

    #[test]
    fn test_parse_sport_invalid() {
        assert_eq!(parse_sport("invalid"), None);
        assert_eq!(parse_sport(""), None);
        assert_eq!(parse_sport("cricket"), None);
    }

    #[test]
    fn test_is_overtime_nhl() {
        assert!(!is_overtime(Sport::NHL, 1));
        assert!(!is_overtime(Sport::NHL, 2));
        assert!(!is_overtime(Sport::NHL, 3));
        assert!(is_overtime(Sport::NHL, 4)); // OT
        assert!(is_overtime(Sport::NHL, 5)); // 2OT
    }

    #[test]
    fn test_is_overtime_nba() {
        assert!(!is_overtime(Sport::NBA, 1));
        assert!(!is_overtime(Sport::NBA, 4));
        assert!(is_overtime(Sport::NBA, 5)); // OT
    }

    #[test]
    fn test_is_overtime_nfl() {
        assert!(!is_overtime(Sport::NFL, 1));
        assert!(!is_overtime(Sport::NFL, 4));
        assert!(is_overtime(Sport::NFL, 5)); // OT
    }

    #[test]
    fn test_is_overtime_ncaab() {
        assert!(!is_overtime(Sport::NCAAB, 1));
        assert!(!is_overtime(Sport::NCAAB, 2));
        assert!(is_overtime(Sport::NCAAB, 3)); // OT (college has 2 halves)
    }

    #[test]
    fn test_is_overtime_mlb() {
        assert!(!is_overtime(Sport::MLB, 1));
        assert!(!is_overtime(Sport::MLB, 9));
        assert!(is_overtime(Sport::MLB, 10)); // Extra innings
    }

    #[test]
    fn test_is_overtime_soccer() {
        assert!(!is_overtime(Sport::Soccer, 1));
        assert!(!is_overtime(Sport::Soccer, 2));
        assert!(is_overtime(Sport::Soccer, 3)); // Extra time
    }

    #[test]
    fn test_format_time_remaining() {
        assert_eq!(format_time_remaining(0), "0:00");
        assert_eq!(format_time_remaining(30), "0:30");
        assert_eq!(format_time_remaining(60), "1:00");
        assert_eq!(format_time_remaining(90), "1:30");
        assert_eq!(format_time_remaining(720), "12:00"); // 12 minutes
        assert_eq!(format_time_remaining(754), "12:34");
    }

    #[test]
    fn test_espn_sport_league_mapping() {
        assert_eq!(espn_sport_league("nfl"), Some(("football", "nfl")));
        assert_eq!(espn_sport_league("ncaaf"), Some(("football", "college-football")));
        assert_eq!(espn_sport_league("nba"), Some(("basketball", "nba")));
        assert_eq!(espn_sport_league("ncaab"), Some(("basketball", "mens-college-basketball")));
        assert_eq!(espn_sport_league("nhl"), Some(("hockey", "nhl")));
        assert_eq!(espn_sport_league("mlb"), Some(("baseball", "mlb")));
        assert_eq!(espn_sport_league("mls"), Some(("soccer", "usa.1")));
        assert_eq!(espn_sport_league("soccer"), Some(("soccer", "eng.1")));
    }

    #[test]
    fn test_espn_sport_league_invalid() {
        assert_eq!(espn_sport_league("invalid"), None);
        assert_eq!(espn_sport_league("tennis"), None);
    }
}
