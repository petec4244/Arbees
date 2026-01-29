mod config;
mod filters;
mod formatters;
mod game_context;
mod scheduler;
mod signal_client;
mod thresholds;

use anyhow::{Context, Result};
use arbees_rust_core::models::{channels, NotificationEvent, NotificationType};
use arbees_rust_core::redis::RedisBus;
use chrono::Utc;
use config::Config;
use dotenv::dotenv;
use filters::NotificationFilter;
use formatters::SummaryData;
use futures_util::StreamExt;
use game_context::{get_active_game_ids, get_game_counts, GameSessionTracker};
use log::{error, info, warn};
use scheduler::AdaptiveScheduler;
use signal_client::SignalClient;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Instant;
use thresholds::ThresholdTracker;
use tokio::sync::RwLock;

// ============================================================================
// Orchestrator Notification Rate Limiter
// ============================================================================

/// Rate limiter for orchestrator notifications to prevent notification spam
/// when services are flapping or having issues.
struct OrchestratorRateLimiter {
    /// Maps "service_type:notification_type" -> last sent time
    last_sent: HashMap<String, Instant>,
    /// Cooldown between identical notifications (5 minutes)
    cooldown_secs: u64,
}

impl OrchestratorRateLimiter {
    fn new(cooldown_secs: u64) -> Self {
        Self {
            last_sent: HashMap::new(),
            cooldown_secs,
        }
    }

    /// Check if we should send this notification type for this service.
    /// Returns true if enough time has passed since the last notification.
    fn should_send(&mut self, service_type: &str, notif_type: &str) -> bool {
        // Critical notifications always go through with shorter cooldown
        let cooldown = match notif_type {
            "service_dead" | "circuit_breaker_opened" => 60, // 1 minute for critical
            _ => self.cooldown_secs, // Standard cooldown for others
        };

        let key = format!("{}:{}", service_type, notif_type);
        let now = Instant::now();

        if let Some(last) = self.last_sent.get(&key) {
            if now.duration_since(*last).as_secs() < cooldown {
                return false;
            }
        }

        self.last_sent.insert(key, now);
        true
    }
}

// ============================================================================
// Orchestrator Notification Handler
// ============================================================================

/// Handle fault tolerance notifications from the orchestrator
async fn handle_orchestrator_notification(
    channel: &str,
    payload_bytes: &[u8],
    signal: &SignalClient,
    metrics: &Arc<RwLock<Metrics>>,
    rate_limiter: &Arc<RwLock<OrchestratorRateLimiter>>,
) {
    // Parse as generic JSON
    let notification: serde_json::Value = match serde_json::from_slice(payload_bytes) {
        Ok(n) => n,
        Err(e) => {
            warn!("Failed to parse orchestrator notification: {}", e);
            let mut m = metrics.write().await;
            m.parse_errors += 1;
            return;
        }
    };

    // Extract common fields
    let notif_type = notification
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let instance_id = notification
        .get("instance_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let service_type = notification
        .get("service_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Format message based on notification type
    let message = match notif_type {
        "service_restarted" => {
            let process_id = notification
                .get("process_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!(
                "🔄 Service Restart Detected\nService: {}\nInstance: {}\nProcess: {}",
                service_type, instance_id, &process_id[..8]
            )
        }
        "service_resync_complete" => {
            let game_count = notification
                .get("games_resynced")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "✅ Service Resync Complete\nService: {}\nInstance: {}\nGames Resynced: {}",
                service_type, instance_id, game_count
            )
        }
        "service_degraded" => {
            let failing_checks = notification
                .get("failing_checks")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "unknown".to_string());
            format!(
                "⚠️ Service Degraded\nService: {}\nInstance: {}\nFailing Checks: {}",
                service_type, instance_id, failing_checks
            )
        }
        "service_recovered" => {
            format!(
                "✅ Service Recovered\nService: {}\nInstance: {}",
                service_type, instance_id
            )
        }
        "circuit_breaker_opened" => {
            let failure_count = notification
                .get("failure_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "🚨 Circuit Breaker Opened\nService: {}\nInstance: {}\nFailures: {}",
                service_type, instance_id, failure_count
            )
        }
        "circuit_breaker_closed" => {
            format!(
                "✅ Circuit Breaker Closed\nService: {}\nInstance: {}",
                service_type, instance_id
            )
        }
        "service_dead" => {
            let last_heartbeat = notification
                .get("last_heartbeat")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!(
                "💀 Service Dead\nService: {}\nInstance: {}\nLast Heartbeat: {}",
                service_type, instance_id, last_heartbeat
            )
        }
        _ => {
            format!(
                "📊 Orchestrator Notification\nType: {}\nService: {}\nInstance: {}",
                notif_type, service_type, instance_id
            )
        }
    };

    // Check rate limiter before sending
    let should_send = {
        let mut limiter = rate_limiter.write().await;
        limiter.should_send(service_type, notif_type)
    };

    if !should_send {
        info!(
            "Rate-limited orchestrator notification: type={} service={} (cooldown active)",
            notif_type, service_type
        );
        let mut m = metrics.write().await;
        m.filtered += 1;
        return;
    }

    // Send notification
    if let Err(e) = signal.send(&message).await {
        error!("Failed to send orchestrator notification: {}", e);
        let mut m = metrics.write().await;
        m.send_errors += 1;
    } else {
        info!(
            "Sent orchestrator notification: type={} service={}",
            notif_type, service_type
        );
        let mut m = metrics.write().await;
        m.sent += 1;
    }
}

