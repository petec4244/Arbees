//! SIMD-accelerated arbitrage detection.
//!
//! This module provides:
//! - `check_arbs_simd` - Single market arbitrage detection with SIMD
//! - `check_arbs_scalar` - Scalar fallback for non-SIMD builds
//! - `batch_scan_arbs` - Batch scanning for multiple markets
//!
//! Arb types (returned as bitmask):
//! - Bit 0: PolyYes + KalshiNo (buy YES on Poly, buy NO on Kalshi)
//! - Bit 1: KalshiYes + PolyNo (buy YES on Kalshi, buy NO on Poly)
//! - Bit 2: PolyOnly (buy YES + NO on Polymarket)
//! - Bit 3: KalshiOnly (buy YES + NO on Kalshi)

use crate::atomic_orderbook::kalshi_fee_cents;
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "simd")]
use wide::{i16x8, CmpLt};

/// Arb type bitmask constants
pub const ARB_POLY_YES_KALSHI_NO: u8 = 1 << 0;
pub const ARB_KALSHI_YES_POLY_NO: u8 = 1 << 1;
pub const ARB_POLY_ONLY: u8 = 1 << 2;
pub const ARB_KALSHI_ONLY: u8 = 1 << 3;

/// Check for arbitrage opportunities using SIMD (when available).
///
/// Returns bitmask indicating which arb types are available:
/// - Bit 0: PolyYes + KalshiNo
/// - Bit 1: KalshiYes + PolyNo
/// - Bit 2: PolyOnly
/// - Bit 3: KalshiOnly
///
/// All prices are in cents (0-100).
/// `threshold_cents` is typically 100 (meaning total cost must be < $1.00).
#[inline]
pub fn check_arbs_simd(
    kalshi_yes: u16,
    kalshi_no: u16,
    poly_yes: u16,
    poly_no: u16,
    threshold_cents: u16,
) -> u8 {
    // Calculate Kalshi fees upfront
    let k_yes_fee = kalshi_fee_cents(kalshi_yes);
    let k_no_fee = kalshi_fee_cents(kalshi_no);

    #[cfg(feature = "simd")]
    {
        // Pack all four cost calculations into a SIMD vector
        // Cost 0: PolyYes + KalshiNo + fee
        // Cost 1: KalshiYes + fee + PolyNo
        // Cost 2: PolyYes + PolyNo (no fees on Polymarket)
        // Cost 3: KalshiYes + fee + KalshiNo + fee
        let costs = i16x8::new([
            (poly_yes + kalshi_no + k_no_fee) as i16,
            (kalshi_yes + k_yes_fee + poly_no) as i16,
            (poly_yes + poly_no) as i16,
            (kalshi_yes + k_yes_fee + kalshi_no + k_no_fee) as i16,
            i16::MAX,
            i16::MAX,
            i16::MAX,
            i16::MAX,
        ]);

        let threshold = i16x8::splat(threshold_cents as i16);
        let cmp = costs.cmp_lt(threshold);

        // Extract comparison results
        let results = cmp.to_array();
        let mut mask = 0u8;
        if results[0] != 0 {
            mask |= ARB_POLY_YES_KALSHI_NO;
        }
        if results[1] != 0 {
            mask |= ARB_KALSHI_YES_POLY_NO;
        }
        if results[2] != 0 {
            mask |= ARB_POLY_ONLY;
        }
        if results[3] != 0 {
            mask |= ARB_KALSHI_ONLY;
        }
        mask
    }

    #[cfg(not(feature = "simd"))]
    {
        check_arbs_scalar(kalshi_yes, kalshi_no, poly_yes, poly_no, threshold_cents)
    }
}

/// Scalar fallback for arbitrage detection.
/// Used when SIMD is not available or for validation.
#[inline]
pub fn check_arbs_scalar(
    kalshi_yes: u16,
    kalshi_no: u16,
    poly_yes: u16,
    poly_no: u16,
    threshold_cents: u16,
) -> u8 {
    let k_yes_fee = kalshi_fee_cents(kalshi_yes);
    let k_no_fee = kalshi_fee_cents(kalshi_no);

    let mut mask = 0u8;

    // PolyYes + KalshiNo
    let cost_0 = poly_yes + kalshi_no + k_no_fee;
    if cost_0 < threshold_cents {
        mask |= ARB_POLY_YES_KALSHI_NO;
    }

    // KalshiYes + PolyNo
    let cost_1 = kalshi_yes + k_yes_fee + poly_no;
    if cost_1 < threshold_cents {
        mask |= ARB_KALSHI_YES_POLY_NO;
    }

    // PolyOnly (no fees on Polymarket)
    let cost_2 = poly_yes + poly_no;
    if cost_2 < threshold_cents {
        mask |= ARB_POLY_ONLY;
    }

    // KalshiOnly
    let cost_3 = kalshi_yes + k_yes_fee + kalshi_no + k_no_fee;
    if cost_3 < threshold_cents {
        mask |= ARB_KALSHI_ONLY;
    }

    mask
}

