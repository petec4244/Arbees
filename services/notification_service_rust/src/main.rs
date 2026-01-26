mod config;
mod filters;
mod formatters;
mod signal_client;

use anyhow::Result;
use arbees_rust_core::models::{channels, NotificationEvent};
use arbees_rust_core::redis::RedisBus;
use chrono::Utc;
use config::Config;
use dotenv::dotenv;
use filters::NotificationFilter;
use futures_util::StreamExt;
use log::{error, info, warn};
use signal_client::SignalClient;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;

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

    info!("Starting Rust Notification Service...");

    let cfg = Config::from_env()?;
    info!(
        "Config: redis_url={} recipients={} quiet_hours={} rate_limit={}/min",
        cfg.redis_url,
        cfg.signal_recipients.len(),
        cfg.quiet_hours_enabled,
        cfg.rate_limit_max_per_minute
    );

    let redis = RedisBus::new().await?;
    info!("Connected to Redis");

    let signal = SignalClient::new(
        cfg.signal_api_base_url.clone(),
        cfg.signal_sender_number.clone(),
        cfg.signal_recipients.clone(),
    );

    let mut filter = NotificationFilter::new(cfg.clone());

    let metrics: Arc<RwLock<Metrics>> = Arc::new(RwLock::new(Metrics::default()));

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

    // Subscribe to notification events
    let mut pubsub = redis.subscribe(channels::NOTIFICATION_EVENTS).await?;
    info!("Subscribed to {}", channels::NOTIFICATION_EVENTS);

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        // Track received
        {
            let mut m = metrics.write().await;
            m.received += 1;
        }

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

        let (should_send, reason) = filter.should_notify(event.priority);
        if !should_send {
            let mut m = metrics.write().await;
            m.filtered += 1;
            info!(
                "Filtered notification: priority={:?} reason={}",
                event.priority,
                reason.unwrap_or_else(|| "unknown".to_string())
            );
            continue;
        }

        // Format + send
        let message = formatters::format_message(&event);
        if let Err(e) = signal.send(&message).await {
            error!("Signal send failed: {}", e);
            let mut m = metrics.write().await;
            m.send_errors += 1;
            continue;
        }

        info!(
            "Sent notification: type={:?} priority={:?}",
            event.event_type, event.priority
        );

        let mut m = metrics.write().await;
        m.sent += 1;
    }

    Ok(())
}

