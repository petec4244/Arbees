//! Model-based probability signals for crypto
//!
//! Uses probability models to detect market mispricing.
//! Compares model probability against market prices to find edges.

use crate::price::data::CryptoPriceData;
use crate::signals::risk::CryptoRiskChecker;
use crate::types::{CryptoEventContext, CryptoExecutionRequest, CryptoSignalType, Direction};
use anyhow::Result;
use chrono::Utc;
use log::{debug, info};
use std::collections::HashMap;
use uuid::Uuid;

/// Detects probability-based mispricings
pub struct CryptoProbabilityDetector {
    /// Minimum edge percentage to trade (after fees)
    pub min_edge_pct: f64,

    /// Minimum model confidence to trade
    pub model_min_confidence: f64,
}

impl CryptoProbabilityDetector {
    pub fn new(min_edge_pct: f64, model_min_confidence: f64) -> Self {
        Self {
            min_edge_pct,
            model_min_confidence,
        }
    }

    /// Calculate model probability for a crypto price target
    /// Uses log-normal distribution based on Black-Scholes
    fn calculate_model_probability(
        &self,
        current_price: f64,
        target_price: f64,
        days_remaining: f64,
        volatility: f64,
    ) -> f64 {
        if days_remaining <= 0.0 {
            // Market has expired
            if current_price >= target_price {
                1.0
            } else {
                0.0
            }
        } else {
            // Black-Scholes-inspired log-normal distribution
            let log_return = (target_price / current_price).ln();
            let std_dev = volatility * days_remaining.sqrt();

            if std_dev < 0.001 {
                // No volatility, deterministic
                if log_return > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                // Cumulative normal distribution approximation
                let z = log_return / std_dev;
                normal_cdf(z)
            }
        }
    }

    /// Detect probability-based signals
    pub async fn detect_and_emit(
        &self,
        event: &CryptoEventContext,
        prices: &HashMap<String, CryptoPriceData>,
        spot_price: f64,
        risk_checker: &CryptoRiskChecker,
    ) -> Result<Option<CryptoExecutionRequest>> {
        // Get target price
        let target_price = event.target_price.ok_or_else(|| {
            anyhow::anyhow!("Event {} has no target price", event.event_id)
        })?;

        // Calculate days until expiration
        let now = Utc::now();
        let time_diff = event.target_date.signed_duration_since(now);
        let days_remaining = time_diff.num_seconds() as f64 / 86400.0;

        if days_remaining < 0.0 {
            return Ok(None); // Event expired
        }

        // Simple volatility estimate (would use historical data in production)
        let volatility = 0.5; // 50% annual volatility for crypto

        // Calculate model probability
        let probability = self.calculate_model_probability(
            spot_price,
            target_price,
            days_remaining,
            volatility,
        );

        if probability < self.model_min_confidence && probability > (1.0 - self.model_min_confidence)
        {
            return Ok(None); // Model not confident enough
        }

        // Find best market opportunity
        let mut best_price: Option<&CryptoPriceData> = None;
        let mut best_edge = 0.0;
        let mut best_direction = Direction::Long;

        for (key, price_data) in prices {
            if !key.starts_with(&format!("{}|", event.asset)) {
                continue;
            }

            // Check long edge (model > market ask)
            let long_edge = probability - price_data.yes_ask;
            if long_edge > best_edge && long_edge > 0.0 {
                best_edge = long_edge;
                best_price = Some(price_data);
                best_direction = Direction::Long;
            }

            // Check short edge (market bid > model)
            let short_edge = price_data.yes_bid - probability;
            if short_edge > best_edge && short_edge > 0.0 {
                best_edge = short_edge;
                best_price = Some(price_data);
                best_direction = Direction::Short;
            }
        }

        let price_data = match best_price {
            Some(p) => p,
            None => return Ok(None),
        };

        let edge_pct = (best_edge / probability).abs() * 100.0;

        if edge_pct < self.min_edge_pct {
            return Ok(None);
        }

        // Kelly criterion position sizing
        let kelly_fraction = 0.25; // Conservative
        let win_prob = match best_direction {
            Direction::Long => probability,
            Direction::Short => 1.0 - probability,
        };

        let odds = if best_direction == Direction::Long {
            (1.0 - price_data.yes_ask) / price_data.yes_ask
        } else {
            price_data.yes_bid / (1.0 - price_data.yes_bid)
        };

        let kelly_size = kelly_fraction * ((win_prob * (odds + 1.0) - 1.0) / odds);
        let suggested_size = (kelly_size * 1000.0).max(50.0).min(500.0); // $50-$500 range

        // Volatility factor (normalized volatility)
        let volatility_factor = volatility / 0.25; // Relative to 25% baseline

        let execution_price = if best_direction == Direction::Long {
            price_data.yes_ask
        } else {
            price_data.yes_bid
        };

        // Pass through risk checker
        let adjusted_size = match risk_checker
            .validate_trade(
                &event.asset,
                &price_data.platform,
                &price_data.market_id,
                edge_pct,
                suggested_size,
                price_data.total_liquidity,
                volatility_factor,
            )
            .await
        {
            Ok(size) => size,
            Err(e) => {
                debug!("Model signal blocked by risk check: {}", e);
                return Ok(None);
            }
        };

        info!(
            "Crypto model signal: {} {:?} {}% edge (model={:.3}, market={:.3}), size=${}",
            event.asset, best_direction, edge_pct, probability, execution_price, adjusted_size
        );

        let request = CryptoExecutionRequest {
            request_id: Uuid::new_v4().to_string(),
            event_id: event.event_id.clone(),
            asset: event.asset.clone(),
            signal_type: CryptoSignalType::ModelEdge,
            platform: price_data.platform.clone(),
            market_id: price_data.market_id.clone(),
            direction: best_direction,
            edge_pct,
            probability,
            suggested_size: adjusted_size,
            max_price: execution_price + 0.01,
            current_price: execution_price,
            timestamp: Utc::now(),
            volatility_factor,
            exposure_check: true,
            balance_check: true,
        };

        Ok(Some(request))
    }
}

