//! Probability Model Abstractions
//!
//! Defines the ProbabilityModel trait that allows pluggable probability
//! calculation models for different market types.

use crate::models::{GameState, MarketType};
use crate::providers::EventState;
use anyhow::Result;
use async_trait::async_trait;

// Concrete model implementations
pub mod crypto;
pub mod economics;
pub mod politics;
pub mod sport;

/// Universal probability model trait
///
/// Implementations provide probability calculations for different market types:
/// - Sports: Win probability models (existing functionality)
/// - Politics: Polling aggregator models
/// - Economics: Consensus forecast models
/// - Crypto: Technical/price target models
#[async_trait]
pub trait ProbabilityModel: Send + Sync {
    /// Calculate probability for entity_a winning/occurring
    ///
    /// # Arguments
    /// * `event_state` - Current event state
    /// * `for_entity_a` - If true, calculate probability for entity_a, else entity_b
    ///
    /// # Returns
    /// Probability between 0.0 and 1.0, or error if calculation fails
    async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64>;

    /// Calculate probability from legacy GameState (for backward compatibility)
    ///
    /// This allows existing sports code to work with the new trait system.
    async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64>;

    /// Check if this model supports the given market type
    fn supports(&self, market_type: &MarketType) -> bool;

    /// Model name for logging and debugging
    fn model_name(&self) -> &str;
}

/// Probability model registry
///
/// Manages multiple probability models and selects the appropriate one
/// based on market type.
pub struct ProbabilityModelRegistry {
    models: Vec<Box<dyn ProbabilityModel>>,
}

impl ProbabilityModelRegistry {
    /// Create a new registry with default models
    pub fn new() -> Self {
        let mut models: Vec<Box<dyn ProbabilityModel>> = Vec::new();

        // Add sport model (existing functionality)
        models.push(Box::new(sport::SportWinProbabilityModel::new()));

        // Add crypto model
        models.push(Box::new(crypto::CryptoProbabilityModel::new()));

        // Add economics model
        models.push(Box::new(economics::EconomicsProbabilityModel::new()));

        // Add politics model
        models.push(Box::new(politics::PoliticsProbabilityModel::new()));

        Self { models }
    }

    /// Calculate probability using the appropriate model
    pub async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64> {
        // Find first model that supports this market type
        for model in &self.models {
            if model.supports(&event_state.market_type) {
                return model
                    .calculate_probability(event_state, for_entity_a)
                    .await;
            }
        }

        Err(anyhow::anyhow!(
            "No probability model found for market type: {}",
            event_state.market_type.type_name()
        ))
    }

    /// Calculate probability from legacy GameState
    pub async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64> {
        // Extract or infer market type
        let market_type = game_state
            .market_type
            .clone()
            .unwrap_or_else(|| MarketType::sport(game_state.sport));

        // Find appropriate model
        for model in &self.models {
            if model.supports(&market_type) {
                return model
                    .calculate_probability_legacy(game_state, for_home_team)
                    .await;
            }
        }

        Err(anyhow::anyhow!(
            "No probability model found for sport: {:?}",
            game_state.sport
        ))
    }

    /// Add a custom model to the registry
    pub fn register_model(&mut self, model: Box<dyn ProbabilityModel>) {
        self.models.push(model);
    }
}

impl Default for ProbabilityModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = ProbabilityModelRegistry::new();
        assert_eq!(registry.models.len(), 4); // Sport + Crypto + Economics + Politics models
    }
}
