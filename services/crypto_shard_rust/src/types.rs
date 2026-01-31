//! Crypto-specific types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Crypto event context (replaces GameContext for crypto)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoEventContext {
    pub event_id: String,
    pub asset: String,
    pub event_type: CryptoEventType,
    pub target_price: Option<f64>,
    pub target_date: DateTime<Utc>,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CryptoEventType {
    PriceTarget,
    Volatility,
    Correlation,
}

/// Crypto execution request (combines signal + execution)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoExecutionRequest {
    pub request_id: String,
    pub event_id: String,
    pub asset: String,
    pub signal_type: CryptoSignalType,
    pub platform: String,
    pub market_id: String,
    pub direction: Direction,
    pub edge_pct: f64,
    pub probability: f64,
    pub suggested_size: f64,
    pub max_price: f64,
    pub current_price: f64,
    pub timestamp: DateTime<Utc>,

    // Risk metadata
    pub volatility_factor: f64,
    pub exposure_check: bool,
    pub balance_check: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CryptoSignalType {
    Arbitrage,
    ModelEdge,
    VolatilityMispricing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Direction {
    Long,
    Short,
}

/// ZMQ envelope wrapper for non-generic payloads (prices with two-step deserialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceZmqEnvelope {
    pub seq: u64,
    pub timestamp_ms: i64,
    pub source: String,
    pub payload: serde_json::Value,  // Non-generic for two-step parsing
}

/// ZMQ envelope wrapper (generic - for execution requests and other typed payloads)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZmqEnvelope<T> {
    pub seq: u64,
    pub timestamp_ms: i64,
    pub source: String,
    pub payload: T,
}

/// Shard command (from orchestrator via Redis)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShardCommand {
    AddEvent { event: CryptoEventContext },
    RemoveEvent { event_id: String },
    Shutdown,
}

/// Statistics tracking
pub struct CryptoShardStats {
    pub events_monitored: std::sync::atomic::AtomicU64,
    pub prices_received: std::sync::atomic::AtomicU64,
    pub arbitrage_signals: std::sync::atomic::AtomicU64,
    pub model_signals: std::sync::atomic::AtomicU64,
    pub execution_requests_sent: std::sync::atomic::AtomicU64,
    pub risk_blocks: std::sync::atomic::AtomicU64,
}

impl CryptoShardStats {
    pub fn new() -> Self {
        use std::sync::atomic::AtomicU64;
        Self {
            events_monitored: AtomicU64::new(0),
            prices_received: AtomicU64::new(0),
            arbitrage_signals: AtomicU64::new(0),
            model_signals: AtomicU64::new(0),
            execution_requests_sent: AtomicU64::new(0),
            risk_blocks: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> CryptoShardStatsSnapshot {
        use std::sync::atomic::Ordering;
        CryptoShardStatsSnapshot {
            events_monitored: self.events_monitored.load(Ordering::Relaxed),
            prices_received: self.prices_received.load(Ordering::Relaxed),
            arbitrage_signals: self.arbitrage_signals.load(Ordering::Relaxed),
            model_signals: self.model_signals.load(Ordering::Relaxed),
            execution_requests_sent: self.execution_requests_sent.load(Ordering::Relaxed),
            risk_blocks: self.risk_blocks.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CryptoShardStatsSnapshot {
    pub events_monitored: u64,
    pub prices_received: u64,
    pub arbitrage_signals: u64,
    pub model_signals: u64,
    pub execution_requests_sent: u64,
    pub risk_blocks: u64,
}

impl Default for CryptoShardStats {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Long
    }
}

impl Direction {
    pub fn opposite(&self) -> Self {
        match self {
            Direction::Long => Direction::Short,
            Direction::Short => Direction::Long,
        }
    }
}

impl CryptoExecutionRequest {
    pub fn new(
        event_id: String,
        asset: String,
        signal_type: CryptoSignalType,
        platform: String,
        market_id: String,
        direction: Direction,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            event_id,
            asset,
            signal_type,
            platform,
            market_id,
            direction,
            edge_pct: 0.0,
            probability: 0.5,
            suggested_size: 0.0,
            max_price: 0.0,
            current_price: 0.0,
            timestamp: Utc::now(),
            volatility_factor: 1.0,
            exposure_check: false,
            balance_check: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direction_opposite() {
        assert_eq!(Direction::Long.opposite(), Direction::Short);
        assert_eq!(Direction::Short.opposite(), Direction::Long);
    }

    #[test]
    fn test_direction_default() {
        assert_eq!(Direction::default(), Direction::Long);
    }

    #[test]
    fn test_crypto_execution_request_new() {
        let req = CryptoExecutionRequest::new(
            "btc_100k".to_string(),
            "BTC".to_string(),
            CryptoSignalType::Arbitrage,
            "kalshi".to_string(),
            "market_123".to_string(),
            Direction::Long,
        );

        assert_eq!(req.event_id, "btc_100k");
        assert_eq!(req.asset, "BTC");
        assert_eq!(req.signal_type, CryptoSignalType::Arbitrage);
        assert_eq!(req.platform, "kalshi");
        assert_eq!(req.market_id, "market_123");
        assert_eq!(req.direction, Direction::Long);
        assert_eq!(req.edge_pct, 0.0);
        assert_eq!(req.probability, 0.5);
        assert!(!req.request_id.is_empty());
    }

    #[test]
    fn test_crypto_event_context_serialization() {
        let event = CryptoEventContext {
            event_id: "btc_100k_2025".to_string(),
            asset: "BTC".to_string(),
            event_type: CryptoEventType::PriceTarget,
            target_price: Some(100000.0),
            target_date: Utc::now(),
            description: "BTC hits $100k".to_string(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: CryptoEventContext = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.event_id, event.event_id);
        assert_eq!(deserialized.asset, event.asset);
        assert_eq!(deserialized.event_type, CryptoEventType::PriceTarget);
    }

    #[test]
    fn test_zmq_envelope_serialization() {
        let payload = "test_payload".to_string();
        let envelope = ZmqEnvelope {
            seq: 42,
            timestamp_ms: 1234567890,
            source: "test_source".to_string(),
            payload,
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: ZmqEnvelope<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.seq, 42);
        assert_eq!(deserialized.timestamp_ms, 1234567890);
        assert_eq!(deserialized.source, "test_source");
        assert_eq!(deserialized.payload, "test_payload");
    }

    #[test]
    fn test_crypto_shard_stats() {
        let stats = CryptoShardStats::new();
        stats.events_monitored.store(10, std::sync::atomic::Ordering::Relaxed);
        stats.arbitrage_signals.store(5, std::sync::atomic::Ordering::Relaxed);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.events_monitored, 10);
        assert_eq!(snapshot.arbitrage_signals, 5);
        assert_eq!(snapshot.prices_received, 0);
    }

    #[test]
    fn test_shard_command_serialization() {
        let event = CryptoEventContext {
            event_id: "test".to_string(),
            asset: "BTC".to_string(),
            event_type: CryptoEventType::PriceTarget,
            target_price: Some(50000.0),
            target_date: Utc::now(),
            description: "Test".to_string(),
            created_at: Utc::now(),
        };

        let cmd = ShardCommand::AddEvent { event: event.clone() };
        let json = serde_json::to_string(&cmd).unwrap();
        let _deserialized: ShardCommand = serde_json::from_str(&json).unwrap();

        // Just verify it serializes/deserializes without panicking
    }
}
