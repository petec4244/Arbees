//! Shared type definitions for game shard
//!
//! This module contains all struct definitions used across the shard:
//! - ZmqEnvelope: Message wrapper for ZMQ transport
//! - PriceListenerStats: Health monitoring for price ingestion
//! - GameContext: Metadata for a tracked game
//! - GameEntry: Runtime state for a game task
//! - ShardCommand: Control messages from orchestrator

use arbees_rust_core::models::MarketType;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::RwLock;

/// ZMQ message envelope format for consistent message routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZmqEnvelope {
    /// Sequence number for message ordering
    pub seq: u64,
    /// Timestamp in milliseconds when message was created
    pub timestamp_ms: i64,
    /// Source service identifier
    pub source: Option<String>,
    /// Message payload as JSON
    pub payload: serde_json::Value,
}

/// Statistics for monitoring price message processing health
#[derive(Debug, Default)]
pub struct PriceListenerStats {
    /// Total price messages received
    pub messages_received: AtomicU64,
    /// Messages successfully parsed and processed
    pub messages_processed: AtomicU64,
    /// Messages that failed to parse (msgpack or JSON)
    pub parse_failures: AtomicU64,
    /// Messages skipped due to no liquidity
    pub no_liquidity_skipped: AtomicU64,
    /// Messages skipped due to missing contract_team
    pub no_team_skipped: AtomicU64,
}

impl PriceListenerStats {
    /// Take a snapshot of current statistics
    pub fn snapshot(&self) -> PriceListenerStatsSnapshot {
        PriceListenerStatsSnapshot {
            messages_received: self.messages_received.load(std::sync::atomic::Ordering::Relaxed),
            messages_processed: self.messages_processed.load(std::sync::atomic::Ordering::Relaxed),
            parse_failures: self.parse_failures.load(std::sync::atomic::Ordering::Relaxed),
            no_liquidity_skipped: self.no_liquidity_skipped.load(std::sync::atomic::Ordering::Relaxed),
            no_team_skipped: self.no_team_skipped.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

/// Snapshot of price listener statistics at a point in time
#[derive(Debug, Clone)]
pub struct PriceListenerStatsSnapshot {
    pub messages_received: u64,
    pub messages_processed: u64,
    pub parse_failures: u64,
    pub no_liquidity_skipped: u64,
    pub no_team_skipped: u64,
}

/// Game metadata and market identifiers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameContext {
    pub game_id: String,
    pub sport: String,                   // Keep for backward compatibility
    pub market_type: Option<MarketType>, // Universal market type
    pub entity_a: Option<String>,        // Generic entity A (home_team for sports)
    pub entity_b: Option<String>,        // Generic entity B (away_team for sports)
    pub polymarket_id: Option<String>,
    pub kalshi_id: Option<String>,
}

/// Runtime tracking for a game being monitored
pub struct GameEntry {
    pub context: GameContext,
    pub task: tokio::task::JoinHandle<()>,
    /// Last calculated home win probability
    pub last_home_win_prob: Arc<RwLock<Option<f64>>>,
    /// Opening market line for home team (first price we see, used as team strength prior)
    pub opening_home_prob: Arc<RwLock<Option<f64>>>,
}

/// Control command from orchestrator to shard
#[derive(Debug, Deserialize)]
pub struct ShardCommand {
    #[serde(rename = "type")]
    pub command_type: String,
    pub game_id: Option<String>,
    pub event_id: Option<String>,           // Universal event ID (for non-sports markets)
    pub sport: Option<String>,
    pub market_type: Option<MarketType>,    // Universal market type (sport, crypto, economics, politics)
    pub entity_a: Option<String>,           // Generic entity A (home_team for sports)
    pub entity_b: Option<String>,           // Generic entity B (away_team for sports)
    pub kalshi_market_id: Option<String>,
    pub polymarket_market_id: Option<String>,
    pub metadata: Option<serde_json::Value>, // Additional market-specific metadata
}
