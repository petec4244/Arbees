use anyhow::{anyhow, Context, Result};
use chrono::NaiveTime;
use chrono_tz::Tz;
use std::env;
use std::str::FromStr;
use std::time::Duration;

use arbees_rust_core::models::NotificationPriority;

/// Notification mode controls the verbosity and behavior of notifications
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationMode {
    /// Adaptive behavior based on context
    Smart,
    /// Errors + daily digest only
    Minimal,
    /// All events immediately (legacy behavior)
    Verbose,
    /// Errors only, no summaries
    Silent,
}

impl Default for NotificationMode {
    fn default() -> Self {
        Self::Smart
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub redis_url: String,

    pub signal_api_base_url: String,
    pub signal_sender_number: String,
    pub signal_recipients: Vec<String>,

    pub quiet_hours_enabled: bool,
    pub quiet_hours_start: NaiveTime,
    pub quiet_hours_end: NaiveTime,
    pub quiet_hours_timezone: Tz,
    pub quiet_hours_min_priority: NotificationPriority,

    pub rate_limit_max_per_minute: usize,
    pub rate_limit_window: Duration,
    pub rate_limit_bypass_critical: bool,

    // Adaptive scheduling
    pub notification_mode: NotificationMode,
    pub summary_interval_active_mins: u64,    // During active trading
    pub summary_interval_games_mins: u64,     // Games in progress, no trades
    pub summary_interval_idle_mins: u64,      // No games happening
    pub upcoming_games_window_hours: u64,     // Window for "imminent" games
    pub game_freshness_mins: u64,             // Max age for "active" game status

    // Threshold notifications
    pub pnl_threshold_notify: f64,            // Notify when PnL crosses this
    pub trade_burst_threshold: u64,           // Notify after N trades in window
    pub trade_burst_window_mins: u64,         // Window for burst detection
    pub win_streak_threshold: u64,            // Notify on win streak

    // End-of-session
    pub session_digest_enabled: bool,         // Send digest when games complete
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let redis_url =
            env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

        let signal_api_base_url = env::var("SIGNAL_API_BASE_URL")
            .unwrap_or_else(|_| "http://signal-cli-rest-api:8080".to_string());

        let signal_sender_number = env::var("SIGNAL_SENDER_NUMBER")
            .context("SIGNAL_SENDER_NUMBER must be set (international format, e.g. +15551234567)")?;

        let signal_recipients = parse_csv_env("SIGNAL_RECIPIENTS")?;

        let quiet_hours_enabled = parse_bool_env("QUIET_HOURS_ENABLED", true);
        let quiet_hours_start =
            parse_time_env("QUIET_HOURS_START", "22:00").context("QUIET_HOURS_START")?;
        let quiet_hours_end =
            parse_time_env("QUIET_HOURS_END", "07:00").context("QUIET_HOURS_END")?;
        let quiet_hours_timezone_str =
            env::var("QUIET_HOURS_TIMEZONE").unwrap_or_else(|_| "America/New_York".to_string());
        let quiet_hours_timezone = Tz::from_str(&quiet_hours_timezone_str).map_err(|_| {
            anyhow!(
                "Invalid QUIET_HOURS_TIMEZONE: {} (expected IANA tz like America/New_York)",
                quiet_hours_timezone_str
            )
        })?;

        let quiet_hours_min_priority =
            parse_priority_env("QUIET_HOURS_MIN_PRIORITY", NotificationPriority::Critical)?;

        let rate_limit_max_per_minute =
            parse_usize_env("RATE_LIMIT_MAX_PER_MINUTE", 10).context("RATE_LIMIT_MAX_PER_MINUTE")?;
        let rate_limit_window = Duration::from_secs(60);

        let rate_limit_bypass_critical = parse_bool_env("RATE_LIMIT_BYPASS_CRITICAL", true);

        // Notification mode
        let notification_mode = parse_notification_mode_env("NOTIFICATION_MODE", NotificationMode::Smart)?;

        // Adaptive scheduling intervals (in minutes)
        let summary_interval_active_mins =
            parse_u64_env("SUMMARY_INTERVAL_ACTIVE_MINS", 15).context("SUMMARY_INTERVAL_ACTIVE_MINS")?;
        let summary_interval_games_mins =
            parse_u64_env("SUMMARY_INTERVAL_GAMES_MINS", 30).context("SUMMARY_INTERVAL_GAMES_MINS")?;
        let summary_interval_idle_mins =
            parse_u64_env("SUMMARY_INTERVAL_IDLE_MINS", 240).context("SUMMARY_INTERVAL_IDLE_MINS")?;
        let upcoming_games_window_hours =
            parse_u64_env("UPCOMING_GAMES_WINDOW_HOURS", 2).context("UPCOMING_GAMES_WINDOW_HOURS")?;
        let game_freshness_mins =
            parse_u64_env("GAME_FRESHNESS_MINS", 30).context("GAME_FRESHNESS_MINS")?;

        // Threshold notifications
        let pnl_threshold_notify =
            parse_f64_env("PNL_THRESHOLD_NOTIFY", 50.0).context("PNL_THRESHOLD_NOTIFY")?;
        let trade_burst_threshold =
            parse_u64_env("TRADE_BURST_THRESHOLD", 5).context("TRADE_BURST_THRESHOLD")?;
        let trade_burst_window_mins =
            parse_u64_env("TRADE_BURST_WINDOW_MINS", 10).context("TRADE_BURST_WINDOW_MINS")?;
        let win_streak_threshold =
            parse_u64_env("WIN_STREAK_THRESHOLD", 3).context("WIN_STREAK_THRESHOLD")?;

        // End-of-session
        let session_digest_enabled = parse_bool_env("SESSION_DIGEST_ENABLED", true);

        Ok(Self {
            redis_url,
            signal_api_base_url,
            signal_sender_number,
            signal_recipients,
            quiet_hours_enabled,
            quiet_hours_start,
            quiet_hours_end,
            quiet_hours_timezone,
            quiet_hours_min_priority,
            rate_limit_max_per_minute,
            rate_limit_window,
            rate_limit_bypass_critical,
            notification_mode,
            summary_interval_active_mins,
            summary_interval_games_mins,
            summary_interval_idle_mins,
            upcoming_games_window_hours,
            game_freshness_mins,
            pnl_threshold_notify,
            trade_burst_threshold,
            trade_burst_window_mins,
            win_streak_threshold,
            session_digest_enabled,
        })
    }
}

