mod engine;

use anyhow::Result;
use arbees_rust_core::models::{ExecutionRequest, ExecutionResult, ExecutionStatus};
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

    let paper_trading = env::var("PAPER_TRADING").unwrap_or_else(|_| "true".to_string()) == "true";
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

        if let Err(e) = redis.publish("execution:results", &result).await {
            error!("Failed to publish execution result: {}", e);
        }
    }

    Ok(())
}
