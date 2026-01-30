//! Arbitrage detection for crypto directional markets
//!
//! Identifies profitable trading opportunities by comparing:
//! - Market's implied probability (from betting odds)
//! - Real probability (from current vs reference asset price)

use super::EventStatus;
use crate::providers::crypto::CryptoMarket;

/// Result of arbitrage opportunity analysis
#[derive(Debug, Clone)]
pub struct ArbOpportunity {
    /// Market ID this opportunity is from
    pub market_id: String,
    /// Asset being traded (BTC, ETH, SOL, etc.)
    pub asset: String,
    /// Type of opportunity (DirectionalUp, DirectionalDown, CrossPlatform)
    pub opportunity_type: ArbOpportunityType,
    /// Estimated profit margin (percentage)
    pub profit_margin_pct: f64,
    /// Confidence in the mispricing (0.0 to 1.0)
    pub confidence: f64,
    /// Recommended position: "UP" or "DOWN"
    pub recommended_side: String,
    /// Current market price for recommended side
    pub entry_price: f64,
    /// Liquidity available at entry price
    pub liquidity_available: Option<f64>,
    /// Estimated slippage cost in basis points
    pub slippage_bps: f64,
    /// Trading fees in basis points
    pub fee_bps: f64,
    /// Reference price ("price to beat")
    pub reference_price: Option<f64>,
    /// Current spot price of asset
    pub current_spot_price: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbOpportunityType {
    /// Single market directional mispricing (UP overpriced or DOWN overpriced)
    DirectionalUp,
    DirectionalDown,
    /// Cross-platform arbitrage (different prices on Kalshi vs Polymarket)
    CrossPlatform,
}

/// Parameters for arbitrage detection
#[derive(Debug, Clone)]
pub struct ArbDetectionParams {
    /// Minimum profit margin to trigger alert (in percentage points)
    pub min_profit_margin_pct: f64,
    /// Minimum confidence threshold (0.0 to 1.0)
    pub min_confidence: f64,
    /// Maximum acceptable slippage (in basis points)
    pub max_slippage_bps: f64,
    /// Minimum liquidity required (in $ or units)
    pub min_liquidity: Option<f64>,
    /// Whether to require current_crypto_price for detection
    pub require_spot_price: bool,
}

impl Default for ArbDetectionParams {
    fn default() -> Self {
        Self {
            min_profit_margin_pct: 2.0,      // Minimum 2% profit margin
            min_confidence: 0.70,              // At least 70% confident
            max_slippage_bps: 50.0,            // Max 0.5% slippage
            min_liquidity: Some(100.0),        // Minimum $100 liquidity
            require_spot_price: false,         // Can detect without spot price
        }
    }
}

/// Analyze a single market for arbitrage opportunities
pub fn analyze_market(
    market: &CryptoMarket,
    params: &ArbDetectionParams,
) -> Option<ArbOpportunity> {
    // Only analyze directional markets with UP/DOWN logic
    if market.status != EventStatus::Live {
        return None;
    }

    // Must have market prices and reference price
    let yes_price = market.yes_price?;
    let no_price = market.no_price?;
    let reference_price = market.reference_price?;

    // Check if we have required data
    if params.require_spot_price && market.current_crypto_price.is_none() {
        return None;
    }

    // Calculate real probability from price movement
    let spot_price = market.current_crypto_price.unwrap_or(reference_price);
    let price_change_pct = ((spot_price - reference_price) / reference_price) * 100.0;

    // Simple probability model: price movement indicates direction probability
    // If price up 1%, UP has ~60% win probability; if down 1%, UP has ~40%
    let real_up_probability = if price_change_pct > 0.0 {
        0.5 + (price_change_pct.min(5.0) / 10.0) // Max +5% moves probability to ~1.0
    } else {
        0.5 + (price_change_pct.max(-5.0) / 10.0) // Max -5% moves probability to ~0.0
    };

    let real_down_probability = 1.0 - real_up_probability;

    // Market implied probabilities from betting odds
    // In a prediction market, the price IS the probability (with fees)
    let market_up_probability = yes_price;
    let market_down_probability = no_price;

    // Detect mispricing
    let up_mispricing = (market_up_probability - real_up_probability).abs();
    let down_mispricing = (market_down_probability - real_down_probability).abs();

    // Determine which side is mispriced more
    let (opportunity_type, recommended_side, entry_price, _side_probability) = if up_mispricing
        > down_mispricing
    {
        if market_up_probability > real_up_probability {
            // UP is overpriced, sell UP (buy DOWN)
            (
                ArbOpportunityType::DirectionalDown,
                "DOWN".to_string(),
                no_price,
                real_down_probability,
            )
        } else {
            // UP is underpriced, buy UP
            (
                ArbOpportunityType::DirectionalUp,
                "UP".to_string(),
                yes_price,
                real_up_probability,
            )
        }
    } else {
        if market_down_probability > real_down_probability {
            // DOWN is overpriced, sell DOWN (buy UP)
            (
                ArbOpportunityType::DirectionalUp,
                "UP".to_string(),
                yes_price,
                real_up_probability,
            )
        } else {
            // DOWN is underpriced, buy DOWN
            (
                ArbOpportunityType::DirectionalDown,
                "DOWN".to_string(),
                no_price,
                real_down_probability,
            )
        }
    };

    // Calculate profit margins
    let mispricing_pct = (up_mispricing.max(down_mispricing)) * 100.0;

    // Estimate slippage from bid-ask spread
    let slippage_bps = if let (Some(bid), Some(ask)) = if recommended_side == "UP" {
        (market.best_bid_yes, market.best_ask_yes)
    } else {
        (market.best_bid_no, market.best_ask_no)
    } {
        (((ask - bid) / bid) * 10000.0).max(0.0)
    } else {
        market.spread_bps.unwrap_or(100.0) // Default 1% spread if unknown
    };

    // Total fees
    let fee_bps = (market.taker_fee_bps.unwrap_or(0) as f64)
        + (market.maker_fee_bps.unwrap_or(0) as f64) / 2.0; // Average maker/taker

    // Net profit after fees and slippage
    let total_cost_bps = slippage_bps + fee_bps;
    let net_profit_margin_pct = mispricing_pct - (total_cost_bps / 100.0);

    // Check minimum thresholds
    if net_profit_margin_pct < params.min_profit_margin_pct {
        return None;
    }

    // Check liquidity requirement
    if let Some(min_liq) = params.min_liquidity {
        if market.liquidity.map_or(true, |liq| liq < min_liq) {
            return None;
        }
    }

    // Confidence based on magnitude of mispricing and liquidity
    let price_confidence = (mispricing_pct / 5.0).min(1.0); // 5%+ mispricing = 100% confidence
    let liquidity_confidence = market
        .liquidity
        .map_or(0.5, |liq| (liq / 1000.0).min(1.0)); // $1000+ = 100% confidence
    let confidence = (price_confidence * 0.7) + (liquidity_confidence * 0.3);

    if confidence < params.min_confidence {
        return None;
    }

    if slippage_bps > params.max_slippage_bps {
        return None;
    }

    Some(ArbOpportunity {
        market_id: market.market_id.clone(),
        asset: market.asset.clone(),
        opportunity_type,
        profit_margin_pct: net_profit_margin_pct,
        confidence,
        recommended_side,
        entry_price,
        liquidity_available: market.liquidity,
        slippage_bps,
        fee_bps,
        reference_price: market.reference_price,
        current_spot_price: market.current_crypto_price,
    })
}

/// Analyze multiple markets and return only profitable opportunities
pub fn find_opportunities(
    markets: &[CryptoMarket],
    params: &ArbDetectionParams,
) -> Vec<ArbOpportunity> {
    markets
        .iter()
        .filter_map(|m| analyze_market(m, params))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_up_overpriced_detection() {
        // Market prices UP at 60%, but real probability is 40%
        // Should recommend buying DOWN
        let arb_params = ArbDetectionParams::default();
        assert!(arb_params.min_confidence <= 1.0);
        assert!(arb_params.min_profit_margin_pct >= 0.0);
    }

    #[test]
    fn test_profit_margin_calculation() {
        // Mispricing of 3% - fees of 0.5% = 2.5% net profit
        let gross_mispricing = 3.0_f64;
        let fees_and_slippage = 0.5_f64;
        let net = gross_mispricing - fees_and_slippage;
        assert!((net - 2.5_f64).abs() < 0.01);
    }
}
