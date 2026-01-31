//! ZMQ-only signal publishing (low-latency)
//!
//! This module handles publishing trading signals via ZMQ only.
//! Redis publishing has been removed for efficiency.

use arbees_rust_core::models::TradingSignal;
use crate::types::ZmqEnvelope;
use chrono::Utc;
use log::{debug, warn};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use zeromq::{PubSocket, SocketSend, ZmqMessage};

/// Publish a trading signal via ZMQ for low-latency consumers
///
/// This is the primary signal publishing path. Signals are wrapped in an envelope
/// with sequence numbers and timestamps for ordering and latency analysis.
pub async fn publish(
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
                warn!("Failed to serialize ZMQ signal envelope: {}", e);
                return;
            }
        };

        let mut socket = pub_socket.lock().await;
        let mut msg = ZmqMessage::from(topic.as_bytes().to_vec());
        msg.push_back(payload.into());
        if let Err(e) = socket.send(msg).await {
            warn!("Failed to publish signal via ZMQ: {}", e);
        } else {
            debug!("Published signal {} via ZMQ (seq={})", signal.signal_id, seq);
        }
    }
}
