//! Cross-platform arbitrage signal detection and emission
//!
//! Detects and emits signals for risk-free arbitrage opportunities
//! between Kalshi and Polymarket, using ZMQ only.

use arbees_rust_core::models::{Platform, SignalDirection, SignalType, Sport, TradingSignal};
use arbees_rust_core::simd::{ARB_KALSHI_YES_POLY_NO, ARB_POLY_YES_KALSHI_NO, decode_arb_mask};
use crate::price::data::MarketPriceData;
use crate::types::ZmqEnvelope;
use chrono::Utc;
use log::info;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeromq::{PubSocket, SocketSend, ZmqMessage};

/// Detect and emit a cross-platform arbitrage signal
///
/// Called when an arbitrage opportunity is found between Kalshi and Polymarket.
/// Publishes a single signal via ZMQ (no Redis fallback).
pub async fn detect_and_emit(
    game_id: &str,
    sport: Sport,
    team: &str,
    arb_mask: u8,
    profit_cents: i16,
    kalshi_price: &MarketPriceData,
    poly_price: &MarketPriceData,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> bool {
    let arb_types = decode_arb_mask(arb_mask);
    let arb_type_str = arb_types.first().unwrap_or(&"Unknown");

    let (buy_platform, sell_platform) = if arb_mask == ARB_POLY_YES_KALSHI_NO {
        (Platform::Polymarket, Platform::Kalshi)
    } else {
        (Platform::Kalshi, Platform::Polymarket)
    };

    // For ARB signals, use the buy-side YES price as market_prob
    // This ensures signal_processor doesn't reject for "no_market"
    let buy_yes_price = if arb_mask == ARB_POLY_YES_KALSHI_NO {
        poly_price.yes_ask
    } else {
        kalshi_price.yes_ask
    };

    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type: SignalType::CrossMarketArb,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction: SignalDirection::Buy,
        model_prob: buy_yes_price, // Use buy price for risk calculations
        market_prob: Some(buy_yes_price),
        edge_pct: profit_cents as f64,
        confidence: 1.0,
        platform_buy: Some(buy_platform),
        platform_sell: Some(sell_platform),
        buy_price: Some(if arb_mask == ARB_POLY_YES_KALSHI_NO {
            poly_price.yes_ask
        } else {
            kalshi_price.yes_ask
        }),
        sell_price: Some(if arb_mask == ARB_POLY_YES_KALSHI_NO {
            1.0 - kalshi_price.yes_bid
        } else {
            1.0 - poly_price.yes_bid
        }),
        liquidity_available: kalshi_price.yes_ask_size.unwrap_or(100.0).min(
            poly_price.yes_ask_size.unwrap_or(100.0)
        ),
        reason: format!(
            "ARB: {} - profit={:.0}¢ (buy {:?} YES + {:?} NO)",
            arb_type_str, profit_cents, buy_platform, sell_platform
        ),
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::seconds(10)),
        play_id: None,
        // Universal fields (backward compat - use game_id/team for sports)
        event_id: None,
        market_type: None,
        entity: None,
    };

    info!(
        "ARB SIGNAL: {} {} - profit={}¢ ({:?} YES + {:?} NO)",
        team, arb_type_str, profit_cents, buy_platform, sell_platform
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
