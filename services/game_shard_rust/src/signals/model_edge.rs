//! Model-based signal generation (non-arbitrage)
//!
//! Detects signals based on model edge (difference between model probability
//! and market price), using ZMQ only for emission.

use arbees_rust_core::models::{Platform, SignalDirection, SignalType, Sport, TradingSignal};
use crate::price::data::MarketPriceData;
use crate::signals::edge::compute_team_net_edge;
use crate::config::{MAX_BUY_PROB, MIN_BUY_PROB};
use crate::types::ZmqEnvelope;
use chrono::Utc;
use log::{debug, info};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeromq::{PubSocket, SocketSend, ZmqMessage};

/// Detect and emit a model-edge signal (no arbitrage)
///
/// Evaluates model probability vs market price and emits a signal if:
/// - Net edge (after fees) exceeds threshold
/// - Model probability is within valid range
///
/// Returns true if a signal was emitted, false otherwise.
pub async fn detect_and_emit(
    game_id: &str,
    sport: Sport,
    team: &str,
    model_prob: f64,
    market_price: &MarketPriceData,
    selected_platform: Platform,
    min_edge_pct: f64,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> bool {
    let (direction, signal_type, net_edge_pct, gross_edge_pct_abs, market_yes_mid) =
        compute_team_net_edge(model_prob, market_price, selected_platform);

    if net_edge_pct < min_edge_pct {
        debug!(
            "Skipping {} - gross {:.1}%, net {:.1}% < {:.1}% threshold ({:?})",
            team, gross_edge_pct_abs, net_edge_pct, min_edge_pct, selected_platform
        );
        return false;
    }

    // Probability bounds (symmetric: avoid trading near-certain outcomes)
    match direction {
        SignalDirection::Buy => {
            if model_prob > MAX_BUY_PROB {
                debug!(
                    "Skipping BUY YES for {} - prob too high: {:.1}%",
                    team,
                    model_prob * 100.0
                );
                return false;
            }
        }
        SignalDirection::Sell => {
            if model_prob < MIN_BUY_PROB {
                debug!(
                    "Skipping BUY NO for {} - prob too low (NO too high): {:.1}%",
                    team,
                    model_prob * 100.0
                );
                return false;
            }
        }
        SignalDirection::Hold => return false,
    }

    // Executable entry/exit prices for better logging + UI
    let (buy_price, sell_price, liquidity_available) = match direction {
        SignalDirection::Buy => (
            market_price.yes_ask,
            market_price.yes_bid,
            market_price
                .yes_ask_size
                .or(market_price.total_liquidity)
                .unwrap_or(100.0),
        ),
        SignalDirection::Sell => {
            let no_ask = (1.0 - market_price.yes_bid).clamp(0.0, 1.0);
            let no_bid = (1.0 - market_price.yes_ask).clamp(0.0, 1.0);
            (
                no_ask,
                no_bid,
                market_price
                    .yes_bid_size
                    .or(market_price.total_liquidity)
                    .unwrap_or(100.0),
            )
        }
        SignalDirection::Hold => (market_price.mid_price, market_price.mid_price, 0.0),
    };

    // Create signal with the selected platform
    // Use fee-adjusted edge for the signal's edge_pct field
    let signal = TradingSignal {
        signal_id: Uuid::new_v4().to_string(),
        signal_type,
        game_id: game_id.to_string(),
        sport,
        team: team.to_string(),
        direction,
        model_prob,
        market_prob: Some(market_yes_mid),
        edge_pct: net_edge_pct, // Fee-adjusted, executable edge
        confidence: (net_edge_pct / 10.0).min(1.0), // Confidence based on net edge
        platform_buy: Some(selected_platform),
        platform_sell: None,
        buy_price: Some(buy_price),
        sell_price: Some(sell_price),
        liquidity_available,
        reason: format!(
            "Model YES: {:.1}% vs Market YES: {:.1}% = {:.1}% gross / {:.1}% net ({:?})",
            model_prob * 100.0,
            market_yes_mid * 100.0,
            gross_edge_pct_abs,
            net_edge_pct,
            selected_platform
        ),
        created_at: Utc::now(),
        // Increased from 30s to 60s for better signal processing time
        expires_at: Some(Utc::now() + chrono::Duration::seconds(60)),
        play_id: None,
        // Universal fields (backward compat - use game_id/team for sports)
        event_id: None,
        market_type: None,
        entity: None,
    };

    // Format direction as "to win" / "to lose" for clarity
    let direction_str = match direction {
        SignalDirection::Buy => "to win",
        SignalDirection::Sell => "to lose",
        SignalDirection::Hold => "hold",
    };
    info!(
        "SIGNAL: {} {} - model_yes={:.1}% market_yes={:.1}% gross={:.1}% net={:.1}% ({:?})",
        team,
        direction_str,
        model_prob * 100.0,
        market_yes_mid * 100.0,
        gross_edge_pct_abs,
        net_edge_pct,
        selected_platform
    );

    // Publish signal via ZMQ only
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
