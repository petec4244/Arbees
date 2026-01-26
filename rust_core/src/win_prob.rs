//! Win probability calculation for live sports games.
//!
//! This module provides high-performance win probability models for various sports.
//! The calculations are based on:
//! - Score differential
//! - Time remaining
//! - Possession (for applicable sports)
//! - Field position (NFL/NCAAF)
//! - Historical data-derived coefficients

use crate::models::{GameState, Sport};

/// Logistic function for probability calculation
#[inline]
fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Home field advantage in points by sport
const NFL_HOME_ADVANTAGE_POINTS: f64 = 2.5;
const NCAAF_HOME_ADVANTAGE_POINTS: f64 = 3.0;

/// Calculate NFL/NCAAF win probability
///
/// Based on:
/// - Score differential (most important)
/// - Home field advantage (decays with time)
/// - Time remaining (exponential decay of variance)
/// - Field position value (if in possession)
/// - Down and distance (situational)
fn calculate_football_win_prob(state: &GameState, for_home: bool) -> f64 {
    let score_diff = if for_home {
        state.home_score as f64 - state.away_score as f64
    } else {
        state.away_score as f64 - state.home_score as f64
    };

    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_fraction = remaining / total_seconds;

    // Volatility decreases as game progresses (roughly sqrt of time remaining)
    // At game start, ~14 point swing is common; at end, much less
    let volatility: f64 = 14.0 * time_fraction.sqrt();

    // Home field advantage - decays as game progresses
    // NFL: ~2.5 points, NCAAF: ~3.0 points (college crowds are louder)
    let home_advantage = match state.sport {
        Sport::NFL => NFL_HOME_ADVANTAGE_POINTS * time_fraction.sqrt(),
        Sport::NCAAF => NCAAF_HOME_ADVANTAGE_POINTS * time_fraction.sqrt(),
        _ => 0.0,
    };
    let home_adj = if for_home { home_advantage } else { -home_advantage };

    // Base probability from score differential + home advantage
    let mut log_odds = (score_diff + home_adj) / volatility.max(1.0);

    // Possession bonus: having the ball is worth ~2.5 points on average
    if let Some(ref poss) = state.possession {
        let has_possession = if for_home {
            poss == &state.home_team
        } else {
            poss == &state.away_team
        };

        if has_possession {
            // Possession value increases with field position
            let field_value = match state.yard_line {
                Some(yl) if state.is_redzone => 4.0 + (20 - yl.min(20)) as f64 * 0.1,
                Some(yl) => 2.5 + (50 - yl.min(50)) as f64 * 0.03,
                None => 2.5,
            };
            log_odds += field_value / volatility.max(1.0);
        }
    }

    // Down and distance adjustment
    if let (Some(down), Some(ytg)) = (state.down, state.yards_to_go) {
        let down_factor = match down {
            1 => 0.0,       // First down is neutral
            2 => -0.1,      // Slight disadvantage
            3 => -0.3,      // Larger disadvantage
            4 => -0.5,      // Big disadvantage (usually punt/FG)
            _ => 0.0,
        };
        // Long yardage is worse
        let ytg_factor = -(ytg as f64 - 7.0) * 0.02;
        log_odds += (down_factor + ytg_factor) / volatility.max(1.0);
    }

    logistic(log_odds)
}

/// Home court advantage in points by sport
const NBA_HOME_ADVANTAGE_POINTS: f64 = 3.0;
const NCAAB_HOME_ADVANTAGE_POINTS: f64 = 4.0;