/// Batch scan multiple markets for arbitrage opportunities.
///
/// Returns a vector of (market_index, arb_mask) for markets with detected arbs.
/// Markets with mask == 0 are filtered out.
///
/// Input format: Vec<(kalshi_yes, kalshi_no, poly_yes, poly_no)>
pub fn batch_scan_arbs(markets: &[(u16, u16, u16, u16)], threshold_cents: u16) -> Vec<(usize, u8)> {
    markets
        .iter()
        .enumerate()
        .filter_map(|(i, &(k_yes, k_no, p_yes, p_no))| {
            let mask = check_arbs_simd(k_yes, k_no, p_yes, p_no, threshold_cents);
            if mask != 0 {
                Some((i, mask))
            } else {
                None
            }
        })
        .collect()
}

/// Calculate estimated profit in cents for a given arb type.
///
/// Returns profit = 100 - total_cost (positive means profit).
#[inline]
pub fn calculate_profit_cents(
    kalshi_yes: u16,
    kalshi_no: u16,
    poly_yes: u16,
    poly_no: u16,
    arb_type: u8,
) -> i16 {
    let k_yes_fee = kalshi_fee_cents(kalshi_yes);
    let k_no_fee = kalshi_fee_cents(kalshi_no);

    let cost = match arb_type {
        ARB_POLY_YES_KALSHI_NO => poly_yes + kalshi_no + k_no_fee,
        ARB_KALSHI_YES_POLY_NO => kalshi_yes + k_yes_fee + poly_no,
        ARB_POLY_ONLY => poly_yes + poly_no,
        ARB_KALSHI_ONLY => kalshi_yes + k_yes_fee + kalshi_no + k_no_fee,
        _ => return 0,
    };

    100i16 - cost as i16
}

/// Decode arb mask into human-readable string.
pub fn decode_arb_mask(mask: u8) -> Vec<&'static str> {
    let mut result = Vec::new();
    if mask & ARB_POLY_YES_KALSHI_NO != 0 {
        result.push("PolyYes+KalshiNo");
    }
    if mask & ARB_KALSHI_YES_POLY_NO != 0 {
        result.push("KalshiYes+PolyNo");
    }
    if mask & ARB_POLY_ONLY != 0 {
        result.push("PolyOnly");
    }
    if mask & ARB_KALSHI_ONLY != 0 {
        result.push("KalshiOnly");
    }
    result
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Check for arbs on a single market (Python function).
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "simd_check_arbs"))]
pub fn py_simd_check_arbs(
    kalshi_yes: u16,
    kalshi_no: u16,
    poly_yes: u16,
    poly_no: u16,
    threshold_cents: u16,
) -> u8 {
    check_arbs_simd(kalshi_yes, kalshi_no, poly_yes, poly_no, threshold_cents)
}

/// Batch scan multiple markets (Python function).
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "simd_batch_scan"))]
pub fn py_simd_batch_scan(
    markets: Vec<(u16, u16, u16, u16)>,
    threshold_cents: u16,
) -> Vec<(usize, u8)> {
    batch_scan_arbs(&markets, threshold_cents)
}

/// Calculate profit for a specific arb type (Python function).
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "simd_calculate_profit"))]
pub fn py_simd_calculate_profit(
    kalshi_yes: u16,
    kalshi_no: u16,
    poly_yes: u16,
    poly_no: u16,
    arb_type: u8,
) -> i16 {
    calculate_profit_cents(kalshi_yes, kalshi_no, poly_yes, poly_no, arb_type)
}

