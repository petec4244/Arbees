//! Economics Probability Model
//!
//! Calculates probability of economic indicators hitting targets based on:
//! - Current value vs threshold
//! - Historical volatility of the indicator
//! - Time until release/resolution
//! - Recent trends (YoY, MoM changes)

use super::ProbabilityModel;
use crate::clients::fred::{series, FredClient};
use crate::models::{GameState, MarketType};
use crate::providers::{EventState, StateData};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Economics probability model using statistical forecasting
pub struct EconomicsProbabilityModel {
    /// FRED client for fetching indicator data
    fred: Arc<FredClient>,
    /// Cache for volatility values by indicator
    volatility_cache: Arc<RwLock<HashMap<String, (f64, DateTime<Utc>)>>>,
    /// Default volatilities by indicator type (annualized std dev)
    default_volatilities: HashMap<String, f64>,
}

impl EconomicsProbabilityModel {
    /// Create a new economics probability model
    pub fn new() -> Self {
        Self::with_fred(Arc::new(FredClient::new()))
    }

    /// Create with custom FRED client (for testing/sharing)
    pub fn with_fred(fred: Arc<FredClient>) -> Self {
        let mut default_volatilities = HashMap::new();

        // Historical annualized volatilities (approximate)
        default_volatilities.insert(series::CPI.to_string(), 0.15); // 15% annual vol
        default_volatilities.insert(series::CORE_CPI.to_string(), 0.10);
        default_volatilities.insert(series::PCE.to_string(), 0.12);
        default_volatilities.insert(series::CORE_PCE.to_string(), 0.08);
        default_volatilities.insert(series::UNEMPLOYMENT.to_string(), 0.25);
        default_volatilities.insert(series::NONFARM_PAYROLLS.to_string(), 0.30);
        default_volatilities.insert(series::FED_FUNDS_RATE.to_string(), 0.50); // Fed moves 25-50bps
        default_volatilities.insert(series::GDP_GROWTH.to_string(), 0.40);
        default_volatilities.insert(series::JOBLESS_CLAIMS.to_string(), 0.35);
        default_volatilities.insert(series::CONSUMER_SENTIMENT.to_string(), 0.20);
        default_volatilities.insert(series::TREASURY_10Y.to_string(), 0.25);
        default_volatilities.insert(series::TREASURY_2Y.to_string(), 0.30);

        Self {
            fred,
            volatility_cache: Arc::new(RwLock::new(HashMap::new())),
            default_volatilities,
        }
    }

    /// Get volatility for an indicator series
    async fn get_volatility(&self, series_id: &str) -> f64 {
        // Check cache (1 day TTL for economic data)
        {
            let cache = self.volatility_cache.read().await;
            if let Some((vol, fetched_at)) = cache.get(series_id) {
                let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
                if age < 86400 {
                    return *vol;
                }
            }
        }

        // Try to calculate from historical data
        match self.calculate_historical_volatility(series_id).await {
            Ok(vol) => {
                let mut cache = self.volatility_cache.write().await;
                cache.insert(series_id.to_string(), (vol, Utc::now()));
                debug!("Calculated volatility for {}: {:.4}", series_id, vol);
                vol
            }
            Err(e) => {
                warn!(
                    "Failed to calculate volatility for {}: {}. Using default.",
                    series_id, e
                );
                self.default_volatilities
                    .get(series_id)
                    .copied()
                    .unwrap_or(0.20)
            }
        }
    }

    /// Calculate historical volatility from FRED data
    async fn calculate_historical_volatility(&self, series_id: &str) -> Result<f64> {
        let observations = self.fred.get_observations(series_id, Some(60)).await?;

        if observations.len() < 12 {
            return Err(anyhow!("Insufficient data for volatility calculation"));
        }

        // Calculate month-over-month changes
        let mut changes: Vec<f64> = Vec::new();
        for i in 1..observations.len() {
            if let (Some(prev), Some(curr)) = (observations[i].value, observations[i - 1].value) {
                if prev != 0.0 {
                    changes.push((curr - prev) / prev);
                }
            }
        }

        if changes.is_empty() {
            return Err(anyhow!("No valid changes for volatility"));
        }

        // Calculate standard deviation
        let mean: f64 = changes.iter().sum::<f64>() / changes.len() as f64;
        let variance: f64 =
            changes.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / changes.len() as f64;
        let monthly_vol = variance.sqrt();

        // Annualize (assuming monthly data)
        Ok(monthly_vol * (12.0_f64).sqrt())
    }

