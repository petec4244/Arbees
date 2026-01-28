//! Politics Probability Model
//!
//! Calculates probability for political events based on:
//! - Market-implied probabilities
//! - Time until event
//! - Polling data (when available)
//! - Historical accuracy adjustments

use super::ProbabilityModel;
use crate::models::{GameState, MarketType, PoliticsEventType};
use crate::providers::{EventState, StateData};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use tracing::debug;

/// Politics probability model using market and polling data
pub struct PoliticsProbabilityModel {
    /// Default probability when no data available
    default_probability: f64,
    /// Mean reversion factor (how much to regress to 50%)
    mean_reversion_factor: f64,
}

impl PoliticsProbabilityModel {
    /// Create a new politics probability model
    pub fn new() -> Self {
        Self {
            default_probability: 0.5,
            mean_reversion_factor: 0.1, // 10% regression to mean
        }
    }

    /// Calculate probability from market and polling data
    ///
    /// Uses a weighted average of market probability and polling data,
    /// with time-based mean reversion.
    pub fn calculate_event_probability(
        market_prob: Option<f64>,
        poll_prob: Option<f64>,
        poll_count: Option<u32>,
        days_until_event: f64,
    ) -> f64 {
        // Start with market probability or default
        let base_prob = market_prob.unwrap_or(0.5);

        // If we have polling data, blend it with market
        let blended = match (poll_prob, poll_count) {
            (Some(poll), Some(count)) if count > 5 => {
                // More polls = more weight on polling
                let poll_weight = (count as f64 / 20.0).min(0.4);
                base_prob * (1.0 - poll_weight) + poll * poll_weight
            }
            (Some(poll), _) => {
                // Few polls, small weight
                base_prob * 0.9 + poll * 0.1
            }
            _ => base_prob,
        };

        // Apply time-based mean reversion
        // Further from event = more uncertainty = regress toward 50%
        let reversion = Self::calculate_mean_reversion(days_until_event);
        let final_prob = blended * (1.0 - reversion) + 0.5 * reversion;

        final_prob.clamp(0.01, 0.99)
    }

    /// Calculate mean reversion factor based on days until event
    ///
    /// More time = more uncertainty = stronger regression to 50%
    fn calculate_mean_reversion(days_until_event: f64) -> f64 {
        if days_until_event <= 0.0 {
            return 0.0; // No reversion at event time
        }

        // Use a logarithmic scale
        // 7 days: ~5% reversion
        // 30 days: ~10% reversion
        // 180 days: ~15% reversion
        // 365 days: ~20% reversion
        let log_days = (days_until_event.max(1.0)).ln();
        let reversion = log_days * 0.03;

        reversion.min(0.25) // Cap at 25% reversion
    }

    /// Adjust probability based on event type
    ///
    /// Different event types have different volatility characteristics
    fn adjust_for_event_type(prob: f64, event_type: &PoliticsEventType) -> f64 {
        match event_type {
            PoliticsEventType::Election => {
                // Elections are relatively stable
                prob
            }
            PoliticsEventType::Confirmation => {
                // Confirmations can be volatile
                // Slightly increase uncertainty
                prob * 0.95 + 0.025
            }
            PoliticsEventType::PolicyVote => {
                // Policy votes can be very uncertain
                prob * 0.9 + 0.05
            }
            PoliticsEventType::Impeachment => {
                // Impeachment is highly uncertain
                prob * 0.85 + 0.075
            }
            PoliticsEventType::Other => {
                // Generic events have moderate uncertainty
                prob * 0.9 + 0.05
            }
        }
    }

    /// Calculate edge between model probability and market probability
    pub fn calculate_edge(model_prob: f64, market_prob: f64) -> f64 {
        if market_prob <= 0.0 || market_prob >= 1.0 {
            return 0.0;
        }
        (model_prob - market_prob) / market_prob * 100.0
    }
}

impl Default for PoliticsProbabilityModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProbabilityModel for PoliticsProbabilityModel {
    async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64> {
        // Extract politics state
        let StateData::Politics(pol_state) = &event_state.state else {
            return Err(anyhow!("Expected politics state data"));
        };

        // Get event type from market type
        let event_type = match &event_state.market_type {
            MarketType::Politics { event_type, .. } => event_type.clone(),
            _ => return Err(anyhow!("Expected politics market type")),
        };

        // Calculate days until event
        let now = Utc::now();
        let days_until_event = (pol_state.event_date - now).num_seconds() as f64 / 86400.0;

        // Calculate base probability
        let prob = Self::calculate_event_probability(
            pol_state.current_probability,
            None, // Would come from polling aggregator
            pol_state.poll_count,
            days_until_event.max(0.0),
        );

        // Adjust for event type
        let adjusted = Self::adjust_for_event_type(prob, &event_type);

        debug!(
            "Politics probability for {}: base={:.3}, adjusted={:.3}, days_remaining={:.0}",
            event_state.entity_a, prob, adjusted, days_until_event
        );

        // for_entity_a = entity wins/occurs
        // for_entity_b = entity loses/doesn't occur
        if for_entity_a {
            Ok(adjusted.clamp(0.001, 0.999))
        } else {
            Ok((1.0 - adjusted).clamp(0.001, 0.999))
        }
    }