// ============================================================================
// Heartbeat
// ============================================================================

#[derive(Debug, serde::Serialize)]
struct Heartbeat {
    service: String,
    instance_id: String,
    status: String,
    timestamp: String,
    checks: HashMap<String, bool>,
    metrics: HashMap<String, f64>,
}

#[derive(Debug, Default)]
struct Metrics {
    received: u64,
    sent: u64,
    filtered: u64,
    parse_errors: u64,
    send_errors: u64,
}

#[derive(Debug, Default)]
struct BatchedEvents {
    trade_entries: u64,
    trade_exits: u64,
    risk_rejections: u64,
    total_entry_size: f64,
    total_exit_pnl: f64,
    last_update: Option<chrono::DateTime<Utc>>,
    wins: u64,
    losses: u64,
}

/// Shared state for adaptive scheduling
struct SharedState {
    metrics: Metrics,
    batched: BatchedEvents,
    scheduler: AdaptiveScheduler,
    thresholds: ThresholdTracker,
    session_tracker: GameSessionTracker,
    filter: NotificationFilter,
}

async fn heartbeat_loop(redis: RedisBus, instance_id: String, metrics: Arc<RwLock<Metrics>>) -> Result<()> {
    info!("Heartbeat loop started for {}", instance_id);
    loop {
        let m = metrics.read().await;
        let mut checks = HashMap::new();
        checks.insert("redis_ok".to_string(), true);

        let mut values = HashMap::new();
        values.insert("received".to_string(), m.received as f64);
        values.insert("sent".to_string(), m.sent as f64);
        values.insert("filtered".to_string(), m.filtered as f64);
        values.insert("parse_errors".to_string(), m.parse_errors as f64);
        values.insert("send_errors".to_string(), m.send_errors as f64);
        drop(m);

        let hb = Heartbeat {
            service: "notification_service_rust".to_string(),
            instance_id: instance_id.clone(),
            status: "healthy".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            checks,
            metrics: values,
        };

        if let Err(e) = redis.publish(channels::HEALTH_HEARTBEATS, &hb).await {
            warn!("Failed to publish heartbeat: {}", e);
        }

        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting Rust Notification Service (modernized)...");

    let cfg = Config::from_env()?;
    info!(
        "Config: mode={:?} quiet_hours={} intervals={}m/{}m/{}m",
        cfg.notification_mode,
        cfg.quiet_hours_enabled,
        cfg.summary_interval_active_mins,
        cfg.summary_interval_games_mins,
        cfg.summary_interval_idle_mins,
    );

    let redis = RedisBus::new().await?;
    info!("Connected to Redis");

    // Database connection for game counts
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://arbees:arbees@localhost:5432/arbees".to_string());
    let db_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("Failed to connect to database")?;
    info!("Connected to database");

    let signal = SignalClient::new(
        cfg.signal_api_base_url.clone(),
        cfg.signal_sender_number.clone(),
        cfg.signal_recipients.clone(),
    );

    // Initialize shared state
    let state = Arc::new(RwLock::new(SharedState {
        metrics: Metrics::default(),
        batched: BatchedEvents::default(),
        scheduler: AdaptiveScheduler::new(cfg.clone()),
        thresholds: ThresholdTracker::new(cfg.clone()),
        session_tracker: GameSessionTracker::new(),
        filter: NotificationFilter::new(cfg.clone()),
    }));

    // Legacy metrics for heartbeat compatibility
    let metrics: Arc<RwLock<Metrics>> = Arc::new(RwLock::new(Metrics::default()));

    // Rate limiter for orchestrator notifications (5 minute cooldown for non-critical)
    let orchestrator_rate_limiter = Arc::new(RwLock::new(OrchestratorRateLimiter::new(300)));

    // Start heartbeat
    let instance_id =
        env::var("HOSTNAME").unwrap_or_else(|_| "notification-service-rust-1".to_string());
    {
        let redis_hb = redis.clone();
        let metrics_hb = metrics.clone();
        tokio::spawn(async move {
            if let Err(e) = heartbeat_loop(redis_hb, instance_id, metrics_hb).await {
                error!("Heartbeat loop error: {}", e);
            }
        });
    }

    // Start adaptive summary timer
    {
        let signal_summary = signal.clone();
        let state_summary = state.clone();
        let db_summary = db_pool.clone();
        let cfg_summary = cfg.clone();
        tokio::spawn(async move {
            adaptive_summary_loop(signal_summary, state_summary, db_summary, cfg_summary).await;
        });
    }

    // Subscribe to notification events and fault tolerance channels
    let mut pubsub = redis.subscribe(channels::NOTIFICATION_EVENTS).await?;
    info!("Subscribed to {}", channels::NOTIFICATION_EVENTS);

    // Subscribe to fault tolerance notification channels
    pubsub
        .subscribe(channels::NOTIFICATIONS_SERVICE_HEALTH)
        .await?;
    info!("Subscribed to {}", channels::NOTIFICATIONS_SERVICE_HEALTH);

    pubsub
        .subscribe(channels::NOTIFICATIONS_SERVICE_RESYNC)
        .await?;
    info!("Subscribed to {}", channels::NOTIFICATIONS_SERVICE_RESYNC);

    pubsub
        .subscribe(channels::NOTIFICATIONS_CIRCUIT_BREAKER)
        .await?;
    info!(
        "Subscribed to {}",
        channels::NOTIFICATIONS_CIRCUIT_BREAKER
    );

    pubsub
        .subscribe(channels::NOTIFICATIONS_DEGRADATION)
        .await?;
    info!("Subscribed to {}", channels::NOTIFICATIONS_DEGRADATION);

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        // Track received
        {
            let mut m = metrics.write().await;
            m.received += 1;
        }

        // Get channel name to determine message type
        let channel: String = match msg.get_channel_name() {
            s if s.starts_with(channels::NOTIFICATION_EVENTS) => {
                channels::NOTIFICATION_EVENTS.to_string()
            }
            s if s.starts_with(channels::NOTIFICATIONS_SERVICE_HEALTH) => {
                channels::NOTIFICATIONS_SERVICE_HEALTH.to_string()
            }
            s if s.starts_with(channels::NOTIFICATIONS_SERVICE_RESYNC) => {
                channels::NOTIFICATIONS_SERVICE_RESYNC.to_string()
            }
            s if s.starts_with(channels::NOTIFICATIONS_CIRCUIT_BREAKER) => {
                channels::NOTIFICATIONS_CIRCUIT_BREAKER.to_string()
            }
            s if s.starts_with(channels::NOTIFICATIONS_DEGRADATION) => {
                channels::NOTIFICATIONS_DEGRADATION.to_string()
            }
            _ => {
                warn!("Unknown channel: {}", msg.get_channel_name());
                continue;
            }
        };

        // Get payload as bytes (consistent with other services)
        let payload_bytes: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
            Ok(p) => p,
            Err(_) => {
                // Fallback to string and convert to bytes
                match msg.get_payload::<String>() {
                    Ok(s) => s.into_bytes(),
                    Err(e) => {
                        warn!("notification event: failed to read payload: {}", e);
                        let mut m = metrics.write().await;
                        m.parse_errors += 1;
                        continue;
                    }
                }
            }
        };

        // Handle fault tolerance notifications separately (with rate limiting)
        if channel != channels::NOTIFICATION_EVENTS {
            handle_orchestrator_notification(&channel, &payload_bytes, &signal, &metrics, &orchestrator_rate_limiter).await;
            continue;
        }

        // Parse JSON from bytes (handles UTF-8 correctly)
        let mut event: NotificationEvent = match serde_json::from_slice(&payload_bytes) {
            Ok(e) => e,
            Err(e) => {
                // Debug: show what we actually received
                let payload_preview = String::from_utf8_lossy(&payload_bytes);
                let preview = if payload_preview.len() > 100 {
                    format!("{}...", &payload_preview[..100])
                } else {
                    payload_preview.to_string()
                };
                warn!("notification event: invalid JSON: {} | payload ({} bytes): {}", e, payload_bytes.len(), preview);
                let mut m = metrics.write().await;
                m.parse_errors += 1;
                continue;
            }
        };

        if event.ts.is_none() {
            event.ts = Some(Utc::now());
        }

        // Process event through shared state
        let alerts = process_event(&event, &state, &signal, &metrics, &cfg).await;

        // Send any threshold alerts
        for alert in alerts {
            let message = alert.format_message();
            if let Err(e) = signal.send(&message).await {
                error!("Failed to send threshold alert: {}", e);
            } else {
                info!("Sent threshold alert: {:?}", alert);
            }
        }
    }

    Ok(())
}

