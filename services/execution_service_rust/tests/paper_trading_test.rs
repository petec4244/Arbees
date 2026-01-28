//! Integration tests for the Execution Engine
//!
//! These tests verify paper trading execution without external dependencies.

use arbees_rust_core::models::{
    ExecutionRequest, ExecutionSide, ExecutionStatus, Platform, Sport,
};
use chrono::Utc;
use execution_service_rust::engine::ExecutionEngine;
use uuid::Uuid;

/// Create a test execution request
fn create_test_request(platform: Platform, price: f64, size: f64) -> ExecutionRequest {
    ExecutionRequest {
        request_id: Uuid::new_v4().to_string(),
        idempotency_key: format!("test-{}", Uuid::new_v4()),
        platform,
        market_id: "TEST-MARKET-123".to_string(),
        contract_team: Some("Test Team".to_string()),
        token_id: None, // For Polymarket CLOB
        game_id: "test-game-456".to_string(),
        sport: Sport::NBA,
        side: ExecutionSide::Yes,
        limit_price: price,
        size,
        signal_id: "test-signal-789".to_string(),
        signal_type: "test".to_string(),
        edge_pct: 5.0,
        model_prob: 0.60,
        market_prob: Some(0.55),
        reason: "Test execution".to_string(),
        created_at: Utc::now(),
        expires_at: None,
    }
}

#[tokio::test]
async fn test_paper_trading_execution_fills_order() {
    let engine = ExecutionEngine::new(true).await; // Paper trading mode

    let request = create_test_request(Platform::Paper, 0.55, 10.0);
    let result = engine.execute(request.clone()).await.expect("Execution should succeed");

    assert_eq!(result.status, ExecutionStatus::Filled);
    assert_eq!(result.filled_qty, 10.0);
    assert_eq!(result.avg_price, 0.55);
    assert!(result.order_id.is_some());
    assert!(result.order_id.unwrap().starts_with("paper-"));
    assert!(result.rejection_reason.is_none());
    assert_eq!(result.platform, Platform::Paper);
}

#[tokio::test]
async fn test_paper_trading_calculates_kalshi_fees() {
    let engine = ExecutionEngine::new(true).await;

    // At 50% price, Kalshi fee is max: ceil(7 * 50 * 50 / 10000) = 2 cents per contract
    let request = create_test_request(Platform::Kalshi, 0.50, 100.0);
    let result = engine.execute(request).await.expect("Execution should succeed");

    assert_eq!(result.status, ExecutionStatus::Filled);
    // 2 cents * 100 contracts = $2.00
    assert!((result.fees - 2.0).abs() < 0.01, "Expected ~$2.00 in fees, got ${:.4}", result.fees);
}

#[tokio::test]
async fn test_paper_trading_calculates_fees_at_extreme_prices() {
    let engine = ExecutionEngine::new(true).await;

    // At 95% price, fee should be lower
    // ceil(7 * 95 * 5 / 10000) = ceil(0.3325) = 1 cent per contract
    let request = create_test_request(Platform::Kalshi, 0.95, 50.0);
    let result = engine.execute(request).await.expect("Execution should succeed");

    assert_eq!(result.status, ExecutionStatus::Filled);
    // 1 cent * 50 contracts = $0.50
    assert!((result.fees - 0.50).abs() < 0.01, "Expected ~$0.50 in fees, got ${:.4}", result.fees);
}

#[tokio::test]
async fn test_paper_trading_tracks_latency() {
    let engine = ExecutionEngine::new(true).await;

    let request = create_test_request(Platform::Paper, 0.60, 5.0);
    let result = engine.execute(request).await.expect("Execution should succeed");

    // Latency should be positive and reasonable (< 1 second for paper trades)
    assert!(result.latency_ms >= 0.0);
    assert!(result.latency_ms < 1000.0, "Latency too high: {}ms", result.latency_ms);
}

#[tokio::test]
async fn test_paper_trading_preserves_request_fields() {
    let engine = ExecutionEngine::new(true).await;

    let request = create_test_request(Platform::Paper, 0.65, 20.0);
    let request_id = request.request_id.clone();
    let idempotency_key = request.idempotency_key.clone();
    let market_id = request.market_id.clone();
    let game_id = request.game_id.clone();
    let signal_id = request.signal_id.clone();

    let result = engine.execute(request).await.expect("Execution should succeed");

    assert_eq!(result.request_id, request_id);
    assert_eq!(result.idempotency_key, idempotency_key);
    assert_eq!(result.market_id, market_id);
    assert_eq!(result.game_id, game_id);
    assert_eq!(result.signal_id, signal_id);
    assert_eq!(result.sport, Sport::NBA);
    assert_eq!(result.side, ExecutionSide::Yes);
}

#[tokio::test]
async fn test_kalshi_live_disabled_in_paper_mode() {
    let engine = ExecutionEngine::new(true).await; // Paper trading mode

    // In paper trading mode, live trading should be disabled
    assert!(!engine.kalshi_live_enabled());
}

#[tokio::test]
async fn test_polymarket_execution_rejected_without_clob() {
    let engine = ExecutionEngine::new(false).await; // Live trading mode

    let request = create_test_request(Platform::Polymarket, 0.55, 10.0);
    let result = engine.execute(request).await.expect("Execution should return result");

    // Polymarket live trading is not implemented
    assert_eq!(result.status, ExecutionStatus::Rejected);
    assert!(result.rejection_reason.is_some());
    assert!(result.rejection_reason.unwrap().contains("CLOB"));
}
