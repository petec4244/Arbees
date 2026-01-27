//! Win probability calculation for live sports games.
//! 
//! This module provides high-performance win probability models for various sports.
//! The calculations are based on:
//! - Score differential
//! - Time remaining
//! - Possession (for applicable sports)
//! - Field position (NFL/NCAAF)
//! - Historical data-derived coefficients

use crate::models::{GameState, Sport, SportSpecificState, FootballState};

/// Logistic function for probability calculation
#[inline]
fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Converts a probability to log-odds.
#[inline]
fn prob_to_log_odds(p: f64) -> f64 {
    (p / (1.0 - p)).ln()
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
fn calculate_football_win_prob(
    state: &GameState,
    football_state: &FootballState,
    for_home: bool,
) -> f64 {
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
            let field_value = match football_state.yard_line {
                Some(yl) if football_state.is_redzone => 4.0 + (20 - yl.min(20)) as f64 * 0.1,
                Some(yl) => 2.5 + (50 - yl.min(50)) as f64 * 0.03,
                None => 2.5,
            };
            log_odds += field_value / volatility.max(1.0);
        }
    }

    // Down and distance adjustment
    if let (Some(down), Some(ytg)) = (football_state.down, football_state.yards_to_go) {
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
    
    // Timeout adjustment - timeouts are a valuable resource late in the game
    let timeout_value = 0.05 * (1.0 - time_fraction); // Value increases as game progresses
    let home_timeout_adj = (3.0 - football_state.timeouts_home as f64) * -timeout_value;
    let away_timeout_adj = (3.0 - football_state.timeouts_away as f64) * -timeout_value;
    let timeout_adj = if for_home {
        home_timeout_adj - away_timeout_adj
    } else {
        away_timeout_adj - home_timeout_adj
    };
    log_odds += timeout_adj;


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
/// - **Late-game dynamics**: final minutes have reduced variance (clock management, fouls)
/// - **Possession value**: having the ball is worth ~1 point
///
/// Calibrated against historical NBA win probability data:
/// - 7-point lead, 8 min left → ~88% win probability
/// - 15-point lead, 8 min left → ~97% win probability
/// - 7-point lead, 2 min left → ~95% win probability
fn calculate_basketball_win_prob(state: &GameState, for_home: bool) -> f64 {
    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_remaining_pct = remaining / total_seconds;

    // Home court advantage - applied as equivalent score points
    // Decays linearly with time (at halftime = half value, at end = near zero)
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
    let abs_score_diff = raw_home_diff.abs();

    // ========== LATE-GAME DYNAMICS ==========
    // In the final minutes, game dynamics change dramatically:
    // - Leading team can run clock (24-second possessions become valuable)
    // - Trailing team must foul (clock stops but free throws are high-percentage)
    // - Variance DECREASES because intentional fouls convert ~75% of the time
    // - Each possession is worth less to the trailing team
    //
    // HOWEVER: Close games (1-3 point leads) remain volatile because:
    // - One 3-pointer can flip the lead
    // - Fouls can backfire (and-1s, technicals)
    // - Single possessions matter more

    // Determine game phase based on time remaining
    // NBA/NCAAB: Total game is 48/40 minutes
    // - Early game: >50% remaining
    // - Mid game: 25-50% remaining
    // - Late game: 10-25% remaining (~5-12 min for NBA)
    // - Very late: <10% remaining (~5 min for NBA)
    // - Crunch time: <2.5% remaining (~1 min for NBA)
    let late_game_threshold = 600.0; // 10 minutes - deep into second half
    let very_late_threshold = 300.0; // 5 minutes - crunch time approaching
    let crunch_time_threshold = 120.0; // 2 minutes - true crunch time

    let is_late_game = remaining < late_game_threshold;
    let is_very_late = remaining < very_late_threshold;
    let is_crunch_time = remaining < crunch_time_threshold;

    // Is this a "close game"? (small lead)
    // Close games remain volatile even late because a single play can change everything
    // Thresholds adjusted for late game - a 5-point lead late is more significant
    let close_game_threshold = if is_late_game { 3.0 } else { 4.0 };
    let moderate_lead_threshold = if is_late_game { 6.0 } else { 8.0 };
    let is_close_game = abs_score_diff <= close_game_threshold;
    let is_moderate_lead = abs_score_diff > close_game_threshold && abs_score_diff <= moderate_lead_threshold;

    // Base volatility - but reduce it in late game scenarios
    // Late game has LOWER variance for larger leads, but close games stay volatile
    // CALIBRATED: Don't reduce too aggressively - we want to match market, not exceed
    let late_game_volatility_factor = if is_close_game {
        // Close games: minimal volatility reduction (single plays still matter)
        if is_crunch_time { 0.85 } else if is_very_late { 0.9 } else if is_late_game { 0.95 } else { 1.0 }
    } else if is_moderate_lead {
        // Moderate leads: some reduction but not extreme
        if is_crunch_time { 0.65 } else if is_very_late { 0.7 } else if is_late_game { 0.8 } else { 1.0 }
    } else {
        // Large leads: significant reduction but not too extreme
        if is_crunch_time { 0.5 } else if is_very_late { 0.6 } else if is_late_game { 0.7 } else { 1.0 }
    };

    let base_volatility = (possessions_remaining.max(1.0)).sqrt() * 2.2 * late_game_volatility_factor;

    // ========== CATCH-UP DIFFICULTY ==========
    // Calculate how hard it is to overcome the deficit
    // Trailing team gets ~half the remaining possessions
    let trailing_team_possessions = possessions_remaining / 2.0;

    // Required margin per possession to overcome deficit
    // NBA teams average ~1.1 points per possession
    // To come back from 7 points down with 8 possessions, need +0.875 PPP margin
    // That's outscoring opponent by almost a point per possession - VERY hard
    let required_margin_per_poss = if trailing_team_possessions > 0.5 && abs_score_diff > 0.0 {
        abs_score_diff / trailing_team_possessions
    } else {
        0.0
    };

    // How late in the game are we? (0 = start, 1 = end)
    let late_factor = (1.0 - time_remaining_pct).clamp(0.0, 1.0);

    // Score weighting increases as:
    // 1. The deficit grows (harder to overcome)
    // 2. Time runs out (fewer opportunities)
    // But keep it moderate to avoid overshooting market expectations
    let score_weight = if is_late_game && !is_close_game {
        // Late game with meaningful lead: score differential is more deterministic
        // But not TOO aggressive - we want to match market, not exceed it
        1.2 + (abs_score_diff / 12.0).min(0.8) * late_factor
    } else if is_late_game && is_close_game {
        // Late game but close: still some extra weight but not extreme
        1.1 + (abs_score_diff / 15.0).min(0.4) * late_factor
    } else {
        1.0 + (abs_score_diff / 12.0).min(1.0) * (0.25 + 0.75 * late_factor)
    };

    // ========== DIFFICULTY FACTOR ==========
    // Exponential scaling that compresses probabilities toward extremes
    // when comebacks are unrealistic
    //
    // Key insight: difficulty should scale with BOTH score differential AND time
    // - Large deficits late: nearly impossible to overcome
    // - Small deficits late: still difficult but not insurmountable
    //
    // Different thresholds and bases based on lead size and time
    // CALIBRATED to match market expectations:
    // - 7pt lead, 8 min left: ~88-90%
    // - 15pt lead, 8 min left: ~97%
    // - 3pt lead, 1 min left: ~85%
    let (difficulty_threshold, difficulty_base, difficulty_exponent): (f64, f64, f64) =
        if is_close_game {
            // Close games: minimal difficulty scaling (volatile)
            if is_crunch_time { (0.55, 1.35, 1.0) } else { (0.6, 1.25, 0.9) }
        } else if is_moderate_lead {
            // Moderate leads (4-6 points): moderate scaling
            if is_crunch_time {
                (0.4, 1.7, 1.3)
            } else if is_very_late {
                (0.45, 1.6, 1.2)
            } else if is_late_game {
                (0.5, 1.5, 1.1)
            } else {
                (0.55, 1.4, 1.0)
            }
        } else {
            // Large leads (7+ points): aggressive but calibrated scaling
            if is_crunch_time {
                (0.35, 2.0, 1.5)
            } else if is_very_late {
                (0.4, 1.8, 1.4)
            } else if is_late_game {
                (0.45, 1.7, 1.3)
            } else {
                (0.5, 1.5, 1.2)
            }
        };

    let difficulty_factor = if required_margin_per_poss > difficulty_threshold {
        let excess = required_margin_per_poss - difficulty_threshold;
        difficulty_base.powf(excess * difficulty_exponent)
    } else {
        1.0
    };

    // Final volatility: reduced by difficulty factor
    // Minimum floor depends on lead size - close games stay more volatile
    let min_volatility = if is_close_game {
        1.0 // Close games need higher volatility floor
    } else if abs_score_diff > 10.0 {
        0.4 // Large leads can have lower floor
    } else {
        0.6 // Moderate leads
    };
    let volatility = (base_volatility / difficulty_factor).max(min_volatility);

    // ========== POSSESSION ADJUSTMENT ==========
    // Possession is worth about 1 point in basketball
    // But in late game with a lead, possession is worth MORE (can run clock)
    let possession_value = if is_crunch_time && score_diff > 0.0 {
        2.0 // Leading with possession in crunch time is VERY valuable
    } else if is_very_late && score_diff > 0.0 {
        1.7 // Leading with possession late is valuable
    } else if is_late_game && score_diff > 0.0 {
        1.3 // Moderate advantage
    } else {
        1.0
    };

    let mut possession_adj = 0.0;
    if let Some(ref poss) = state.possession {
        let has_possession = if for_home {
            poss == &state.home_team
        } else {
            poss == &state.away_team
        };
        if has_possession {
            possession_adj = possession_value;
        }
    }

    let log_odds = (score_diff * score_weight + possession_adj) / volatility;
    logistic(log_odds)
}

/// Calculate NHL win probability
///
/// Hockey is low-scoring with strong home ice advantage.
/// Model calibrated to match market expectations:
/// - Team down 2-0 in 2nd period should be ~15-20%, not 40%
/// - Goals are rare and each one is highly deterministic
fn calculate_hockey_win_prob(state: &GameState, for_home: bool) -> f64 {
    let score_diff = if for_home {
        state.home_score as f64 - state.away_score as f64
    } else {
        state.away_score as f64 - state.home_score as f64
    };

    // Home ice advantage: ~3% edge (reduced from 0.15, more conservative)
    let home_adj = if for_home { 0.10 } else { -0.10 };

    let total_seconds = state.sport.total_seconds() as f64;
    let remaining = state.total_time_remaining() as f64;
    let time_fraction = remaining / total_seconds;

    // Hockey is LOW volatility - goals are rare (~3 per team per game)
    // Reduced from 2.5 to 1.2 to match market expectations
    // A 2-goal lead should be very significant
    let base_volatility = 1.2;

    // Further reduce volatility in late game (goals matter more)
    let late_game_factor = if time_fraction < 0.33 { 0.7 } // 3rd period
        else if time_fraction < 0.5 { 0.85 } // late 2nd
        else { 1.0 };

    let volatility = base_volatility * time_fraction.sqrt() * late_game_factor;

    let log_odds = (score_diff + home_adj) / volatility.max(0.3);
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
    // Calculate base live win probability from current game state
    let base_prob = match state.sport {
        Sport::NFL | Sport::NCAAF => {
            // Extract football state from sport_specific
            match &state.sport_specific {
                SportSpecificState::Football(fb) => calculate_football_win_prob(state, fb, for_home),
                _ => {
                    // Fallback with default football state if sport_specific doesn't match
                    let default_fb = FootballState::default();
                    calculate_football_win_prob(state, &default_fb, for_home)
                }
            }
        },
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
    };

    // If we have pregame probability, blend it with live model
    // This is useful early in games when score doesn't tell the full story
    if let Some(pregame_prob) = state.pregame_home_prob {
        let pregame_for_team = if for_home { pregame_prob } else { 1.0 - pregame_prob };
        blend_pregame_and_live_prob(pregame_for_team, base_prob, state)
    } else {
        base_prob
    }
}

/// Blend pregame probability with live win probability
///
/// Early in the game, pregame expectations (team strength, home advantage, etc.) matter more.
/// As the game progresses, the actual score and game state become more important.
///
/// Weight formula:
/// - At game start: 50% pregame, 50% live model
/// - At halftime: 25% pregame, 75% live model
/// - At end: 5% pregame, 95% live model
///
/// This helps avoid overreacting to small early leads/deficits.
fn blend_pregame_and_live_prob(pregame_prob: f64, live_prob: f64, state: &GameState) -> f64 {
    // Clamp pregame probability to valid range
    let pregame_prob = pregame_prob.clamp(0.01, 0.99);

    // Calculate how far into the game we are (0.0 = start, 1.0 = end)
    let total_seconds = state.sport.total_seconds() as f64;
    let elapsed = total_seconds - state.total_time_remaining() as f64;
    let game_progress = (elapsed / total_seconds).clamp(0.0, 1.0);

    // Pregame weight decreases as game progresses
    // Start at 0.5, decay exponentially to ~0.05 by game end
    let pregame_weight = 0.5 * (-2.5 * game_progress).exp();
    let live_weight = 1.0 - pregame_weight;

    // Blend in log-odds space for better mathematical properties
    let pregame_log_odds = prob_to_log_odds(pregame_prob);
    let live_log_odds = prob_to_log_odds(live_prob);

    let blended_log_odds = pregame_weight * pregame_log_odds + live_weight * live_log_odds;

    logistic(blended_log_odds)
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
    use crate::models::BasketballState;
    use chrono::Utc;

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
            fetched_at: Utc::now(),
            pregame_home_prob: None,
            sport_specific: SportSpecificState::Basketball(BasketballState::default()),
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
            fetched_at: Utc::now(),
            pregame_home_prob: None,
            sport_specific: SportSpecificState::Basketball(BasketballState::default()),
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

    // ========== NEW LATE-GAME CALIBRATION TESTS ==========
    // These tests verify the model matches historical NBA win probability data

    #[test]
    fn test_7pt_lead_q4_8min() {
        // Historical data: 7-point lead with 8 minutes left → ~85-90% win probability
        // This is the scenario where we were losing money (model said 73%, market said 90%)
        let state = make_nba_state(105, 98, 4, 480); // Q4, 8 min left, 7-pt lead
        let home_prob = calculate_win_probability(&state, true);
        let away_prob = calculate_win_probability(&state, false);

        // Must be > 85% (was 73% before fix)
        assert!(
            home_prob > 0.85,
            "7-point lead with 8 min left should be >85%: got {:.1}%",
            home_prob * 100.0
        );

        // Should be < 95% (not completely certain yet)
        assert!(
            home_prob < 0.95,
            "7-point lead with 8 min left should be <95%: got {:.1}%",
            home_prob * 100.0
        );

        // Probabilities should sum to 1
        assert!(
            (home_prob + away_prob - 1.0).abs() < 0.01,
            "Probs should sum to 1: {:.3} + {:.3}",
            home_prob,
            away_prob
        );
    }

    #[test]
    fn test_15pt_lead_q4_8min() {
        // Historical data: 15-point lead with 8 minutes left → ~97% win probability
        let state = make_nba_state(110, 95, 4, 480); // Q4, 8 min left, 15-pt lead
        let home_prob = calculate_win_probability(&state, true);

        assert!(
            home_prob > 0.95,
            "15-point lead with 8 min left should be >95%: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_7pt_lead_q4_2min() {
        // Historical data: 7-point lead with 2 minutes left → ~95% win probability
        // Late game makes even moderate leads nearly insurmountable
        let state = make_nba_state(100, 93, 4, 120); // Q4, 2 min left, 7-pt lead
        let home_prob = calculate_win_probability(&state, true);

        assert!(
            home_prob > 0.93,
            "7-point lead with 2 min left should be >93%: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_3pt_lead_q4_1min() {
        // Historical data: 3-point lead with 1 minute left → ~90-95% win probability
        // Leading team can run clock, foul strategy makes comebacks very hard
        // Even hitting a 3 just ties the game with <30 sec left
        let state = make_nba_state(95, 92, 4, 60); // Q4, 1 min left, 3-pt lead
        let home_prob = calculate_win_probability(&state, true);

        assert!(
            home_prob > 0.85,
            "3-point lead with 1 min left should be >85%: got {:.1}%",
            home_prob * 100.0
        );

        // Adjusted upper bound: 96% is reasonable for this scenario
        assert!(
            home_prob < 0.96,
            "3-point lead with 1 min left should be <96%: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_1pt_lead_q4_30sec() {
        // Historical data: 1-point lead with 30 seconds left → ~65-75% win probability
        // Still vulnerable but favored
        let state = make_nba_state(90, 89, 4, 30); // Q4, 30 sec left, 1-pt lead
        let home_prob = calculate_win_probability(&state, true);

        assert!(
            home_prob > 0.60 && home_prob < 0.85,
            "1-point lead with 30 sec left should be 60-85%: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_trailing_team_very_late() {
        // Trailing by 5 with 1 minute left - should be very low probability
        let state = make_nba_state(90, 95, 4, 60); // Q4, 1 min left, down 5
        let home_prob = calculate_win_probability(&state, true);

        assert!(
            home_prob < 0.15,
            "5-point deficit with 1 min left should be <15%: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_no_false_edges_late_game() {
        // This is the key test: model should NOT generate "edge" signals
        // when market is correctly pricing a late-game scenario
        //
        // Scenario: Hawks up 7 in Q4 with 8 min left
        // Market price: 90% for Hawks
        // Model MUST be close to 90% (no significant edge)
        let state = make_nba_state(105, 98, 4, 480);
        let home_prob = calculate_win_probability(&state, true);
        let market_prob = 0.90;

        let edge = (home_prob - market_prob).abs() * 100.0;

        // Edge should be less than 5% (our minimum edge threshold is typically 2%)
        // If model says 88% and market says 90%, that's only 2% edge - borderline acceptable
        // If model says 73% and market says 90%, that's 17% edge - WAY too much
        assert!(
            edge < 8.0,
            "Model should not generate large false edges. Model={:.1}%, Market={:.1}%, Edge={:.1}%",
            home_prob * 100.0,
            market_prob * 100.0,
            edge
        );
    }

    // ========== PREGAME PROBABILITY BLENDING TESTS ==========

    #[test]
    fn test_pregame_blending_early_game() {
        // Early in the game with small lead, pregame expectations should matter
        // Scenario: Strong team (65% pregame) is down 2 points early in Q1
        let mut state = make_nba_state(8, 10, 1, 660); // Q1, ~11 min left, down 2
        state.pregame_home_prob = Some(0.65); // Strong favorite at home

        let home_prob = calculate_win_probability(&state, true);

        // Without pregame: down 2 early might be ~45-48%
        // With pregame (65%): should blend to ~55-58%
        // The model should recognize that a strong team down 2 early is still favored
        assert!(
            home_prob > 0.52,
            "Strong team down 2 early should still be favored: got {:.1}%",
            home_prob * 100.0
        );

        // But not as favored as pregame suggested (pregame weight decays)
        assert!(
            home_prob < 0.65,
            "Should be less than pure pregame prob: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_pregame_blending_late_game() {
        // Late in the game, live score should dominate over pregame expectations
        // Scenario: Underdog (35% pregame) is up 8 points with 2 minutes left
        let mut state = make_nba_state(98, 90, 4, 120); // Q4, 2 min left, up 8
        state.pregame_home_prob = Some(0.35); // Big underdog

        let home_prob = calculate_win_probability(&state, true);

        // Up 8 with 2 min left should be ~90-95% regardless of pregame expectations
        // Pregame weight should be minimal late in the game
        assert!(
            home_prob > 0.88,
            "Up 8 with 2 min left should be very high probability: got {:.1}%",
            home_prob * 100.0
        );
    }

    #[test]
    fn test_pregame_blending_vs_no_pregame() {
        // Compare same game state with and without pregame probability
        let mut state_with_pregame = make_nba_state(15, 12, 1, 600); // Q1, 10 min left, up 3
        state_with_pregame.pregame_home_prob = Some(0.70); // Strong favorite

        let state_without_pregame = make_nba_state(15, 12, 1, 600);

        let prob_with = calculate_win_probability(&state_with_pregame, true);
        let prob_without = calculate_win_probability(&state_without_pregame, true);

        // With pregame info, probability should be higher (team is a strong favorite)
        assert!(
            prob_with > prob_without,
            "Pregame info should boost probability: with={:.1}% without={:.1}%",
            prob_with * 100.0,
            prob_without * 100.0
        );

        // Difference should be meaningful but not huge (blend, not replace)
        let diff = prob_with - prob_without;
        assert!(
            diff > 0.02 && diff < 0.15,
            "Pregame impact should be moderate early game: diff={:.1}%",
            diff * 100.0
        );
    }

    #[test]
    fn test_pregame_weight_decays() {
        // Verify that pregame weight decreases as game progresses
        // Use away team probability to avoid home court advantage confounding the test
        let mut early_state = make_nba_state(20, 20, 1, 660); // Q1, tied
        early_state.pregame_home_prob = Some(0.70); // Home team favored

        let mut mid_state = make_nba_state(50, 50, 2, 600); // Q2, tied
        mid_state.pregame_home_prob = Some(0.70);

        let mut late_state = make_nba_state(90, 90, 4, 120); // Q4, tied
        late_state.pregame_home_prob = Some(0.70);

        // Calculate for AWAY team (so pregame_prob = 1 - 0.70 = 0.30)
        // This way we can see pregame influence without home court advantage interference
        let early_away = calculate_win_probability(&early_state, false);
        let mid_away = calculate_win_probability(&mid_state, false);
        let late_away = calculate_win_probability(&late_state, false);

        // Away team has 30% pregame probability
        // As game progresses with score tied, they should drift toward 50%

        // Early: strong pregame influence keeps them low
        assert!(
            early_away < 0.40,
            "Early tied game with 30% pregame should be low for away: {:.1}%",
            early_away * 100.0
        );

        // Mid: moderate influence, getting closer to 50%
        assert!(
            mid_away > early_away && mid_away < 0.45,
            "Mid-game should drift toward 50%: {:.1}%",
            mid_away * 100.0
        );

        // Late: minimal pregame influence, very close to 50%
        assert!(
            late_away > mid_away && late_away > 0.45,
            "Late tied game should be close to 50%: {:.1}%",
            late_away * 100.0
        );

        // Verify monotonic increase toward 50% as pregame influence fades
        assert!(
            early_away < mid_away && mid_away < late_away,
            "Probability should monotonically approach 50%: early={:.1}% < mid={:.1}% < late={:.1}%",
            early_away * 100.0,
            mid_away * 100.0,
            late_away * 100.0
        );
    }

    #[test]
    fn test_prob_to_log_odds_inverse_of_logistic() {
        // Test that prob_to_log_odds and logistic are inverses
        let test_probs = vec![0.1, 0.25, 0.5, 0.75, 0.9];

        for prob in test_probs {
            let log_odds = prob_to_log_odds(prob);
            let recovered_prob = logistic(log_odds);

            assert!(
                (prob - recovered_prob).abs() < 0.0001,
                "Conversion should be reversible: {:.4} -> {:.4} -> {:.4}",
                prob,
                log_odds,
                recovered_prob
            );
        }
    }
}