/// Process a notification event and return any immediate alerts
async fn process_event(
    event: &NotificationEvent,
    state: &Arc<RwLock<SharedState>>,
    signal: &SignalClient,
    metrics: &Arc<RwLock<Metrics>>,
    cfg: &Config,
) -> Vec<thresholds::ThresholdAlert> {
    let mut alerts = Vec::new();

    // Error events are always sent immediately (not batched)
    if event.event_type == NotificationType::Error {
        let should_send = {
            let mut s = state.write().await;
            let (send, reason) = s.filter.should_notify(event.priority);
            if !send {
                let mut m = metrics.write().await;
                m.filtered += 1;
                info!(
                    "Filtered error notification: priority={:?} reason={}",
                    event.priority,
                    reason.unwrap_or_else(|| "unknown".to_string())
                );
            }
            send
        };

        if should_send {
            let message = formatters::format_message(event);
            if let Err(e) = signal.send(&message).await {
                error!("Signal send failed: {}", e);
                let mut m = metrics.write().await;
                m.send_errors += 1;
            } else {
                info!("Sent error notification: priority={:?}", event.priority);
                let mut m = metrics.write().await;
                m.sent += 1;
            }
        }

        return alerts;
    }

    // Batch trade events and check thresholds
    let mut s = state.write().await;

    match event.event_type {
        NotificationType::TradeEntry => {
            s.batched.trade_entries += 1;
            s.scheduler.record_trade();

            if let Some(size) = event.data.get("size").and_then(|v| v.as_f64()) {
                s.batched.total_entry_size += size;
            }

            // Record in threshold tracker (no PnL yet)
            let entry_alerts = s.thresholds.record_trade(None, None);
            alerts.extend(entry_alerts);
        }
        NotificationType::TradeExit => {
            s.batched.trade_exits += 1;
            s.scheduler.record_trade();

            let pnl = event.data.get("pnl").and_then(|v| v.as_f64());
            if let Some(p) = pnl {
                s.batched.total_exit_pnl += p;

                // Track wins/losses for session
                let is_win = p > 0.0;
                if is_win {
                    s.batched.wins += 1;
                } else {
                    s.batched.losses += 1;
                }

                // Record in session tracker
                s.session_tracker.record_trade(Some(p));

                // Check threshold alerts
                let exit_alerts = s.thresholds.record_trade(Some(p), Some(is_win));
                alerts.extend(exit_alerts);
            }
        }
        NotificationType::RiskRejection => {
            s.batched.risk_rejections += 1;
        }
        _ => {}
    }

    s.batched.last_update = Some(Utc::now());

    info!(
        "Batched notification: type={:?} (entries={} exits={} pnl=${:.2})",
        event.event_type, s.batched.trade_entries, s.batched.trade_exits, s.batched.total_exit_pnl
    );

    alerts
}