/// Approximate cumulative normal distribution function
/// Used for probability calculations
fn normal_cdf(z: f64) -> f64 {
    // Abramowitz and Stegun approximation
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if z < 0.0 { -1.0 } else { 1.0 };
    let z = z.abs() / std::f64::consts::SQRT_2;

    let t = 1.0 / (1.0 + p * z);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;

    let y = 1.0 - (a1 * t + a2 * t2 + a3 * t3 + a4 * t4 + a5 * t5) * (-z * z).exp();

    0.5 * (1.0 + sign * y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probability_detector_creation() {
        let detector = CryptoProbabilityDetector::new(3.0, 0.6);
        assert_eq!(detector.min_edge_pct, 3.0);
        assert_eq!(detector.model_min_confidence, 0.6);
    }

    #[test]
    fn test_model_probability_expired_above_target() {
        let detector = CryptoProbabilityDetector::new(3.0, 0.6);
        // Current 100, target 90: current >= target, so probability should be 1.0
        let prob = detector.calculate_model_probability(100.0, 90.0, -1.0, 0.5);
        assert_eq!(prob, 1.0); // Event expired, current >= target
    }

    #[test]
    fn test_model_probability_expired_below_target() {
        let detector = CryptoProbabilityDetector::new(3.0, 0.6);
        // Current 100, target 110: current < target, so probability should be 0.0
        let prob = detector.calculate_model_probability(100.0, 110.0, -1.0, 0.5);
        assert_eq!(prob, 0.0); // Event expired, didn't reach target
    }

    #[test]
    fn test_normal_cdf_bounds() {
        // CDF must be between 0 and 1
        let cdf_neg10 = normal_cdf(-10.0);
        assert!(cdf_neg10 >= 0.0 && cdf_neg10 <= 0.001); // Very close to 0

        let cdf_0 = normal_cdf(0.0);
        assert!(cdf_0 > 0.49 && cdf_0 < 0.51); // ~0.5 at 0

        let cdf_10 = normal_cdf(10.0);
        assert!(cdf_10 > 0.999 && cdf_10 <= 1.0); // Very close to 1
    }

    #[test]
    fn test_normal_cdf_symmetry() {
        // CDF is symmetric: CDF(z) + CDF(-z) = 1
        let cdf_pos = normal_cdf(1.96);
        let cdf_neg = normal_cdf(-1.96);
        assert!((cdf_pos + cdf_neg - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_model_probability_calculation_above_target() {
        let detector = CryptoProbabilityDetector::new(3.0, 0.6);
        // Current price is above target, should be low probability
        let prob = detector.calculate_model_probability(150.0, 100.0, 1.0, 0.5);
        assert!(prob < 0.5);
    }

    #[test]
    fn test_model_probability_calculation_below_target() {
        let detector = CryptoProbabilityDetector::new(3.0, 0.6);
        // Current price is below target, should be high probability
        let prob = detector.calculate_model_probability(50.0, 100.0, 1.0, 0.5);
        assert!(prob > 0.5);
    }

    #[test]
    fn test_kelly_criterion_sizing() {
        // Basic Kelly criterion: f = (bp - q) / b
        // where f = fraction of bankroll, b = odds, p = win probability, q = loss probability

        let win_prob = 0.6;
        let _loss_prob = 0.4;
        let odds = 1.0; // Even odds
        let kelly_fraction = 0.25; // Conservative

        let kelly_size = kelly_fraction * ((win_prob * (odds + 1.0) - 1.0) / odds);
        assert!(kelly_size > 0.0);
        assert!(kelly_size < kelly_fraction); // Should be less than full kelly
    }

    #[test]
    fn test_position_sizing_bounds() {
        let kelly_size = 0.05; // 5% of bankroll
        let base_bankroll = 1000.0_f64;
        let suggested_size = (kelly_size * base_bankroll).max(50.0).min(500.0);

        assert!(suggested_size >= 50.0);
        assert!(suggested_size <= 500.0);
    }
}