    /// Calculate probability of indicator being above threshold
    ///
    /// Uses a normal distribution model where:
    /// - Current value is the mean
    /// - Historical volatility scales with time to release
    /// - P(X > threshold) = 1 - Φ((threshold - current) / (σ * √T))
    pub fn calculate_threshold_probability(
        current_value: f64,
        threshold: f64,
        months_remaining: f64,
        annualized_volatility: f64,
        yoy_trend: Option<f64>,
    ) -> f64 {
        if months_remaining <= 0.0 {
            return if current_value >= threshold { 1.0 } else { 0.0 };
        }

        // Time-scaled volatility
        let t = months_remaining / 12.0;
        let sigma = annualized_volatility * current_value.abs() * t.sqrt();

        if sigma <= 0.0 {
            return if current_value >= threshold { 0.9 } else { 0.1 };
        }

        // Adjust expected value based on trend
        let drift = match yoy_trend {
            Some(trend) => trend / 100.0 * t, // Convert percentage to decimal
            None => 0.0,
        };
        let expected_value = current_value * (1.0 + drift);

        // Calculate z-score
        let z = (threshold - expected_value) / sigma;

        // Probability of being ABOVE threshold = 1 - Φ(z)
        1.0 - normal_cdf(z)
    }

    /// Calculate probability with special handling for Fed rate decisions
    pub fn calculate_fed_rate_probability(
        current_rate: f64,
        target_rate: f64,
        _meetings_until: u32,
    ) -> f64 {
        // Fed typically moves in 25bp increments
        let rate_diff = target_rate - current_rate;
        let moves_needed = (rate_diff / 0.25).abs();

        // Simple model: probability decreases with more moves needed
        if rate_diff == 0.0 {
            0.5 // Already at target
        } else if moves_needed <= 1.0 {
            0.7 // One move needed, fairly likely
        } else if moves_needed <= 2.0 {
            0.5 // Two moves, uncertain
        } else if moves_needed <= 4.0 {
            0.3 // Multiple moves, less likely
        } else {
            0.15 // Many moves, unlikely in near term
        }
    }
}

impl Default for EconomicsProbabilityModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProbabilityModel for EconomicsProbabilityModel {
    async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64> {
        // Extract economics state
        let StateData::Economics(econ_state) = &event_state.state else {
            return Err(anyhow!("Expected economics state data"));
        };

        // Get indicator from market type
        let (indicator, threshold) = match &event_state.market_type {
            MarketType::Economics {
                indicator,
                threshold,
            } => (indicator.clone(), threshold.or(econ_state.forecast_value)),
            _ => return Err(anyhow!("Expected economics market type")),
        };

        let threshold = threshold.ok_or_else(|| anyhow!("No threshold specified"))?;
        let current_value = econ_state
            .current_value
            .ok_or_else(|| anyhow!("No current value"))?;

        // Get series ID and volatility
        let series_id = crate::providers::economics::EconomicsEventProvider::indicator_to_series(&indicator);
        let volatility = self.get_volatility(series_id).await;

        // Calculate months remaining
        let now = Utc::now();
        let months_remaining = (econ_state.release_date - now).num_days() as f64 / 30.0;

        // Extract trend from metadata
        let yoy_trend = econ_state
            .metadata
            .get("yoy_change")
            .and_then(|v| v.as_f64());

        // Calculate base probability
        let prob = Self::calculate_threshold_probability(
            current_value,
            threshold,
            months_remaining.max(0.001),
            volatility,
            yoy_trend,
        );

        // for_entity_a means "above threshold"
        // for_entity_b means "below threshold"
        if for_entity_a {
            Ok(prob.clamp(0.001, 0.999))
        } else {
            Ok((1.0 - prob).clamp(0.001, 0.999))
        }
    }

    async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64> {
        // Extract economics state from market_specific if available
        let econ_state = match &game_state.market_specific {
            Some(crate::models::MarketSpecificState::Economics(state)) => state.clone(),
            _ => {
                return Err(anyhow!("No economics state in legacy GameState"));
            }
        };

        // Get indicator and threshold from market type
        let (indicator, threshold) = match &game_state.market_type {
            Some(MarketType::Economics {
                indicator,
                threshold,
            }) => (indicator.clone(), threshold.or(econ_state.forecast_value)),
            _ => return Err(anyhow!("Not an economics market type")),
        };

        let threshold = threshold.ok_or_else(|| anyhow!("No threshold specified"))?;
        let current_value = econ_state
            .current_value
            .ok_or_else(|| anyhow!("No current value"))?;

        let series_id = crate::providers::economics::EconomicsEventProvider::indicator_to_series(&indicator);
        let volatility = self.get_volatility(series_id).await;

        let now = Utc::now();
        let months_remaining = (econ_state.release_date - now).num_days() as f64 / 30.0;

        let prob = Self::calculate_threshold_probability(
            current_value,
            threshold,
            months_remaining.max(0.001),
            volatility,
            None,
        );

        if for_home_team {
            Ok(prob.clamp(0.001, 0.999))
        } else {
            Ok((1.0 - prob).clamp(0.001, 0.999))
        }
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Economics { .. })
    }

    fn model_name(&self) -> &str {
        "EconomicsProbability"
    }
}