/// Calculate NBA/NCAAB win probability
///
/// Basketball has more scoring, so volatility model differs.
/// Based on score differential and possessions remaining.
///
/// This model accounts for:
/// - **Home court advantage**: NBA (~3 pts), NCAAB (~4 pts) added directly to score diff
/// - **Catch-up difficulty**: large deficits late are nearly insurmountable
/// - **Possession value**: having the ball is worth ~1 point
///
/// The volatility is adjusted based on the absolute score differential and time
/// remaining, applied symmetrically so probabilities remain complementary.
fn calculate_basketball_win_prob(state: &GameState, for_home: bool) -> f64 {
    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_remaining_pct = remaining / total_seconds;

    // Home court advantage - applied as equivalent score points
    // Decays linearly with time (at halftime = half value, at end = near zero)
    // This is NOT scaled by volatility so it has more impact early in the game
    let home_advantage_points = match state.sport {
        Sport::NBA => NBA_HOME_ADVANTAGE_POINTS * time_remaining_pct,
        Sport::NCAAB => NCAAB_HOME_ADVANTAGE_POINTS * time_remaining_pct,
        _ => 0.0,
    };

    // Calculate effective score differential including home advantage
    let raw_home_diff = state.home_score as f64 - state.away_score as f64;
    let adjusted_home_diff = raw_home_diff + home_advantage_points;
    let score_diff = if for_home { adjusted_home_diff } else { -adjusted_home_diff };

    // Estimate possessions remaining (about 100 possessions per game for NBA)
    let possessions_remaining = time_remaining_pct * 100.0;

    // Points per possession ~1.1, variance ~1.0
    // Base volatility = sqrt(possessions) * variance_per_possession
    let base_volatility = (possessions_remaining.max(1.0)).sqrt() * 2.2;

    // Calculate catch-up difficulty factor based on absolute score differential
    // Use the RAW score diff (without home advantage) for difficulty calculation
    // This is applied SYMMETRICALLY so probabilities remain complementary
    let trailing_team_possessions = possessions_remaining / 2.0;
    let abs_score_diff = raw_home_diff.abs();

    // Required points per possession to overcome the deficit
    let required_margin_per_poss = if trailing_team_possessions > 0.5 && abs_score_diff > 0.0 {
        abs_score_diff / trailing_team_possessions
    } else {
        0.0
    };

    // Apply difficulty scaling that compresses probabilities toward extremes
    // when comebacks are unrealistic. This makes volatility shrink, which
    // pushes probabilities toward 0 or 100%.
    let difficulty_factor = if required_margin_per_poss > 0.5 {
        // Exponential scaling: larger deficits with less time = lower volatility
        // This makes the leading team's probability approach 100%
        let excess = required_margin_per_poss - 0.5;
        1.5_f64.powf(excess * 1.5) // Smooth exponential
    } else {
        1.0
    };

    // Reduce volatility based on difficulty (makes outcomes more certain)
    let volatility = (base_volatility / difficulty_factor).max(0.3);

    // Possession is worth about 1 point in basketball
    let mut possession_adj = 0.0;
    if let Some(ref poss) = state.possession {
        let has_possession = if for_home {
            poss == &state.home_team
        } else {
            poss == &state.away_team
        };
        if has_possession {
            possession_adj = 1.0;
        }
    }

    let log_odds = (score_diff + possession_adj) / volatility.max(0.3);
    logistic(log_odds)
}

/// Calculate NHL win probability
///
/// Hockey is low-scoring with strong home ice advantage.
fn calculate_hockey_win_prob(state: &GameState, for_home: bool) -> f64 {
    let score_diff = if for_home {
        state.home_score as f64 - state.away_score as f64
    } else {
        state.away_score as f64 - state.home_score as f64
    };

    // Home ice advantage: ~3-4% edge
    let home_adj = if for_home { 0.15 } else { -0.15 };

    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_fraction = remaining / total_seconds;

    // Hockey volatility: ~2.5 goals per team per game
    let volatility: f64 = 2.5 * time_fraction.sqrt();

    let log_odds = (score_diff + home_adj) / volatility.max(0.5);
    logistic(log_odds)
}