    async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64> {
        // Extract politics state from market_specific
        let pol_state = match &game_state.market_specific {
            Some(crate::models::MarketSpecificState::Politics(state)) => state.clone(),
            _ => return Err(anyhow!("No politics state in legacy GameState")),
        };

        // Get event type from market type
        let event_type = match &game_state.market_type {
            Some(MarketType::Politics { event_type, .. }) => event_type.clone(),
            _ => PoliticsEventType::Other,
        };

        // Calculate days until event
        let now = Utc::now();
        let days_until_event = (pol_state.event_date - now).num_seconds() as f64 / 86400.0;

        // Calculate probability
        let prob = Self::calculate_event_probability(
            pol_state.current_probability,
            None,
            pol_state.poll_count,
            days_until_event.max(0.0),
        );

        let adjusted = Self::adjust_for_event_type(prob, &event_type);

        if for_home_team {
            Ok(adjusted.clamp(0.001, 0.999))
        } else {
            Ok((1.0 - adjusted).clamp(0.001, 0.999))
        }
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Politics { .. })
    }

    fn model_name(&self) -> &str {
        "PoliticsProbability"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mean_reversion() {
        // No reversion at event
        let rev = PoliticsProbabilityModel::calculate_mean_reversion(0.0);
        assert!(rev < 0.01);

        // Small reversion close to event
        let rev = PoliticsProbabilityModel::calculate_mean_reversion(7.0);
        assert!(rev > 0.0 && rev < 0.1);

        // Moderate reversion further out
        let rev = PoliticsProbabilityModel::calculate_mean_reversion(30.0);
        assert!(rev > 0.05 && rev < 0.15);

        // Higher reversion far out
        let rev = PoliticsProbabilityModel::calculate_mean_reversion(365.0);
        assert!(rev > 0.1 && rev < 0.25);
    }

    #[test]
    fn test_event_probability_market_only() {
        // Market probability only, close to event
        let prob = PoliticsProbabilityModel::calculate_event_probability(
            Some(0.7),
            None,
            None,
            7.0, // 7 days
        );
        // Should be close to market but slightly regressed
        assert!(prob > 0.65 && prob < 0.75);
    }

    #[test]
    fn test_event_probability_with_polls() {
        // Market and polls, strong poll consensus
        let prob = PoliticsProbabilityModel::calculate_event_probability(
            Some(0.55),
            Some(0.65),
            Some(20),
            30.0,
        );
        // Should blend toward polls
        assert!(prob > 0.55 && prob < 0.65);
    }

    #[test]
    fn test_event_probability_far_out() {
        // Extreme probability far from event
        let prob = PoliticsProbabilityModel::calculate_event_probability(
            Some(0.85),
            None,
            None,
            365.0, // 1 year
        );
        // Should regress significantly toward 50%
        assert!(prob < 0.80);
    }

    #[test]
    fn test_event_type_adjustment() {
        let base = 0.7;

        // Elections stay stable
        let election = PoliticsProbabilityModel::adjust_for_event_type(
            base,
            &PoliticsEventType::Election,
        );
        assert!((election - base).abs() < 0.01);

        // Impeachment adds uncertainty
        let impeach = PoliticsProbabilityModel::adjust_for_event_type(
            base,
            &PoliticsEventType::Impeachment,
        );
        assert!(impeach < base && impeach > 0.6);
    }

    #[test]
    fn test_edge_calculation() {
        // Model thinks more likely than market
        let edge = PoliticsProbabilityModel::calculate_edge(0.7, 0.6);
        assert!((edge - 16.67).abs() < 0.1);

        // Model thinks less likely
        let edge = PoliticsProbabilityModel::calculate_edge(0.5, 0.6);
        assert!((edge - (-16.67)).abs() < 0.1);

        // Exact match
        let edge = PoliticsProbabilityModel::calculate_edge(0.5, 0.5);
        assert!(edge.abs() < 0.01);
    }

    #[tokio::test]
    async fn test_politics_model_supports() {
        let model = PoliticsProbabilityModel::new();
        assert_eq!(model.model_name(), "PoliticsProbability");

        let pol_market = MarketType::Politics {
            region: "US".to_string(),
            event_type: PoliticsEventType::Election,
        };
        assert!(model.supports(&pol_market));

        let sport_market = MarketType::sport(crate::models::Sport::NBA);
        assert!(!model.supports(&sport_market));
    }
}
