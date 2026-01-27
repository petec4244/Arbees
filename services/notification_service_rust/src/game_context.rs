//! Game context queries with freshness validation
//!
//! Provides accurate game state information by checking:
//! - Game status freshness (updated_at)
//! - Imminent games window (configurable)
//! - Session tracking for end-of-session digests

use anyhow::Result;
use chrono::{DateTime, Utc};
use log::warn;
use sqlx::PgPool;
use std::collections::HashSet;

/// Game counts with freshness validation
#[derive(Debug, Clone, Default)]
pub struct GameCounts {
    /// Games currently in progress (with fresh status)
    pub active: u64,
    /// Games starting within the imminent window
    pub imminent: u64,
    /// Games scheduled for today (broader view)
    pub upcoming_today: u64,
}

/// Tracks active game sessions for digest generation
#[derive(Debug)]
pub struct GameSessionTracker {
    /// Game IDs that were active in current session
    active_game_ids: HashSet<String>,
    /// When the current session started
    session_start: Option<DateTime<Utc>>,
    /// Total trades in this session
    session_trades: u64,
    /// Session PnL
    session_pnl: f64,
}

impl Default for GameSessionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl GameSessionTracker {
    pub fn new() -> Self {
        Self {
            active_game_ids: HashSet::new(),
            session_start: None,
            session_trades: 0,
            session_pnl: 0.0,
        }
    }

    /// Record that a game is active
    pub fn record_active_game(&mut self, game_id: &str) {
        if self.session_start.is_none() {
            self.session_start = Some(Utc::now());
        }
        self.active_game_ids.insert(game_id.to_string());
    }

    /// Record a trade in the current session
    pub fn record_trade(&mut self, pnl: Option<f64>) {
        self.session_trades += 1;
        if let Some(p) = pnl {
            self.session_pnl += p;
        }
    }

    /// Check if all previously active games have completed
    /// Returns session stats if session just ended
    pub fn check_session_end(&mut self, current_active_ids: &[String]) -> Option<SessionSummary> {
        if self.active_game_ids.is_empty() {
            return None;
        }

        let current_set: HashSet<_> = current_active_ids.iter().cloned().collect();

        // Check if all our tracked games are now complete
        let all_complete = self.active_game_ids.iter().all(|id| !current_set.contains(id));

        if all_complete && self.session_trades > 0 {
            let summary = SessionSummary {
                games_count: self.active_game_ids.len() as u64,
                trades_count: self.session_trades,
                total_pnl: self.session_pnl,
                duration: self.session_start.map(|s| Utc::now().signed_duration_since(s)),
            };

            // Reset session
            self.active_game_ids.clear();
            self.session_start = None;
            self.session_trades = 0;
            self.session_pnl = 0.0;

            return Some(summary);
        }

        // Update active games set with current games
        for id in current_active_ids {
            self.active_game_ids.insert(id.clone());
        }

        None
    }

    /// Get current session stats
    pub fn session_stats(&self) -> (u64, u64, f64) {
        (
            self.active_game_ids.len() as u64,
            self.session_trades,
            self.session_pnl,
        )
    }
}

/// Summary of a completed trading session
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub games_count: u64,
    pub trades_count: u64,
    pub total_pnl: f64,
    pub duration: Option<chrono::Duration>,
}

/// Query game counts with freshness validation
pub async fn get_game_counts(
    pool: &PgPool,
    freshness_mins: u64,
    imminent_hours: u64,
) -> Result<GameCounts> {
    // Query active games with freshness check
    let active_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM games
        WHERE status IN ('in_progress', 'halftime', 'end_period')
          AND updated_at > NOW() - make_interval(mins => $1::integer)
        "#,
    )
    .bind(freshness_mins as i32)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Query imminent games (starting within window)
    let imminent_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM games
        WHERE (status IS NULL OR status IN ('scheduled', 'status_scheduled', 'pregame'))
          AND scheduled_time > NOW()
          AND scheduled_time < NOW() + make_interval(hours => $1::integer)
        "#,
    )
    .bind(imminent_hours as i32)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Query all upcoming games today (for context)
    let upcoming_today: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM games
        WHERE (status IS NULL OR status IN ('scheduled', 'status_scheduled', 'pregame'))
          AND scheduled_time > NOW()
          AND scheduled_time < NOW() + INTERVAL '24 hours'
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(GameCounts {
        active: active_count as u64,
        imminent: imminent_count as u64,
        upcoming_today: upcoming_today as u64,
    })
}

/// Get list of currently active game IDs (for session tracking)
pub async fn get_active_game_ids(pool: &PgPool, freshness_mins: u64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT game_id
        FROM games
        WHERE status IN ('in_progress', 'halftime', 'end_period')
          AND updated_at > NOW() - make_interval(mins => $1::integer)
        "#,
    )
    .bind(freshness_mins as i32)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Check for stale games that might need cleanup notification
pub async fn get_stale_game_count(pool: &PgPool, stale_threshold_mins: u64) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM games
        WHERE status IN ('in_progress', 'halftime', 'end_period')
          AND updated_at < NOW() - make_interval(mins => $1::integer)
        "#,
    )
    .bind(stale_threshold_mins as i32)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    if count > 0 {
        warn!(
            "Found {} stale games (not updated in {}+ minutes)",
            count, stale_threshold_mins
        );
    }

    Ok(count as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_tracker_empty() {
        let tracker = GameSessionTracker::new();
        assert_eq!(tracker.session_stats(), (0, 0, 0.0));
    }

    #[test]
    fn test_session_tracker_records_games() {
        let mut tracker = GameSessionTracker::new();
        tracker.record_active_game("game1");
        tracker.record_active_game("game2");
        tracker.record_trade(Some(10.0));
        tracker.record_trade(Some(-5.0));

        let (games, trades, pnl) = tracker.session_stats();
        assert_eq!(games, 2);
        assert_eq!(trades, 2);
        assert!((pnl - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_session_end_detection() {
        let mut tracker = GameSessionTracker::new();
        tracker.record_active_game("game1");
        tracker.record_trade(Some(25.0));

        // Games still active
        let result = tracker.check_session_end(&["game1".to_string()]);
        assert!(result.is_none());

        // Game completed
        let result = tracker.check_session_end(&[]);
        assert!(result.is_some());

        let summary = result.unwrap();
        assert_eq!(summary.games_count, 1);
        assert_eq!(summary.trades_count, 1);
        assert!((summary.total_pnl - 25.0).abs() < 0.001);
    }
}
