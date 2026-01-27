//! Adaptive scheduling for notification summaries
//!
//! Adjusts summary interval based on trading context:
//! - Active trading: more frequent updates
//! - Games in progress: moderate frequency
//! - Idle periods: infrequent health checks

use crate::config::{Config, NotificationMode};
use chrono::{DateTime, Utc};
use std::time::Duration;

/// Trading context for adaptive scheduling decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingContext {
    /// Active trading happening (trades in last N minutes)
    ActiveTrading,
    /// Games in progress but no recent trades
    GamesInProgress,
    /// Games starting soon (within window)
    GamesImminent,
    /// No games, idle period
    Idle,
    /// Quiet hours - no notifications
    QuietHours,
}

impl TradingContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ActiveTrading => "active_trading",
            Self::GamesInProgress => "games_in_progress",
            Self::GamesImminent => "games_imminent",
            Self::Idle => "idle",
            Self::QuietHours => "quiet_hours",
        }
    }
}

/// Scheduler state for adaptive intervals
#[derive(Debug)]
pub struct AdaptiveScheduler {
    config: Config,
    last_trade_time: Option<DateTime<Utc>>,
    current_context: TradingContext,
}

impl AdaptiveScheduler {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            last_trade_time: None,
            current_context: TradingContext::Idle,
        }
    }

    /// Record a trade event to track activity
    pub fn record_trade(&mut self) {
        self.last_trade_time = Some(Utc::now());
    }

    /// Update and return the current trading context
    pub fn update_context(
        &mut self,
        active_games: u64,
        imminent_games: u64,
        is_quiet_hours: bool,
    ) -> TradingContext {
        // Quiet hours takes precedence
        if is_quiet_hours {
            self.current_context = TradingContext::QuietHours;
            return self.current_context;
        }

        // Check if we have recent trading activity
        let has_recent_trades = self.last_trade_time.map_or(false, |t| {
            let age = Utc::now().signed_duration_since(t);
            age.num_minutes() < self.config.trade_burst_window_mins as i64
        });

        self.current_context = if has_recent_trades && active_games > 0 {
            TradingContext::ActiveTrading
        } else if active_games > 0 {
            TradingContext::GamesInProgress
        } else if imminent_games > 0 {
            TradingContext::GamesImminent
        } else {
            TradingContext::Idle
        };

        self.current_context
    }

    /// Get the appropriate summary interval based on current context and mode
    pub fn get_summary_interval(&self) -> Duration {
        // In verbose mode, always use the active trading interval
        if self.config.notification_mode == NotificationMode::Verbose {
            return Duration::from_secs(self.config.summary_interval_active_mins * 60);
        }

        // In minimal mode, always use the idle interval (longest)
        if self.config.notification_mode == NotificationMode::Minimal {
            return Duration::from_secs(self.config.summary_interval_idle_mins * 60);
        }

        // Smart mode: context-aware intervals
        let mins = match self.current_context {
            TradingContext::ActiveTrading => self.config.summary_interval_active_mins,
            TradingContext::GamesInProgress => self.config.summary_interval_games_mins,
            TradingContext::GamesImminent => self.config.summary_interval_games_mins,
            TradingContext::Idle => self.config.summary_interval_idle_mins,
            TradingContext::QuietHours => {
                // Return a long interval; the filter will block anyway
                self.config.summary_interval_idle_mins
            }
        };

        Duration::from_secs(mins * 60)
    }

    /// Get current context
    pub fn current_context(&self) -> TradingContext {
        self.current_context
    }

    /// Check if we should skip the summary based on context
    pub fn should_skip_summary(
        &self,
        trade_count: u64,
        active_games: u64,
        imminent_games: u64,
    ) -> (bool, Option<String>) {
        // In silent mode, always skip summaries
        if self.config.notification_mode == NotificationMode::Silent {
            return (true, Some("silent_mode".to_string()));
        }

        // In minimal mode, only send if there's actual trading activity
        if self.config.notification_mode == NotificationMode::Minimal {
            if trade_count == 0 {
                return (true, Some("minimal_mode_no_trades".to_string()));
            }
        }

        // In smart mode, skip if nothing is happening
        if self.config.notification_mode == NotificationMode::Smart {
            if trade_count == 0 && active_games == 0 && imminent_games == 0 {
                return (true, Some("no_activity".to_string()));
            }
        }

        // Verbose mode: never skip (unless quiet hours, handled elsewhere)
        (false, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            redis_url: "redis://localhost".to_string(),
            signal_api_base_url: "http://localhost".to_string(),
            signal_sender_number: "+1234567890".to_string(),
            signal_recipients: vec!["+0987654321".to_string()],
            quiet_hours_enabled: false,
            quiet_hours_start: chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            quiet_hours_end: chrono::NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
            quiet_hours_timezone: chrono_tz::America::New_York,
            quiet_hours_min_priority: arbees_rust_core::models::NotificationPriority::Critical,
            rate_limit_max_per_minute: 10,
            rate_limit_window: Duration::from_secs(60),
            rate_limit_bypass_critical: true,
            notification_mode: NotificationMode::Smart,
            summary_interval_active_mins: 15,
            summary_interval_games_mins: 30,
            summary_interval_idle_mins: 240,
            upcoming_games_window_hours: 2,
            game_freshness_mins: 30,
            pnl_threshold_notify: 50.0,
            trade_burst_threshold: 5,
            trade_burst_window_mins: 10,
            win_streak_threshold: 3,
            session_digest_enabled: true,
        }
    }

    #[test]
    fn test_context_idle() {
        let mut scheduler = AdaptiveScheduler::new(test_config());
        let ctx = scheduler.update_context(0, 0, false);
        assert_eq!(ctx, TradingContext::Idle);
    }

    #[test]
    fn test_context_games_in_progress() {
        let mut scheduler = AdaptiveScheduler::new(test_config());
        let ctx = scheduler.update_context(3, 0, false);
        assert_eq!(ctx, TradingContext::GamesInProgress);
    }

    #[test]
    fn test_context_quiet_hours() {
        let mut scheduler = AdaptiveScheduler::new(test_config());
        let ctx = scheduler.update_context(5, 2, true);
        assert_eq!(ctx, TradingContext::QuietHours);
    }

    #[test]
    fn test_interval_smart_mode() {
        let scheduler = AdaptiveScheduler::new(test_config());
        // Default context is Idle
        let interval = scheduler.get_summary_interval();
        assert_eq!(interval.as_secs(), 240 * 60); // 4 hours
    }
}
