mod engine;

use anyhow::Result;
use arbees_rust_core::models::{
    channels, ExecutionRequest, ExecutionResult, ExecutionStatus, NotificationEvent,
    NotificationPriority, NotificationType,
};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use dotenv::dotenv;
use engine::ExecutionEngine;
use futures_util::StreamExt;
use log::{info, error, warn};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting ExecutionService Rust Service...");

    let paper_trading_val = env::var("PAPER_TRADING").unwrap_or_else(|_| "1".to_string());
    let paper_trading = matches!(paper_trading_val.to_lowercase().as_str(), "1" | "true" | "yes");
    let engine = ExecutionEngine::new(paper_trading);
    let redis = RedisBus::new().await?;

    info!("Execution Service ready (Paper Trading: {})", paper_trading);

    let mut pubsub = redis.subscribe("execution:requests").await?;
    info!("Subscribed to execution:requests");

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
            Ok(p) => p,
            Err(e) => {
                warn!("Execution request: failed to read payload: {}", e);
                continue;
            }
        };

        let request: ExecutionRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("Execution request: invalid JSON: {}", e);
                continue;
            }
        };

        let result = match engine.execute(request.clone()).await {
            Ok(res) => res,
            Err(e) => {
                error!("Execution failed for {}: {}", request.request_id, e);
                let executed_at = Utc::now();
                let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
                ExecutionResult {
                    request_id: request.request_id,
                    idempotency_key: request.idempotency_key,
                    rejection_reason: Some(e.to_string()),
                    status: ExecutionStatus::Failed,
                    order_id: None,
                    filled_qty: 0.0,
                    avg_price: 0.0,
                    fees: 0.0,
                    platform: request.platform,
                    market_id: request.market_id,
                    contract_team: request.contract_team,
                    game_id: request.game_id,
                    sport: request.sport,
                    signal_id: request.signal_id,
                    signal_type: request.signal_type,
                    edge_pct: request.edge_pct,
                    side: request.side,
                    requested_at: request.created_at,
                    executed_at,
                    latency_ms,
                }
            }
        };

        // Publish notification on execution failure
        if result.status == ExecutionStatus::Failed {
            let event = NotificationEvent {
                event_type: NotificationType::Error,
                priority: NotificationPriority::Error,
                data: serde_json::json!({
                    "service": "execution_service_rust",
                    "request_id": result.request_id,
                    "message": result.rejection_reason.clone().unwrap_or_else(|| "execution_failed".to_string()),
                }),
                ts: Some(Utc::now()),
            };
            if let Err(e) = redis.publish(channels::NOTIFICATION_EVENTS, &event).await {
                warn!("Failed to publish notification event: {}", e);
            }
        }

        if let Err(e) = redis.publish(channels::EXECUTION_RESULTS, &result).await {
            error!("Failed to publish execution result: {}", e);
        }
    }

    Ok(())
}
