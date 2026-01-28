//! Universal event state database operations
//!
//! Provides functions to insert event state for any market type into the database.
//! Handles mapping from universal EventState/GameState to database schema.

use crate::models::GameState;
use crate::providers::EventState;
use anyhow::Result;
use sqlx::PgPool;
use tracing::{debug, warn};

/// Insert event state for any market type
///
/// This function handles both legacy GameState and new EventState formats.
/// For sports markets, it inserts into game_states table.
/// For non-sports markets (crypto, economics, politics), it logs a warning
/// as these will require additional database tables in Phase 7+.
pub async fn insert_event_state(pool: &PgPool, state: &GameState) -> Result<()> {
    // Extract sport string from market type or use legacy sport field
    let sport_str = state
        .get_sport()
        .map(|s| s.as_str().to_lowercase())
        .unwrap_or_else(|| "nba".to_string());

    // Format time remaining as "MM:SS"
    let time_str = format!(
        "{}:{:02}",
        state.time_remaining_seconds / 60,
        state.time_remaining_seconds % 60
    );

    // Determine status string
    let status = if state.time_remaining_seconds > 0 {
        "STATUS_IN_PROGRESS"
    } else {
        "STATUS_FINAL"
    };

    debug!(
        "Inserting game state: game_id={}, sport={}, score={}–{}, period={}, time={}",
        state.game_id, sport_str, state.home_score, state.away_score, state.period, time_str
    );

    sqlx::query(
        r#"
        INSERT INTO game_states (
            game_id, sport, home_score, away_score, period,
            time_remaining, status, possession, home_win_prob, time
        )
        VALUES ($1, $2::sport_enum, $3, $4, $5, $6, $7, $8, $9, NOW())
        ON CONFLICT (game_id, time) DO UPDATE SET
            home_score = EXCLUDED.home_score,
            away_score = EXCLUDED.away_score,
            period = EXCLUDED.period,
            time_remaining = EXCLUDED.time_remaining,
            status = EXCLUDED.status,
            possession = EXCLUDED.possession,
            home_win_prob = EXCLUDED.home_win_prob
        "#,
    )
    .bind(&state.game_id)
    .bind(&sport_str)
    .bind(state.home_score as i32)
    .bind(state.away_score as i32)
    .bind(state.period as i32)
    .bind(&time_str)
    .bind(status)
    .bind(&state.possession)
    .bind(state.pregame_home_prob.unwrap_or(0.5))
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert event state from the universal EventState format
///
/// Converts EventState to GameState for database insertion.
/// This allows new-style event data to be stored in the existing schema.
pub async fn insert_from_event_state(pool: &PgPool, event_state: &EventState) -> Result<()> {
    use crate::models::{Sport, SportSpecificState};
    use crate::providers::StateData;

    // Convert EventState to GameState for database insertion
    let game_state = match &event_state.state {
        StateData::Sport(sport_data) => {
            // Extract sport from market type
            let sport = event_state
                .market_type
                .as_sport()
                .unwrap_or(Sport::NBA);

            GameState {
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
                away_team: event_state.entity_b.clone().unwrap_or_default(),
                home_score: sport_data.score_a,
                away_score: sport_data.score_b,
                period: sport_data.period,
                time_remaining_seconds: sport_data.time_remaining,
                possession: sport_data.possession.clone(),
                fetched_at: event_state.fetched_at,
                pregame_home_prob: None,
                sport_specific: SportSpecificState::Other,
                market_specific: None,
            }
        }
        StateData::Crypto(_) | StateData::Economics(_) | StateData::Politics(_) | StateData::Entertainment(_) => {
            // Non-sports markets will be supported in a future phase
            // For now, log a warning and skip insertion
            warn!(
                "Non-sports event state insertion not yet supported: {} ({:?})",
                event_state.event_id, event_state.market_type.type_name()
            );
            return Ok(());
        }
    };

    insert_event_state(pool, &game_state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MarketType, Sport, SportSpecificState};

    fn make_test_game_state() -> GameState {
        GameState {
            event_id: "test-game-123".to_string(),
            market_type: Some(MarketType::sport(Sport::NBA)),
            entity_a: Some("Lakers".to_string()),
            entity_b: Some("Celtics".to_string()),
            event_start: None,
            event_end: None,
            resolution_criteria: None,
            game_id: "test-game-123".to_string(),
            sport: Sport::NBA,
            home_team: "Lakers".to_string(),
            away_team: "Celtics".to_string(),
            home_score: 85,
            away_score: 82,
            period: 3,
            time_remaining_seconds: 420, // 7 minutes
            possession: Some("home".to_string()),
            fetched_at: Utc::now(),
            pregame_home_prob: Some(0.55),
            sport_specific: SportSpecificState::Other,
            market_specific: None,
        }
    }

    #[test]
    fn test_game_state_time_format() {
        let state = make_test_game_state();
        let time_str = format!(
            "{}:{:02}",
            state.time_remaining_seconds / 60,
            state.time_remaining_seconds % 60
        );
        assert_eq!(time_str, "7:00");
    }

    #[test]
    fn test_sport_extraction() {
        let state = make_test_game_state();
        let sport_str = state
            .get_sport()
            .map(|s| s.as_str().to_lowercase())
            .unwrap_or_else(|| "nba".to_string());
        assert_eq!(sport_str, "nba");
    }
}
