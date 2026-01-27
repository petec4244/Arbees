//! Threshold-based notification triggers
//!
//! Monitors trading activity and triggers immediate notifications when:
//! - PnL crosses a threshold (positive or negative)
//! - Trade burst detected (many trades in short window)
//! - Win/loss streak reaches threshold

use crate::config::Config;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;

/// Types of threshold alerts
#[derive(Debug, Clone, PartialEq)]
pub enum ThresholdAlert {
    /// PnL crossed the threshold (positive)
    PnLPositive { amount: f64, threshold: f64 },
    /// PnL crossed the threshold (negative)
    PnLNegative { amount: f64, threshold: f64 },
    /// Trade burst detected
    TradeBurst { count: u64, window_mins: u64 },
    /// Win streak achieved
    WinStreak { count: u64 },
    /// Loss streak (warning)
    LossStreak { count: u64 },
}

impl ThresholdAlert {
    pub fn format_message(&self) -> String {
        match self {
            Self::PnLPositive { amount, threshold } => {
                format!("🎯 PnL milestone: +${:.2} (crossed +${:.0} threshold)", amount, threshold)
            }
            Self::PnLNegative { amount, threshold } => {
                format!("⚠️ PnL alert: ${:.2} (crossed -${:.0} threshold)", amount, threshold)
            }
            Self::TradeBurst { count, window_mins } => {
                format!("📈 Trade burst: {} trades in {} minutes", count, window_mins)
            }
            Self::WinStreak { count } => {
                format!("🔥 Win streak: {} consecutive wins!", count)
            }
            Self::LossStreak { count } => {
                format!("📉 Loss streak: {} consecutive losses", count)
            }
        }
    }
}

/// Tracks trading metrics for threshold detection
#[derive(Debug)]
pub struct ThresholdTracker {
    config: Config,

    // PnL tracking
    session_pnl: f64,
    last_pnl_alert_positive: Option<f64>,
    last_pnl_alert_negative: Option<f64>,

    // Trade burst tracking
    recent_trades: VecDeque<DateTime<Utc>>,
    last_burst_alert: Option<DateTime<Utc>>,

    // Streak tracking
    current_streak: i32,  // Positive = wins, negative = losses
    last_streak_alert: i32,
}

