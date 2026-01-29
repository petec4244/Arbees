//! Sport Win Probability Model
//!
//! Implements the ProbabilityModel trait for sports win probability calculation.
//! Wraps the existing calculate_win_probability logic.

use super::ProbabilityModel;
use crate::models::{GameState, MarketType, Sport};
use crate::providers::{EventState, StateData};
use crate::win_prob::calculate_win_probability;
use anyhow::{anyhow, Result};
use async_trait::async_trait;

/// Sport win probability model
///
/// Uses the existing calculate_win_probability function to provide
/// probability estimates for sports markets.
pub struct SportWinProbabilityModel;

impl SportWinProbabilityModel {
    pub fn new() -> Self {
        Self
    }

    /// Convert EventState to GameState for legacy compatibility
    fn event_state_to_game_state(event_state: &EventState) -> Result<GameState> {
        // Extract sport from market type
        let sport = match &event_state.market_type {
            MarketType::Sport { sport } => *sport,
            _ => return Err(anyhow!("Not a sports event")),
        };

        // Extract sport-specific state
        let StateData::Sport(sport_state) = &event_state.state else {
            return Err(anyhow!("Missing sport state data"));
        };

        // Build GameState from EventState
        Ok(GameState {
            // Universal fields
            event_id: event_state.event_id.clone(),
            market_type: Some(event_state.market_type.clone()),
            entity_a: Some(event_state.entity_a.clone()),
            entity_b: event_state.entity_b.clone(),
            event_start: None,
            event_end: None,
            resolution_criteria: None,

            // Legacy fields
            game_id: event_state.event_id.clone(),
            sport,
            home_team: event_state.entity_a.clone(),
            away_team: event_state
                .entity_b
                .clone()
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            home_score: sport_state.score_a,
            away_score: sport_state.score_b,
            period: sport_state.period,
            time_remaining_seconds: sport_state.time_remaining,
            possession: sport_state.possession.clone(),

            // Common fields
            fetched_at: event_state.fetched_at,
            pregame_home_prob: None,

            // Sport-specific state (flatten from sport_details JSON)
            sport_specific: parse_sport_specific(sport, &sport_state.sport_details),
            market_specific: None,
        })
    }
}

/// Parse sport-specific details from JSON
fn parse_sport_specific(
    sport: Sport,
    details: &serde_json::Value,
) -> crate::models::SportSpecificState {
    use crate::models::*;

    match sport {
        Sport::NFL | Sport::NCAAF => {
            let down = details["down"].as_u64().map(|v| v as u8);
            let yards_to_go = details["yards_to_go"].as_u64().map(|v| v as u8);
            let yard_line = details["yard_line"].as_u64().map(|v| v as u8);
            let is_redzone = details["is_redzone"].as_bool().unwrap_or(false);

            SportSpecificState::Football(FootballState {
                down,
                yards_to_go,
                yard_line,
                is_redzone,
                timeouts_home: 3, // Default values
                timeouts_away: 3,
            })
        }
        Sport::NBA | Sport::NCAAB => {
            SportSpecificState::Basketball(BasketballState {
                timeouts_home: 7,
                timeouts_away: 7,
                home_team_fouls: 0,
                away_team_fouls: 0,
            })
        }
        Sport::NHL => SportSpecificState::Hockey(HockeyState {
            power_play_team: None,
            power_play_seconds_remaining: None,
            home_goalie_pulled: false,
            away_goalie_pulled: false,
        }),
        Sport::MLB => SportSpecificState::Baseball(BaseballState {
            outs: 0,
            base_runners: 0,
        }),
        Sport::MLS | Sport::Soccer => SportSpecificState::Soccer(SoccerState {
            home_red_cards: 0,
            away_red_cards: 0,
        }),
        _ => SportSpecificState::Other,
    }
}

#[async_trait]
impl ProbabilityModel for SportWinProbabilityModel {
    async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64> {
        // Convert to GameState
        let game_state = Self::event_state_to_game_state(event_state)?;

        // Use existing win probability calculation
        let prob = calculate_win_probability(&game_state, for_entity_a);

        // Validate probability range
        if !(0.0..=1.0).contains(&prob) {
            return Err(anyhow!(
                "Invalid probability: {} (must be between 0.0 and 1.0)",
                prob
            ));
        }

        Ok(prob)
    }

    async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64> {
        // Use existing win probability calculation directly
        let prob = calculate_win_probability(game_state, for_home_team);

        // Validate probability range
        if !(0.0..=1.0).contains(&prob) {
            return Err(anyhow!(
                "Invalid probability: {} (must be between 0.0 and 1.0)",
                prob
            ));
        }

        Ok(prob)
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Sport { .. })
    }

    fn model_name(&self) -> &str {
        "SportWinProbability"
    }
}

impl Default for SportWinProbabilityModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BasketballState, SportSpecificState};
    use crate::providers::{EventStatus, SportStateData};
    use chrono::Utc;

    #[tokio::test]
    async fn test_sport_model_supports() {
        let model = SportWinProbabilityModel::new();
        assert_eq!(model.model_name(), "SportWinProbability");

        let sport_market = MarketType::sport(Sport::NBA);
        assert!(model.supports(&sport_market));

        let politics_market = MarketType::Politics {
            region: "us".to_string(),
            event_type: crate::models::PoliticsEventType::Election,
        };
        assert!(!model.supports(&politics_market));
    }

    #[tokio::test]
    async fn test_sport_model_legacy_calculation() {
        let model = SportWinProbabilityModel::new();

        // Create a tied NBA game at halftime
        let game_state = GameState {
            event_id: "test".to_string(),
            market_type: Some(MarketType::sport(Sport::NBA)),
            entity_a: Some("PHI".to_string()),
            entity_b: Some("NYK".to_string()),
            event_start: Some(Utc::now()),
            event_end: None,
            resolution_criteria: None,
            game_id: "test".to_string(),
            sport: Sport::NBA,
            home_team: "PHI".to_string(),
            away_team: "NYK".to_string(),
            home_score: 50,
            away_score: 50,
            period: 2,
            time_remaining_seconds: 720,
            possession: None,
            fetched_at: Utc::now(),
            pregame_home_prob: None,
            sport_specific: SportSpecificState::Basketball(BasketballState::default()),
            market_specific: None,
        };

        let prob = model
            .calculate_probability_legacy(&game_state, true)
            .await
            .unwrap();

        // Should have home court advantage (~50-55%)
        assert!(prob > 0.50 && prob < 0.70);
    }

    #[tokio::test]
    async fn test_event_state_conversion() {
        let model = SportWinProbabilityModel::new();

        // Create EventState for an NBA game
        let event_state = EventState {
            event_id: "test".to_string(),
            market_type: MarketType::sport(Sport::NBA),
            entity_a: "PHI".to_string(),
            entity_b: Some("NYK".to_string()),
            status: EventStatus::Live,
            state: StateData::Sport(SportStateData {
                score_a: 72,
                score_b: 68,
                period: 3,
                time_remaining: 420,
                possession: Some("home".to_string()),
                sport_details: serde_json::json!({}),
            }),
            fetched_at: Utc::now(),
        };

        let prob = model
            .calculate_probability(&event_state, true)
            .await
            .unwrap();

        // Home team leading by 4 points, should be favored
        assert!(prob > 0.60 && prob <= 1.0);
    }
}