/// Calculate MLB win probability
///
/// Baseball uses innings and outs, non-linear scoring patterns.
fn calculate_baseball_win_prob(state: &GameState, for_home: bool) -> f64 {
    let score_diff = if for_home {
        state.home_score as f64 - state.away_score as f64
    } else {
        state.away_score as f64 - state.home_score as f64
    };

    // Innings remaining (period = current inning)
    let innings_remaining = (9.0 - state.period as f64).max(0.0);
    let is_bottom = state.possession.as_ref().map(|p| p == &state.home_team).unwrap_or(false);

    // Expected runs remaining: ~4.5 runs per game, so ~0.5 per inning
    let runs_remaining = innings_remaining * 0.5 + if is_bottom && for_home { 0.25 } else { 0.0 };

    // Volatility decreases as game progresses
    let volatility = (runs_remaining * 2.0).max(0.5);

    // Home team bats last (walk-off advantage)
    let home_adj = if for_home && innings_remaining <= 1.0 { 0.1 } else { 0.0 };

    let log_odds = (score_diff + home_adj) / volatility;
    logistic(log_odds)
}

/// Calculate soccer/MLS win probability
fn calculate_soccer_win_prob(state: &GameState, for_home: bool) -> f64 {
    let score_diff = if for_home {
        state.home_score as f64 - state.away_score as f64
    } else {
        state.away_score as f64 - state.home_score as f64
    };

    // Home advantage in soccer is significant: ~0.4 goals
    let home_adj = if for_home { 0.4 } else { -0.4 };

    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_fraction = remaining / total_seconds;

    // ~2.5 goals per game total, volatility decreases with time
    let volatility: f64 = 1.5 * time_fraction.sqrt();

    let log_odds = (score_diff + home_adj) / volatility.max(0.3);
    logistic(log_odds)
}

/// Calculate win probability for a team in the current game state
pub fn calculate_win_probability(state: &GameState, for_home: bool) -> f64 {
    match state.sport {
        Sport::NFL | Sport::NCAAF => calculate_football_win_prob(state, for_home),
        Sport::NBA | Sport::NCAAB => calculate_basketball_win_prob(state, for_home),
        Sport::NHL => calculate_hockey_win_prob(state, for_home),
        Sport::MLB => calculate_baseball_win_prob(state, for_home),
        Sport::MLS | Sport::Soccer => calculate_soccer_win_prob(state, for_home),
        // Default model for other sports
        Sport::Tennis | Sport::MMA => {
            let score_diff = if for_home {
                state.home_score as f64 - state.away_score as f64
            } else {
                state.away_score as f64 - state.home_score as f64
            };
            let volatility = 3.0;
            logistic(score_diff / volatility)
        }
    }
}

/// Calculate win probability change from a state transition
pub fn calculate_win_prob_delta(
    old_state: &GameState,
    new_state: &GameState,
    for_home: bool,
) -> f64 {
    let old_prob = calculate_win_probability(old_state, for_home);
    let new_prob = calculate_win_probability(new_state, for_home);
    new_prob - old_prob
}

/// Batch calculate win probabilities for multiple game states.
///
/// Uses parallel processing for optimal performance.
pub fn batch_calculate_win_probs(states: &[GameState], for_home: bool) -> Vec<f64> {
    use rayon::prelude::*;
    states
        .par_iter()
        .map(|state| calculate_win_probability(state, for_home))
        .collect()
}