impl ThresholdTracker {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            session_pnl: 0.0,
            last_pnl_alert_positive: None,
            last_pnl_alert_negative: None,
            recent_trades: VecDeque::new(),
            last_burst_alert: None,
            current_streak: 0,
            last_streak_alert: 0,
        }
    }

    /// Record a trade and check for threshold alerts
    /// Returns any alerts that should be sent immediately
    pub fn record_trade(&mut self, pnl: Option<f64>, is_win: Option<bool>) -> Vec<ThresholdAlert> {
        let mut alerts = Vec::new();
        let now = Utc::now();

        // Track trade timing for burst detection
        self.recent_trades.push_back(now);
        self.cleanup_old_trades();

        // Check for trade burst
        if let Some(alert) = self.check_trade_burst() {
            alerts.push(alert);
        }

        // Track PnL
        if let Some(p) = pnl {
            self.session_pnl += p;

            // Check PnL thresholds
            if let Some(alert) = self.check_pnl_threshold() {
                alerts.push(alert);
            }
        }

        // Track streaks
        if let Some(win) = is_win {
            self.update_streak(win);

            // Check streak threshold
            if let Some(alert) = self.check_streak_threshold() {
                alerts.push(alert);
            }
        }

        alerts
    }

    /// Reset session tracking (e.g., at start of new day)
    pub fn reset_session(&mut self) {
        self.session_pnl = 0.0;
        self.last_pnl_alert_positive = None;
        self.last_pnl_alert_negative = None;
        self.current_streak = 0;
        self.last_streak_alert = 0;
        self.recent_trades.clear();
        self.last_burst_alert = None;
    }

    /// Get current session PnL
    pub fn session_pnl(&self) -> f64 {
        self.session_pnl
    }

    /// Get current streak (positive = wins, negative = losses)
    pub fn current_streak(&self) -> i32 {
        self.current_streak
    }

    fn cleanup_old_trades(&mut self) {
        let window = chrono::Duration::minutes(self.config.trade_burst_window_mins as i64);
        let cutoff = Utc::now() - window;

        while let Some(front) = self.recent_trades.front() {
            if *front < cutoff {
                self.recent_trades.pop_front();
            } else {
                break;
            }
        }
    }

    fn check_trade_burst(&mut self) -> Option<ThresholdAlert> {
        let count = self.recent_trades.len() as u64;

        if count >= self.config.trade_burst_threshold {
            // Don't alert again within the same window
            let now = Utc::now();
            let window = chrono::Duration::minutes(self.config.trade_burst_window_mins as i64);

            if let Some(last) = self.last_burst_alert {
                if now - last < window {
                    return None;
                }
            }

            self.last_burst_alert = Some(now);
            return Some(ThresholdAlert::TradeBurst {
                count,
                window_mins: self.config.trade_burst_window_mins,
            });
        }

        None
    }

    fn check_pnl_threshold(&mut self) -> Option<ThresholdAlert> {
        let threshold = self.config.pnl_threshold_notify;

        // Check positive threshold
        if self.session_pnl >= threshold {
            let alert_level = (self.session_pnl / threshold).floor() * threshold;

            if self.last_pnl_alert_positive.map_or(true, |last| alert_level > last) {
                self.last_pnl_alert_positive = Some(alert_level);
                return Some(ThresholdAlert::PnLPositive {
                    amount: self.session_pnl,
                    threshold,
                });
            }
        }

        // Check negative threshold
        if self.session_pnl <= -threshold {
            let alert_level = (self.session_pnl / threshold).ceil() * threshold;

            if self.last_pnl_alert_negative.map_or(true, |last| alert_level < last) {
                self.last_pnl_alert_negative = Some(alert_level);
                return Some(ThresholdAlert::PnLNegative {
                    amount: self.session_pnl,
                    threshold,
                });
            }
        }

        None
    }

    fn update_streak(&mut self, is_win: bool) {
        if is_win {
            if self.current_streak > 0 {
                self.current_streak += 1;
            } else {
                self.current_streak = 1;
            }
        } else {
            if self.current_streak < 0 {
                self.current_streak -= 1;
            } else {
                self.current_streak = -1;
            }
        }
    }

    fn check_streak_threshold(&mut self) -> Option<ThresholdAlert> {
        let threshold = self.config.win_streak_threshold as i32;

        // Check win streak
        if self.current_streak >= threshold && self.current_streak > self.last_streak_alert {
            self.last_streak_alert = self.current_streak;
            return Some(ThresholdAlert::WinStreak {
                count: self.current_streak as u64,
            });
        }

        // Check loss streak (alert at same threshold)
        if self.current_streak <= -threshold && self.current_streak < -self.last_streak_alert.abs() {
            self.last_streak_alert = self.current_streak;
            return Some(ThresholdAlert::LossStreak {
                count: self.current_streak.unsigned_abs() as u64,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
            notification_mode: crate::config::NotificationMode::Smart,
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
    fn test_pnl_positive_threshold() {
        let mut tracker = ThresholdTracker::new(test_config());

        // Below threshold - no alert
        let alerts = tracker.record_trade(Some(30.0), Some(true));
        assert!(alerts.is_empty());

        // Cross threshold - alert
        let alerts = tracker.record_trade(Some(25.0), Some(true));
        assert_eq!(alerts.len(), 1);
        assert!(matches!(alerts[0], ThresholdAlert::PnLPositive { .. }));

        // Still above threshold but same level - no alert
        let alerts = tracker.record_trade(Some(10.0), Some(true));
        assert!(alerts.is_empty());

        // Cross next threshold level - alert
        let alerts = tracker.record_trade(Some(40.0), Some(true));
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn test_win_streak() {
        let mut tracker = ThresholdTracker::new(test_config());

        // Build up streak
        tracker.record_trade(Some(10.0), Some(true));
        tracker.record_trade(Some(10.0), Some(true));

        // Third win crosses threshold
        let alerts = tracker.record_trade(Some(10.0), Some(true));
        assert_eq!(alerts.len(), 1);
        assert!(matches!(alerts[0], ThresholdAlert::WinStreak { count: 3 }));

        // Loss breaks streak
        tracker.record_trade(Some(-10.0), Some(false));
        assert_eq!(tracker.current_streak(), -1);
    }

    #[test]
    fn test_session_reset() {
        let mut tracker = ThresholdTracker::new(test_config());

        tracker.record_trade(Some(100.0), Some(true));
        assert!(tracker.session_pnl() > 0.0);

        tracker.reset_session();
        assert_eq!(tracker.session_pnl(), 0.0);
        assert_eq!(tracker.current_streak(), 0);
    }
}
