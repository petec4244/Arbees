//! Latency-based signal detection (score change signals)
//!
//! Detects signals when team scores are detected before market adjusts,
//! using ZMQ only for emission.
//!
//! **Note:** This is currently disabled by default. ESPN's scoreboard API
//! is too slow - by the time we detect a score, the market has already adjusted.
//! To enable: set LATENCY_SIGNALS_ENABLED=true (requires faster data source).

use arbees_rust_core::models::{Platform, SignalDirection, SignalType, Sport, TradingSignal};
use crate::price::data::MarketPriceData;
use crate::types::ZmqEnvelope;
use chrono::Utc;
use log::info;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeromq::{PubSocket, SocketSend, ZmqMessage};

/// Detect and emit a latency-based signal when a team scores
///
/// Called when a score change is detected. We bet on the expected price movement
/// after the score is reflected in the market.
///
/// Returns true if a signal was emitted, false otherwise.
pub async fn detect_and_emit(
    game_id: &str,
    sport: Sport,
    team: &str,
    direction: SignalDirection,
    market_price: &MarketPriceData,
    platform: Platform,
    model_prob: f64,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> bool {
    // For a BUY signal, we expect the price to go UP after the score
    // Edge is the expected price movement (model prob - current market price)
    let current_price = match direction {
        SignalDirection::Buy => market_price.yes_ask,
        SignalDirection::Sell => 1.0 - market_price.yes_bid,
        SignalDirection::Hold => return false,
    };
    let expected_move = (model_prob - current_price).abs() * 100.0;

    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type: SignalType::ScoringPlay,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction,
        model_prob,
        market_prob: Some(current_price),
        edge_pct: expected_move, // Expected price movement as edge
        confidence: 0.9, // High confidence for latency plays
        platform_buy: Some(platform),
        platform_sell: None,
        buy_price: Some(current_price),
        sell_price: None,
        liquidity_available: market_price.yes_ask_size.or(market_price.total_liquidity).unwrap_or(100.0),
        reason: format!(
            "LATENCY: Score detected! Current={:.1}% → Expected={:.1}% (move={:.1}%)",
            current_price * 100.0,
            model_prob * 100.0,
            expected_move
        ),
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::seconds(60)), // 1 minute expiry
        play_id: None,
        // Universal fields (backward compat - use game_id/team for sports)
        event_id: None,
        market_type: None,
        entity: None,
    };

    info!(
        "LATENCY SIGNAL: {} {:?} - current={:.1}% → expected={:.1}% (move={:.1}%)",
        team, direction,
        current_price * 100.0,
        model_prob * 100.0,
        expected_move
    );

    // Publish via ZMQ only
    publish_zmq(&signal, zmq_pub, zmq_seq).await;
    true
}

/// Internal: Publish signal via ZMQ
async fn publish_zmq(
    signal: &TradingSignal,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) {
    if let Some(ref pub_socket) = zmq_pub {
        let topic = format!("signals.trade.{}", signal.signal_id);
        let seq = zmq_seq.fetch_add(1, Ordering::Relaxed);

        let envelope = ZmqEnvelope {
            seq,
            timestamp_ms: Utc::now().timestamp_millis(),
            source: Some("game_shard".to_string()),
            payload: serde_json::to_value(signal).unwrap_or_default(),
        };

        let payload = match serde_json::to_vec(&envelope) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("Failed to serialize ZMQ signal envelope: {}", e);
                return;
            }
        };

        let mut socket = pub_socket.lock().await;
        let mut msg = ZmqMessage::from(topic.as_bytes().to_vec());
        msg.push_back(payload.into());
        if let Err(e) = socket.send(msg).await {
            log::warn!("Failed to publish signal via ZMQ: {}", e);
        }
    }
}