/// Standard normal CDF approximation (Abramowitz and Stegun)
fn normal_cdf(x: f64) -> f64 {
    if x < -8.0 {
        return 0.0;
    }
    if x > 8.0 {
        return 1.0;
    }

    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs() / (2.0_f64).sqrt();

    let t = 1.0 / (1.0 + p * x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();

    0.5 * (1.0 + sign * y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EconomicIndicator;

    #[test]
    fn test_normal_cdf() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 0.001);
        assert!((normal_cdf(-1.96) - 0.025).abs() < 0.01);
        assert!((normal_cdf(1.96) - 0.975).abs() < 0.01);
        assert!(normal_cdf(-8.0) < 0.001);
        assert!(normal_cdf(8.0) > 0.999);
    }

    #[test]
    fn test_threshold_probability_at_threshold() {
        // Current value equals threshold
        let prob = EconomicsProbabilityModel::calculate_threshold_probability(
            3.0, // current
            3.0, // threshold
            6.0, // 6 months
            0.15, // 15% annual vol
            None,
        );
        // Should be close to 50%
        assert!(prob > 0.45 && prob < 0.55, "At-threshold prob: {}", prob);
    }

    #[test]
    fn test_threshold_probability_above() {
        // Current value above threshold
        let prob = EconomicsProbabilityModel::calculate_threshold_probability(
            4.0, // current
            3.0, // threshold
            6.0, // 6 months
            0.15, // 15% annual vol
            None,
        );
        // Should be high
        assert!(prob > 0.70, "Above-threshold prob: {}", prob);
    }

    #[test]
    fn test_threshold_probability_below() {
        // Current value below threshold
        let prob = EconomicsProbabilityModel::calculate_threshold_probability(
            2.5, // current
            3.5, // threshold
            6.0, // 6 months
            0.15, // 15% annual vol
            None,
        );
        // Should be lower (below threshold with no drift)
        assert!(prob < 0.50, "Below-threshold prob: {}", prob);
    }

    #[test]
    fn test_threshold_probability_with_trend() {
        // Below threshold but with positive trend
        let prob_no_trend = EconomicsProbabilityModel::calculate_threshold_probability(
            2.8, // current
            3.0, // threshold
            6.0, // 6 months
            0.15,
            None,
        );

        let prob_with_trend = EconomicsProbabilityModel::calculate_threshold_probability(
            2.8, // current
            3.0, // threshold
            6.0, // 6 months
            0.15,
            Some(10.0), // 10% YoY growth
        );

        // Positive trend should increase probability of hitting higher threshold
        assert!(
            prob_with_trend > prob_no_trend,
            "With trend {} vs no trend {}",
            prob_with_trend,
            prob_no_trend
        );
    }

    #[test]
    fn test_fed_rate_probability() {
        // At target rate
        let prob = EconomicsProbabilityModel::calculate_fed_rate_probability(5.25, 5.25, 2);
        assert!((prob - 0.5).abs() < 0.1);

        // One 25bp cut
        let prob = EconomicsProbabilityModel::calculate_fed_rate_probability(5.25, 5.0, 2);
        assert!(prob > 0.5);

        // Multiple cuts needed
        let prob = EconomicsProbabilityModel::calculate_fed_rate_probability(5.25, 4.0, 2);
        assert!(prob < 0.4);
    }

    #[tokio::test]
    async fn test_economics_model_supports() {
        let model = EconomicsProbabilityModel::new();
        assert_eq!(model.model_name(), "EconomicsProbability");

        let econ_market = MarketType::Economics {
            indicator: EconomicIndicator::CPI,
            threshold: Some(3.0),
        };
        assert!(model.supports(&econ_market));

        let sport_market = MarketType::sport(crate::models::Sport::NBA);
        assert!(!model.supports(&sport_market));
    }
}