/// Estimate expected points from current field position (NFL/NCAAF)
///
/// Based on historical expected points models.
pub fn expected_points_from_field_position(yard_line: u8, down: u8, yards_to_go: u8) -> f64 {
    // Simplified EP model based on yard line
    // Real models use comprehensive lookup tables
    let field_value = match yard_line {
        0..=10 => 6.0 - (10 - yard_line) as f64 * 0.3,   // Red zone
        11..=20 => 4.0 - (20 - yard_line) as f64 * 0.2,  // Near red zone
        21..=50 => 2.0 - (50 - yard_line) as f64 * 0.04, // Midfield
        51..=80 => 0.5 - (80 - yard_line) as f64 * 0.05, // Own territory
        _ => -0.5,                                        // Deep in own territory
    };

    // Adjust for down and distance
    let down_adj = match down {
        1 => 0.0,
        2 => -0.5 - (yards_to_go as f64 - 7.0) * 0.1,
        3 => -1.0 - (yards_to_go as f64 - 5.0) * 0.15,
        4 => -2.0,
        _ => 0.0,
    };

    field_value + down_adj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_nba_state(home_score: u16, away_score: u16, period: u8, time_remaining: u32) -> GameState {
        GameState {
            game_id: "test".to_string(),
            sport: Sport::NBA,
            home_team: "PHI".to_string(),
            away_team: "NYK".to_string(),
            home_score,
            away_score,
            period,
            time_remaining_seconds: time_remaining,
            possession: None,
            down: None,
            yards_to_go: None,
            yard_line: None,
            is_redzone: false,
        }
    }

    #[test]
    fn test_tied_game_start_home_advantage() {
        let state = make_nba_state(0, 0, 1, 720);
        let home_prob = calculate_win_probability(&state, true);
        let away_prob = calculate_win_probability(&state, false);

        // Home team should have an advantage at game start (tied score)
        // With 3-point home advantage, home team should be favored (>50%)
        assert!(
            home_prob > 0.50,
            "Home team should be favored in tied game: {:.3}",
            home_prob
        );

        // Probabilities should sum to ~1.0 (complementary)
        assert!(
            (home_prob + away_prob - 1.0).abs() < 0.01,
            "Probs should sum to 1: {:.3} + {:.3} = {:.3}",
            home_prob,
            away_prob,
            home_prob + away_prob
        );

        // But advantage shouldn't be overwhelming at game start
        assert!(
            home_prob < 0.65,
            "Home advantage shouldn't be too large: {:.3}",
            home_prob
        );
    }

    #[test]
    fn test_big_lead_late() {
        let state = make_nba_state(110, 85, 4, 60);
        let home_prob = calculate_win_probability(&state, true);
        // 25 point lead with 1 minute left should be very high
        assert!(home_prob > 0.95);
    }

    #[test]
    fn test_close_game_late() {
        let state = make_nba_state(95, 93, 4, 60);
        let home_prob = calculate_win_probability(&state, true);
        // 2 point lead with 1 minute left should favor home but not certain
        assert!(home_prob > 0.5 && home_prob < 0.85);
    }

    #[test]
    fn test_home_advantage_decays_with_time() {
        // Test that home advantage is smaller late in the game
        let early_state = make_nba_state(50, 50, 1, 720); // Q1, tied
        let late_state = make_nba_state(90, 90, 4, 60);  // Q4, 1 min left, tied

        let early_home = calculate_win_probability(&early_state, true);
        let late_home = calculate_win_probability(&late_state, true);

        // Early game: home advantage should give clear edge
        let early_advantage = early_home - 0.5;
        // Late game: home advantage should be much smaller
        let late_advantage = late_home - 0.5;

        // Home advantage should decay (early > late)
        assert!(
            early_advantage > late_advantage,
            "Home advantage should decay: early={:.3} > late={:.3}",
            early_advantage,
            late_advantage
        );
    }

    #[test]
    fn test_ncaab_home_advantage_larger() {
        // NCAAB has larger home court advantage than NBA
        let nba_state = GameState {
            game_id: "test".to_string(),
            sport: Sport::NBA,
            home_team: "PHI".to_string(),
            away_team: "NYK".to_string(),
            home_score: 0,
            away_score: 0,
            period: 1,
            time_remaining_seconds: 720,
            possession: None,
            down: None,
            yards_to_go: None,
            yard_line: None,
            is_redzone: false,
        };

        let ncaab_state = GameState {
            sport: Sport::NCAAB,
            ..nba_state.clone()
        };

        let nba_home = calculate_win_probability(&nba_state, true);
        let ncaab_home = calculate_win_probability(&ncaab_state, true);

        // NCAAB should have larger home advantage
        assert!(
            ncaab_home > nba_home,
            "NCAAB home advantage should be larger: NCAAB={:.3} > NBA={:.3}",
            ncaab_home,
            nba_home
        );
    }
}
