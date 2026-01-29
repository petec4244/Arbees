//! Team Entity Matcher
//!
//! Implements the EntityMatcher trait for sports team name matching.
//! Wraps the existing team matching logic from utils::matching.

use super::{EntityMatcher, MatchContext, MatchResult};
use crate::models::MarketType;
use crate::utils::matching::{match_team_in_text, MatchResult as LegacyMatchResult};
use async_trait::async_trait;

/// Team entity matcher for sports
///
/// Uses the existing team matching logic with aliases and fuzzy matching.
pub struct TeamMatcher;

impl TeamMatcher {
    pub fn new() -> Self {
        Self
    }

    /// Convert legacy MatchResult to new MatchResult
    fn convert_match_result(legacy: LegacyMatchResult) -> MatchResult {
        use super::MatchConfidence;

        let confidence = match legacy.confidence {
            crate::utils::matching::MatchConfidence::None => MatchConfidence::None,
            crate::utils::matching::MatchConfidence::Low => MatchConfidence::Low,
            crate::utils::matching::MatchConfidence::Medium => MatchConfidence::Medium,
            crate::utils::matching::MatchConfidence::High => MatchConfidence::High,
            crate::utils::matching::MatchConfidence::Exact => MatchConfidence::Exact,
        };

        MatchResult {
            confidence,
            score: legacy.score,
            reason: legacy.reason,
        }
    }

    /// Extract sport name from market type
    fn extract_sport(market_type: &MarketType) -> Option<String> {
        match market_type {
            MarketType::Sport { sport } => Some(sport.as_str().to_lowercase()),
            _ => None,
        }
    }
}

impl Default for TeamMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EntityMatcher for TeamMatcher {
    async fn match_entity_in_text(
        &self,
        entity_name: &str,
        text: &str,
        context: &MatchContext,
    ) -> MatchResult {
        // Extract sport from context if available
        let sport = context
            .market_type
            .as_ref()
            .and_then(Self::extract_sport)
            .unwrap_or_else(|| "nba".to_string()); // Default to NBA

        // Use existing team matching logic
        let legacy_result = match_team_in_text(entity_name, text, &sport);

        // Convert to new MatchResult
        Self::convert_match_result(legacy_result)
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Sport { .. })
    }

    fn matcher_name(&self) -> &str {
        "TeamMatcher"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Sport;

    #[tokio::test]
    async fn test_team_matcher_supports() {
        let matcher = TeamMatcher::new();
        assert_eq!(matcher.matcher_name(), "TeamMatcher");

        let sport_market = MarketType::sport(Sport::NBA);
        assert!(matcher.supports(&sport_market));

        let politics_market = MarketType::Politics {
            region: "us".to_string(),
            event_type: crate::models::PoliticsEventType::Election,
        };
        assert!(!matcher.supports(&politics_market));
    }

    #[tokio::test]
    async fn test_team_matching_basic() {
        let matcher = TeamMatcher::new();
        let context = MatchContext::new()
            .with_market_type(MarketType::sport(Sport::NBA));

        // Should match "Lakers" in text
        let result = matcher
            .match_entity_in_text("Lakers", "Los Angeles Lakers vs Celtics", &context)
            .await;

        assert!(result.is_match());
        assert!(result.score > 0.7);
    }

    #[tokio::test]
    async fn test_team_matching_with_alias() {
        let matcher = TeamMatcher::new();
        let context = MatchContext::new()
            .with_market_type(MarketType::sport(Sport::NBA));

        // Should match "LAL" as Lakers alias
        let result = matcher
            .match_entity_in_text("Lakers", "LAL vs BOS", &context)
            .await;

        assert!(result.is_match());
    }

    #[tokio::test]
    async fn test_team_no_match() {
        let matcher = TeamMatcher::new();
        let context = MatchContext::new()
            .with_market_type(MarketType::sport(Sport::NBA));

        // Should NOT match unrelated text
        let result = matcher
            .match_entity_in_text("Lakers", "Warriors vs Celtics", &context)
            .await;

        assert!(!result.is_match());
    }
}