/// Decode arb mask to list of strings (Python function).
#[cfg_attr(feature = "python", pyfunction)]
#[cfg_attr(feature = "python", pyo3(name = "simd_decode_mask"))]
pub fn py_simd_decode_mask(mask: u8) -> Vec<String> {
    decode_arb_mask(mask)
        .into_iter()
        .map(String::from)
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arb_detection_poly_yes_kalshi_no() {
        // Poly YES = 40¢, Kalshi NO = 55¢ (+ ~2¢ fee) = 97¢ total < 100¢
        let mask = check_arbs_simd(50, 55, 40, 60, 100);
        assert!(
            mask & ARB_POLY_YES_KALSHI_NO != 0,
            "Should detect PolyYes+KalshiNo arb, got mask: {:#b}",
            mask
        );
    }

    #[test]
    fn test_arb_detection_kalshi_yes_poly_no() {
        // Kalshi YES = 40¢ (+ ~2¢ fee), Poly NO = 55¢ = 97¢ total < 100¢
        let mask = check_arbs_simd(40, 60, 50, 55, 100);
        assert!(
            mask & ARB_KALSHI_YES_POLY_NO != 0,
            "Should detect KalshiYes+PolyNo arb, got mask: {:#b}",
            mask
        );
    }

    #[test]
    fn test_no_arb_when_above_threshold() {
        // Poly YES = 55¢, Kalshi NO = 55¢ (+ fee) = 112¢ > 100¢
        let mask = check_arbs_simd(50, 55, 55, 50, 100);
        assert_eq!(mask, 0, "Should not detect arb above threshold");
    }

    #[test]
    fn test_simd_matches_scalar() {
        // Fuzz test: verify SIMD and scalar produce identical results
        for k_yes in (10..90).step_by(10) {
            for k_no in (10..90).step_by(10) {
                for p_yes in (10..90).step_by(10) {
                    for p_no in (10..90).step_by(10) {
                        let simd = check_arbs_simd(k_yes, k_no, p_yes, p_no, 100);
                        let scalar = check_arbs_scalar(k_yes, k_no, p_yes, p_no, 100);
                        assert_eq!(
                            simd, scalar,
                            "SIMD and scalar mismatch for k_yes={}, k_no={}, p_yes={}, p_no={}",
                            k_yes, k_no, p_yes, p_no
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_batch_scan() {
        let markets = vec![
            (50, 55, 40, 60), // PolyYes+KalshiNo arb (40 + 55 + 2 = 97)
            (50, 50, 50, 50), // No arb (50 + 50 = 100, not < 100)
            (40, 60, 50, 55), // KalshiYes+PolyNo arb (40 + 2 + 55 = 97)
            (80, 80, 80, 80), // No arb
        ];

        let results = batch_scan_arbs(&markets, 100);

        // Should find 2 markets with arbs
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0); // First market
        assert_eq!(results[1].0, 2); // Third market
    }

    #[test]
    fn test_profit_calculation() {
        // PolyYes=40, KalshiNo=55, fee=2 -> cost=97, profit=3
        let profit = calculate_profit_cents(50, 55, 40, 60, ARB_POLY_YES_KALSHI_NO);
        assert_eq!(profit, 3, "Expected 3¢ profit");
    }

    #[test]
    fn test_decode_mask() {
        let mask = ARB_POLY_YES_KALSHI_NO | ARB_KALSHI_ONLY;
        let decoded = decode_arb_mask(mask);
        assert!(decoded.contains(&"PolyYes+KalshiNo"));
        assert!(decoded.contains(&"KalshiOnly"));
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn test_poly_only_arb() {
        // Polymarket has no fees, so YES=45 + NO=50 = 95 < 100
        let mask = check_arbs_simd(50, 50, 45, 50, 100);
        assert!(
            mask & ARB_POLY_ONLY != 0,
            "Should detect PolyOnly arb, got mask: {:#b}",
            mask
        );
    }

    #[test]
    fn test_kalshi_only_arb() {
        // Kalshi: YES=40 (fee~2) + NO=50 (fee~2) = 94 < 100
        let mask = check_arbs_simd(40, 50, 55, 55, 100);
        assert!(
            mask & ARB_KALSHI_ONLY != 0,
            "Should detect KalshiOnly arb, got mask: {:#b}",
            mask
        );
    }

    #[test]
    fn test_edge_cases() {
        // Zero prices
        let mask = check_arbs_simd(0, 0, 0, 0, 100);
        assert!(mask & ARB_POLY_ONLY != 0);
        assert!(mask & ARB_KALSHI_ONLY != 0);

        // Max prices (100 each) - no arbs possible
        let mask = check_arbs_simd(100, 100, 100, 100, 100);
        assert_eq!(mask, 0);
    }
}
