//! Entity Matching Abstractions
//!
//! Defines the EntityMatcher trait that allows pluggable entity matching
//! for different market types (teams, candidates, indicators, assets).

use crate::models::MarketType;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Concrete matcher implementations
pub mod team;

/// Match confidence level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MatchConfidence {
    None = 0,
    Low = 1,    // Fuzzy match only - risky
    Medium = 2, // Partial alias or word match
    High = 3,   // Strong alias match or multiple words
    Exact = 4,  // Normalized exact match
}

/// Result of matching an entity name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    pub confidence: MatchConfidence,
    pub score: f64,
    pub reason: String,
}

impl MatchResult {
    pub fn none() -> Self {
        Self {
            confidence: MatchConfidence::None,
            score: 0.0,
            reason: "No match".to_string(),
        }
    }

    pub fn exact(reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::Exact,
            score: 1.0,
            reason: reason.to_string(),
        }
    }

    pub fn high(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::High,
            score,
            reason: reason.to_string(),
        }
    }

    pub fn medium(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::Medium,
            score,
            reason: reason.to_string(),
        }
    }

    pub fn low(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::Low,
            score,
            reason: reason.to_string(),
        }
    }

    pub fn is_match(&self) -> bool {
        self.confidence >= MatchConfidence::Medium
    }
}

/// Match context provides additional information for better matching
#[derive(Debug, Clone, Default)]
pub struct MatchContext {
    pub market_type: Option<MarketType>,
    pub event_id: Option<String>,
    pub opponent: Option<String>,
    pub additional_context: HashMap<String, String>,
}

impl MatchContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_market_type(mut self, market_type: MarketType) -> Self {
        self.market_type = Some(market_type);
        self
    }

    pub fn with_opponent(mut self, opponent: String) -> Self {
        self.opponent = Some(opponent);
        self
    }
}

/// Universal entity matcher trait
///
/// Implementations provide entity matching for different market types:
/// - Sports: Team name matching with aliases
/// - Politics: Candidate name matching
/// - Economics: Indicator name matching
/// - Crypto: Asset/token name matching
#[async_trait]
pub trait EntityMatcher: Send + Sync {
    /// Match entity name in text
    ///
    /// # Arguments
    /// * `entity_name` - Canonical entity name to search for
    /// * `text` - Text to search in
    /// * `context` - Additional context for better matching
    ///
    /// # Returns
    /// Match result with confidence and details
    async fn match_entity_in_text(
        &self,
        entity_name: &str,
        text: &str,
        context: &MatchContext,
    ) -> MatchResult;

    /// Check if this matcher supports the given market type
    fn supports(&self, market_type: &MarketType) -> bool;

    /// Matcher name for logging and debugging
    fn matcher_name(&self) -> &str;
}

/// Entity matcher registry
///
/// Manages multiple entity matchers and selects the appropriate one
/// based on market type.
pub struct EntityMatcherRegistry {
    matchers: Vec<Box<dyn EntityMatcher>>,
}

impl EntityMatcherRegistry {
    /// Create a new registry with default matchers
    pub fn new() -> Self {
        let mut matchers: Vec<Box<dyn EntityMatcher>> = Vec::new();

        // Add team matcher (existing functionality)
        matchers.push(Box::new(team::TeamMatcher::new()));

        Self { matchers }
    }

    /// Match entity using the appropriate matcher
    pub async fn match_entity(
        &self,
        entity_name: &str,
        text: &str,
        context: &MatchContext,
    ) -> Result<MatchResult> {
        // If no market type in context, try all matchers
        if context.market_type.is_none() {
            for matcher in &self.matchers {
                let result = matcher
                    .match_entity_in_text(entity_name, text, context)
                    .await;
                if result.is_match() {
                    return Ok(result);
                }
            }
            return Ok(MatchResult::none());
        }

        // Find first matcher that supports this market type
        let market_type = context.market_type.as_ref().unwrap();
        for matcher in &self.matchers {
            if matcher.supports(market_type) {
                return Ok(matcher
                    .match_entity_in_text(entity_name, text, context)
                    .await);
            }
        }

        Err(anyhow::anyhow!(
            "No entity matcher found for market type: {}",
            market_type.type_name()
        ))
    }

    /// Add a custom matcher to the registry
    pub fn register_matcher(&mut self, matcher: Box<dyn EntityMatcher>) {
        self.matchers.push(matcher);
    }
}

impl Default for EntityMatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Sport;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = EntityMatcherRegistry::new();
        assert_eq!(registry.matchers.len(), 1); // Team matcher
    }

    #[tokio::test]
    async fn test_match_result_is_match() {
        assert!(!MatchResult::none().is_match());
        assert!(!MatchResult::low(0.6, "fuzzy").is_match());
        assert!(MatchResult::medium(0.75, "partial").is_match());
        assert!(MatchResult::high(0.9, "strong").is_match());
        assert!(MatchResult::exact("exact").is_match());
    }

    #[tokio::test]
    async fn test_match_context_builder() {
        let context = MatchContext::new()
            .with_market_type(MarketType::sport(Sport::NBA))
            .with_opponent("Lakers".to_string());

        assert!(context.market_type.is_some());
        assert_eq!(context.opponent, Some("Lakers".to_string()));
    }
}
