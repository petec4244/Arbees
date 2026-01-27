use arbees_rust_core::models::{NotificationEvent, NotificationType};
use chrono::{DateTime, Utc};

use crate::game_context::SessionSummary;
use crate::scheduler::TradingContext;

pub fn format_message(event: &NotificationEvent) -> String {
    match event.event_type {
        NotificationType::TradeEntry => format_trade_entry(event),
        NotificationType::TradeExit => format_trade_exit(event),
        NotificationType::RiskRejection => format_risk_rejection(event),
        NotificationType::Error => format_error(event),
    }
}

/// Format timestamp concisely (time only for recent, date+time for older)
fn ts_str(ts: Option<DateTime<Utc>>) -> String {
    let t = ts.unwrap_or_else(Utc::now);
    let now = Utc::now();
    let age = now.signed_duration_since(t);

    // If within last 24 hours, just show time
    if age.num_hours() < 24 {
        t.format("%H:%M:%S").to_string()
    } else {
        t.format("%m-%d %H:%M").to_string()
    }
}

/// Format timestamp as time only (for compact messages)
fn time_only(ts: Option<DateTime<Utc>>) -> String {
    ts.unwrap_or_else(Utc::now).format("%H:%M").to_string()
}

fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn get_f64(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

fn format_trade_entry(event: &NotificationEvent) -> String {
    let d = &event.data;
    let game_id = get_str(d, "game_id").unwrap_or_else(|| "?".to_string());
    let sport = get_str(d, "sport").unwrap_or_else(|| "?".to_string());
    let team = get_str(d, "team")
        .or_else(|| get_str(d, "contract_team"))
        .unwrap_or_else(|| "?".to_string());
    let side = get_str(d, "side").unwrap_or_else(|| "?".to_string());
    let price = get_f64(d, "price")
        .or_else(|| get_f64(d, "entry_price"))
        .unwrap_or(0.0);
    let size = get_f64(d, "size").unwrap_or(0.0);
    let platform = get_str(d, "platform").unwrap_or_else(|| "?".to_string());
    let market_id = get_str(d, "market_id").unwrap_or_else(|| "?".to_string());
    let edge_pct = get_f64(d, "edge_pct");

    let mut out = String::new();
    out.push_str("🟢 TRADE ENTRY\n");
    out.push_str(&format!("{sport} {game_id}\n"));
    out.push_str(&format!("{team} ({side}) @ {price:.3}\n"));
    out.push_str(&format!("size=${size:.2}"));
    if let Some(e) = edge_pct {
        out.push_str(&format!(" edge={e:.1}%"));
    }
    out.push('\n');
    out.push_str(&format!("{platform} {market_id}\n"));
    out.push_str(&format!("ts={}", ts_str(event.ts)));
    out
}

fn format_trade_exit(event: &NotificationEvent) -> String {
    let d = &event.data;
    let game_id = get_str(d, "game_id").unwrap_or_else(|| "?".to_string());
    let sport = get_str(d, "sport").unwrap_or_else(|| "?".to_string());
    let team = get_str(d, "team")
        .or_else(|| get_str(d, "contract_team"))
        .or_else(|| get_str(d, "market_title"))
        .unwrap_or_else(|| "?".to_string());
    let pnl = get_f64(d, "pnl").unwrap_or(0.0);
    let pnl_pct = get_f64(d, "pnl_pct").unwrap_or(0.0);
    let entry_price = get_f64(d, "entry_price").unwrap_or(0.0);
    let exit_price = get_f64(d, "exit_price").unwrap_or(0.0);
    let size = get_f64(d, "size").unwrap_or(0.0);
    let duration_minutes = d.get("duration_minutes").and_then(|x| x.as_i64()).unwrap_or(0);
    let platform = get_str(d, "platform").unwrap_or_else(|| "?".to_string());
    let exit_reason = get_str(d, "exit_reason").unwrap_or_else(|| "".to_string());

    let emoji = if pnl >= 0.0 { "💰" } else { "📉" };
    let sign = if pnl >= 0.0 { "+" } else { "" };

    let mut out = String::new();
    out.push_str(&format!("{emoji} TRADE EXIT\n"));
    out.push_str(&format!("{sport} {game_id}\n"));
    out.push_str(&format!("{team}\n"));
    out.push_str(&format!("pnl={sign}{pnl:.2} ({pnl_pct:+.1}%)\n"));
    out.push_str(&format!(
        "entry={entry_price:.3} exit={exit_price:.3} size=${size:.2}\n"
    ));
    if duration_minutes > 0 {
        out.push_str(&format!("dur={duration_minutes}m "));
    }
    out.push_str(&platform);
    if !exit_reason.is_empty() {
        out.push_str(&format!(" reason={exit_reason}"));
    }
    out.push('\n');
    out.push_str(&format!("ts={}", ts_str(event.ts)));
    out
}

fn format_risk_rejection(event: &NotificationEvent) -> String {
    let d = &event.data;
    let game_id = get_str(d, "game_id").unwrap_or_else(|| "?".to_string());
    let team = get_str(d, "team").unwrap_or_else(|| "?".to_string());
    let edge_pct = get_f64(d, "edge_pct").unwrap_or(0.0);
    let size = get_f64(d, "size").unwrap_or(0.0);
    let reason = get_str(d, "rejection_reason").unwrap_or_else(|| "?".to_string());

    let mut out = String::new();
    out.push_str("🛑 RISK REJECTION\n");
    out.push_str(&format!("{game_id} {team}\n"));
    out.push_str(&format!("edge={edge_pct:.1}% size=${size:.2}\n"));
    out.push_str(&format!("reason={reason}\n"));

    // Optional exposure context if provided
    if let Some(balance) = get_f64(d, "balance") {
        out.push_str(&format!("balance=${balance:.2} "));
    }
    if let Some(daily_loss) = get_f64(d, "daily_loss") {
        out.push_str(&format!("daily_loss=${daily_loss:.2} "));
    }
    if let Some(game_exp) = get_f64(d, "game_exposure") {
        out.push_str(&format!("game_exp=${game_exp:.2} "));
    }
    if let Some(sport_exp) = get_f64(d, "sport_exposure") {
        out.push_str(&format!("sport_exp=${sport_exp:.2} "));
    }
    out.push('\n');
    out.push_str(&format!("ts={}", ts_str(event.ts)));
    out
}

fn format_error(event: &NotificationEvent) -> String {
    let d = &event.data;
    let service = get_str(d, "service").unwrap_or_else(|| "unknown".to_string());
    let message = get_str(d, "message").unwrap_or_else(|| "?".to_string());
    let request_id = get_str(d, "request_id").unwrap_or_else(|| "".to_string());

    let mut out = String::new();
    out.push_str("⚠️ ERROR\n");
    out.push_str(&format!("service={service}\n"));
    if !request_id.is_empty() {
        out.push_str(&format!("request_id={request_id}\n"));
    }
    out.push_str(&message);
    out.push('\n');
    out.push_str(&format!("ts={}", ts_str(event.ts)));
    out
}

/// Summary data for formatting
#[derive(Debug, Clone, Default)]
pub struct SummaryData {
    pub trade_entries: u64,
    pub trade_exits: u64,
    pub risk_rejections: u64,
    pub total_entry_size: f64,
    pub total_exit_pnl: f64,
    pub last_update: Option<DateTime<Utc>>,
    pub active_games: u64,
    pub imminent_games: u64,
    pub upcoming_today: u64,
    pub context: Option<TradingContext>,
    pub interval_mins: u64,
    pub session_pnl: f64,
    pub win_streak: i32,
}

pub fn format_summary(data: &SummaryData) -> String {
    let mut out = String::new();

    // Header with context-aware interval
    let interval_desc = match data.interval_mins {
        15 => "15m",
        30 => "30m",
        60 => "1h",
        240 => "4h",
        _ => "periodic",
    };
    out.push_str(&format!("📊 STATUS ({})\n", interval_desc));

    // Context indicator (compact)
    if let Some(ctx) = &data.context {
        let ctx_emoji = match ctx {
            TradingContext::ActiveTrading => "🔥",
            TradingContext::GamesInProgress => "🎮",
            TradingContext::GamesImminent => "⏰",
            TradingContext::Idle => "💤",
            TradingContext::QuietHours => "🌙",
        };
        out.push_str(&format!("{} {}\n", ctx_emoji, ctx.as_str()));
    }

    // Game counts (compact single line)
    if data.active_games > 0 || data.imminent_games > 0 {
        out.push_str(&format!(
            "Games: {} live",
            data.active_games
        ));
        if data.imminent_games > 0 {
            out.push_str(&format!(", {} soon", data.imminent_games));
        }
        out.push('\n');
    } else if data.upcoming_today > 0 {
        out.push_str(&format!("📅 {} games today\n", data.upcoming_today));
    } else {
        out.push_str("No games scheduled\n");
    }

    // Trading activity (compact)
    let total_trades = data.trade_entries + data.trade_exits;
    if total_trades > 0 {
        out.push_str(&format!(
            "Trades: {}↗ {}↘",
            data.trade_entries, data.trade_exits
        ));
        if data.total_exit_pnl != 0.0 {
            let sign = if data.total_exit_pnl >= 0.0 { "+" } else { "" };
            out.push_str(&format!(" {sign}${:.2}", data.total_exit_pnl));
        }
        out.push('\n');
    }

    // Session stats (if meaningful)
    if data.session_pnl.abs() > 0.01 {
        let sign = if data.session_pnl >= 0.0 { "+" } else { "" };
        out.push_str(&format!("Session: {sign}${:.2}", data.session_pnl));
        if data.win_streak != 0 {
            let streak_emoji = if data.win_streak > 0 { "🔥" } else { "📉" };
            out.push_str(&format!(" {}x{}", streak_emoji, data.win_streak.abs()));
        }
        out.push('\n');
    }

    // Risk rejections (only if any)
    if data.risk_rejections > 0 {
        out.push_str(&format!("🛑 {} blocked\n", data.risk_rejections));
    }

    // Timestamp (compact)
    if let Some(ts) = data.last_update {
        out.push_str(&format!("@ {}", time_only(Some(ts))));
    } else {
        out.push_str(&format!("@ {}", time_only(None)));
    }

    out
}

/// Legacy format_summary for backwards compatibility
pub fn format_summary_legacy(
    trade_entries: u64,
    trade_exits: u64,
    risk_rejections: u64,
    total_entry_size: f64,
    total_exit_pnl: f64,
    last_update: Option<DateTime<Utc>>,
    active_games: u64,
    upcoming_games: u64,
) -> String {
    let data = SummaryData {
        trade_entries,
        trade_exits,
        risk_rejections,
        total_entry_size,
        total_exit_pnl,
        last_update,
        active_games,
        imminent_games: 0,
        upcoming_today: upcoming_games,
        context: None,
        interval_mins: 30,
        session_pnl: total_exit_pnl,
        win_streak: 0,
    };
    format_summary(&data)
}

/// Format end-of-session digest
pub fn format_session_digest(summary: &SessionSummary) -> String {
    let mut out = String::new();

    out.push_str("🏁 SESSION COMPLETE\n");

    // Duration
    if let Some(dur) = summary.duration {
        let hours = dur.num_hours();
        let mins = dur.num_minutes() % 60;
        if hours > 0 {
            out.push_str(&format!("Duration: {}h {}m\n", hours, mins));
        } else {
            out.push_str(&format!("Duration: {}m\n", mins));
        }
    }

    // Games
    out.push_str(&format!("Games: {}\n", summary.games_count));

    // Trades and PnL
    out.push_str(&format!("Trades: {}\n", summary.trades_count));

    let sign = if summary.total_pnl >= 0.0 { "+" } else { "" };
    let emoji = if summary.total_pnl >= 0.0 { "💰" } else { "📉" };
    out.push_str(&format!("{} PnL: {sign}${:.2}\n", emoji, summary.total_pnl));

    // Win rate if calculable
    if summary.trades_count > 0 {
        // Note: We don't have win count in SessionSummary, so skip win rate
        // This could be enhanced later
    }

    out.push_str(&format!("@ {}", time_only(None)));

    out
}