/// Adaptive summary loop with context-aware intervals
async fn adaptive_summary_loop(
    signal: SignalClient,
    state: Arc<RwLock<SharedState>>,
    db: PgPool,
    cfg: Config,
) {
    info!("Starting adaptive summary loop");

    // Start with a reasonable default interval
    let mut current_interval = std::time::Duration::from_secs(cfg.summary_interval_games_mins * 60);

    loop {
        tokio::time::sleep(current_interval).await;

        // Query game counts with freshness check
        let game_counts = match get_game_counts(&db, cfg.game_freshness_mins, cfg.upcoming_games_window_hours).await {
            Ok(counts) => counts,
            Err(e) => {
                warn!("Failed to query game counts: {}", e);
                game_context::GameCounts::default()
            }
        };

        // Get active game IDs for session tracking
        let active_game_ids = match get_active_game_ids(&db, cfg.game_freshness_mins).await {
            Ok(ids) => ids,
            Err(e) => {
                warn!("Failed to get active game IDs: {}", e);
                Vec::new()
            }
        };

        // Update state and determine if we should send
        let (should_send, summary_opt, session_digest_opt, new_interval) = {
            let mut s = state.write().await;

            // Check quiet hours
            let is_quiet = s.filter.is_in_quiet_hours();

            // Update scheduler context
            let context = s.scheduler.update_context(
                game_counts.active,
                game_counts.imminent,
                is_quiet,
            );

            // Get new interval based on context
            let interval = s.scheduler.get_summary_interval();

            // Check if we should send summary
            let (filter_ok, skip_reason) = s.filter.should_send_summary();
            if !filter_ok {
                info!("Skipping summary: {:?}", skip_reason);
                (false, None, None, interval)
            } else {
                // Check skip logic based on activity
                let trade_count = s.batched.trade_entries + s.batched.trade_exits + s.batched.risk_rejections;
                let (skip, skip_reason) = s.scheduler.should_skip_summary(
                    trade_count,
                    game_counts.active,
                    game_counts.imminent,
                );

                if skip {
                    info!("Skipping summary: {:?} context={}", skip_reason, context.as_str());
                    (false, None, None, interval)
                } else {
                    // Check for session end (games completed)
                    let session_digest = if cfg.session_digest_enabled {
                        s.session_tracker.check_session_end(&active_game_ids)
                    } else {
                        None
                    };

                    // Record active games
                    for id in &active_game_ids {
                        s.session_tracker.record_active_game(id);
                    }

                    // Build summary data
                    let summary_data = SummaryData {
                        trade_entries: s.batched.trade_entries,
                        trade_exits: s.batched.trade_exits,
                        risk_rejections: s.batched.risk_rejections,
                        total_entry_size: s.batched.total_entry_size,
                        total_exit_pnl: s.batched.total_exit_pnl,
                        last_update: s.batched.last_update,
                        active_games: game_counts.active,
                        imminent_games: game_counts.imminent,
                        upcoming_today: game_counts.upcoming_today,
                        context: Some(context),
                        interval_mins: interval.as_secs() / 60,
                        session_pnl: s.thresholds.session_pnl(),
                        win_streak: s.thresholds.current_streak(),
                    };

                    let summary = formatters::format_summary(&summary_data);

                    // Reset batch
                    s.batched = BatchedEvents::default();

                    (true, Some(summary), session_digest, interval)
                }
            }
        };

        // Update interval for next iteration
        current_interval = new_interval;

        // Send summary if appropriate
        if should_send {
            if let Some(summary) = summary_opt {
                if let Err(e) = signal.send(&summary).await {
                    error!("Failed to send summary: {}", e);
                } else {
                    info!("Sent adaptive summary (interval={}m)", current_interval.as_secs() / 60);
                }
            }
        }

        // Send session digest if games completed
        if let Some(digest) = session_digest_opt {
            let digest_msg = formatters::format_session_digest(&digest);
            if let Err(e) = signal.send(&digest_msg).await {
                error!("Failed to send session digest: {}", e);
            } else {
                info!("Sent session digest: {} games, {} trades, ${:.2} PnL",
                    digest.games_count, digest.trades_count, digest.total_pnl);
            }
        }
    }
}

