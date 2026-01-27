use crate::config::{Config, NotificationMode};
use arbees_rust_core::models::NotificationPriority;
use chrono::Utc;
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Debug)]
pub struct NotificationFilter {
    cfg: Config,
    sent_timestamps: VecDeque<Instant>,
}

impl NotificationFilter {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            sent_timestamps: VecDeque::new(),
        }
    }

    /// Check if we should send a notification based on priority, quiet hours, and rate limits
    pub fn should_notify(&mut self, priority: NotificationPriority) -> (bool, Option<String>) {
        // Silent mode only allows critical errors
        if self.cfg.notification_mode == NotificationMode::Silent {
            if priority != NotificationPriority::Critical {
                return (false, Some("silent_mode".to_string()));
            }
        }

        // Quiet hours check
        if self.cfg.quiet_hours_enabled && self.is_in_quiet_hours() {
            if priority.rank() < self.cfg.quiet_hours_min_priority.rank() {
                return (
                    false,
                    Some(format!(
                        "quiet_hours(priority<{:?})",
                        self.cfg.quiet_hours_min_priority
                    )),
                );
            }
        }

        // Rate limiting
        if self.cfg.rate_limit_bypass_critical && priority == NotificationPriority::Critical {
            return (true, None);
        }

        let (ok, reason) = self.check_rate_limit();
        if !ok {
            return (false, reason);
        }

        (true, None)
    }

    /// Check if we should send a summary notification
    /// Respects quiet hours and notification mode
    pub fn should_send_summary(&self) -> (bool, Option<String>) {
        // Silent mode never sends summaries
        if self.cfg.notification_mode == NotificationMode::Silent {
            return (false, Some("silent_mode".to_string()));
        }

        // Quiet hours check for summaries
        if self.cfg.quiet_hours_enabled && self.is_in_quiet_hours() {
            return (false, Some("quiet_hours".to_string()));
        }

        (true, None)
    }

    /// Public method to check if currently in quiet hours
    pub fn is_in_quiet_hours(&self) -> bool {
        if !self.cfg.quiet_hours_enabled {
            return false;
        }

        let tz = self.cfg.quiet_hours_timezone;
        let now_local = Utc::now().with_timezone(&tz).time();

        let start = self.cfg.quiet_hours_start;
        let end = self.cfg.quiet_hours_end;

        // Handle overnight quiet hours: start > end means wrap midnight.
        if start > end {
            now_local >= start || now_local < end
        } else {
            now_local >= start && now_local < end
        }
    }

    /// Get the config reference
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn check_rate_limit(&mut self) -> (bool, Option<String>) {
        let now = Instant::now();

        // Drop timestamps outside window.
        while let Some(front) = self.sent_timestamps.front() {
            if now.duration_since(*front) > self.cfg.rate_limit_window {
                self.sent_timestamps.pop_front();
            } else {
                break;
            }
        }

        if self.sent_timestamps.len() >= self.cfg.rate_limit_max_per_minute {
            return (false, Some("rate_limited".to_string()));
        }

        self.sent_timestamps.push_back(now);
        (true, None)
    }
}

