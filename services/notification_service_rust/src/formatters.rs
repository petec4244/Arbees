use arbees_rust_core::models::{NotificationEvent, NotificationType};
use chrono::{DateTime, Utc};

pub fn format_message(event: &NotificationEvent) -> String {
    match event.event_type {
        NotificationType::TradeEntry => format_trade_entry(event),
        NotificationType::TradeExit => format_trade_exit(event),
        NotificationType::RiskRejection => format_risk_rejection(event),
        NotificationType::Error => format_error(event),
    }
}

fn ts_str(ts: Option<DateTime<Utc>>) -> String {
    ts.unwrap_or_else(Utc::now).to_rfc3339()
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