fn parse_csv_env(key: &str) -> Result<Vec<String>> {
    let raw = env::var(key).with_context(|| format!("{key} must be set (comma-separated)"))?;
    let vals: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if vals.is_empty() {
        return Err(anyhow!("{key} must contain at least one recipient"));
    }
    Ok(vals)
}

fn parse_bool_env(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "y" | "on"))
        .unwrap_or(default)
}

fn parse_time_env(key: &str, default: &str) -> Result<NaiveTime> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    NaiveTime::parse_from_str(&raw, "%H:%M")
        .with_context(|| format!("Invalid {key}: {raw} (expected HH:MM)"))
}

fn parse_usize_env(key: &str, default: usize) -> Result<usize> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    raw.parse::<usize>()
        .with_context(|| format!("Invalid {key}: {raw} (expected integer)"))
}

fn parse_priority_env(key: &str, default: NotificationPriority) -> Result<NotificationPriority> {
    let raw = env::var(key).unwrap_or_else(|_| match default {
        NotificationPriority::Info => "INFO",
        NotificationPriority::Warning => "WARNING",
        NotificationPriority::Error => "ERROR",
        NotificationPriority::Critical => "CRITICAL",
    }
    .to_string());

    match raw.trim().to_uppercase().as_str() {
        "INFO" => Ok(NotificationPriority::Info),
        "WARNING" => Ok(NotificationPriority::Warning),
        "ERROR" => Ok(NotificationPriority::Error),
        "CRITICAL" => Ok(NotificationPriority::Critical),
        other => Err(anyhow!("Invalid {key}: {other} (expected INFO|WARNING|ERROR|CRITICAL)")),
    }
}

fn parse_u64_env(key: &str, default: u64) -> Result<u64> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    raw.parse::<u64>()
        .with_context(|| format!("Invalid {key}: {raw} (expected integer)"))
}

fn parse_f64_env(key: &str, default: f64) -> Result<f64> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    raw.parse::<f64>()
        .with_context(|| format!("Invalid {key}: {raw} (expected number)"))
}

fn parse_notification_mode_env(key: &str, default: NotificationMode) -> Result<NotificationMode> {
    let default_str = match default {
        NotificationMode::Smart => "SMART",
        NotificationMode::Minimal => "MINIMAL",
        NotificationMode::Verbose => "VERBOSE",
        NotificationMode::Silent => "SILENT",
    };
    let raw = env::var(key).unwrap_or_else(|_| default_str.to_string());

    match raw.trim().to_uppercase().as_str() {
        "SMART" => Ok(NotificationMode::Smart),
        "MINIMAL" => Ok(NotificationMode::Minimal),
        "VERBOSE" => Ok(NotificationMode::Verbose),
        "SILENT" => Ok(NotificationMode::Silent),
        other => Err(anyhow!("Invalid {key}: {other} (expected SMART|MINIMAL|VERBOSE|SILENT)")),
    }
}

