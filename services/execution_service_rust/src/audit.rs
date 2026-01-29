//! Audit logging module for execution service
//!
//! Provides structured JSON audit logging for all execution events.
//! Logs are written to console and optionally published to Redis.

use arbees_rust_core::models::{ExecutionRequest, ExecutionResult, ExecutionStatus, Platform};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::{DateTime, Utc};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Redis channel for audit events
pub const AUDIT_CHANNEL: &str = "audit:execution";

/// Types of audit events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Execution request received
    ExecutionRequested,
    /// Order successfully placed
    OrderPlaced,
    /// Order fully filled
    OrderFilled,
    /// Order partially filled
    OrderPartialFill,
    /// Order rejected (validation failed)
    OrderRejected,
    /// Order failed (API error)
    OrderFailed,
    /// Order cancelled
    OrderCancelled,
    /// Kill switch activated
    KillSwitchActivated,
    /// Kill switch deactivated
    KillSwitchDeactivated,
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Idempotency duplicate detected
    DuplicateDetected,
    /// Balance validation failed
    InsufficientBalance,
    /// Price sanity check failed
    PriceSanityFailed,
    /// Daily loss limit warning
    DailyLossWarning,
    /// Daily loss limit exceeded
    DailyLossExceeded,
}

/// Structured audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Timestamp of the event
    pub timestamp: DateTime<Utc>,
    /// Type of event
    pub event_type: AuditEventType,
    /// Request ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Idempotency key (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Platform
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
    /// Market ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_id: Option<String>,
    /// Game ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    /// Order ID (if order was placed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    /// Order size
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Limit price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    /// Filled quantity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_qty: Option<f64>,
    /// Signal ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
    /// Edge percentage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_pct: Option<f64>,
    /// Rejection/failure reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Execution latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl AuditLogEntry {
    /// Create a new audit entry with just event type
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            request_id: None,
            idempotency_key: None,
            platform: None,
            market_id: None,
            game_id: None,
            order_id: None,
            size: None,
            price: None,
            filled_qty: None,
            signal_id: None,
            edge_pct: None,
            reason: None,
            latency_ms: None,
            metadata: None,
        }
    }

    /// Create an audit entry from an execution request
    pub fn from_request(event_type: AuditEventType, request: &ExecutionRequest) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            request_id: Some(request.request_id.clone()),
            idempotency_key: Some(request.idempotency_key.clone()),
            platform: Some(request.platform),
            market_id: Some(request.market_id.clone()),
            game_id: Some(request.game_id.clone()),
            order_id: None,
            size: Some(request.size),
            price: Some(request.limit_price),
            filled_qty: None,
            signal_id: Some(request.signal_id.clone()),
            edge_pct: Some(request.edge_pct),
            reason: None,
            latency_ms: None,
            metadata: None,
        }
    }

    /// Create an audit entry from an execution result
    pub fn from_result(result: &ExecutionResult) -> Self {
        let event_type = match result.status {
            ExecutionStatus::Filled => AuditEventType::OrderFilled,
            ExecutionStatus::Partial => AuditEventType::OrderPartialFill,
            ExecutionStatus::Rejected => AuditEventType::OrderRejected,
            ExecutionStatus::Failed => AuditEventType::OrderFailed,
            ExecutionStatus::Cancelled => AuditEventType::OrderCancelled,
            ExecutionStatus::Pending => AuditEventType::OrderPlaced,
            ExecutionStatus::Accepted => AuditEventType::OrderPlaced,
        };

        Self {
            timestamp: Utc::now(),
            event_type,
            request_id: Some(result.request_id.clone()),
            idempotency_key: Some(result.idempotency_key.clone()),
            platform: Some(result.platform),
            market_id: Some(result.market_id.clone()),
            game_id: Some(result.game_id.clone()),
            order_id: result.order_id.clone(),
            size: None,
            price: Some(result.avg_price),
            filled_qty: Some(result.filled_qty),
            signal_id: Some(result.signal_id.clone()),
            edge_pct: Some(result.edge_pct),
            reason: result.rejection_reason.clone(),
            latency_ms: Some(result.latency_ms),
            metadata: None,
        }
    }

    /// Set the reason field
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Audit logger that writes to console and Redis
pub struct AuditLogger {
    redis: Option<Arc<RedisBus>>,
    enabled: bool,
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new(redis: Option<Arc<RedisBus>>, enabled: bool) -> Self {
        Self { redis, enabled }
    }

    /// Log an audit entry
    pub async fn log(&self, entry: AuditLogEntry) {
        if !self.enabled {
            return;
        }

        let json = entry.to_json();

        // Log to console with appropriate level
        match entry.event_type {
            AuditEventType::KillSwitchActivated
            | AuditEventType::DailyLossExceeded
            | AuditEventType::OrderFailed => {
                error!("[AUDIT] {}", json);
            }
            AuditEventType::OrderRejected
            | AuditEventType::RateLimitExceeded
            | AuditEventType::DuplicateDetected
            | AuditEventType::InsufficientBalance
            | AuditEventType::PriceSanityFailed
            | AuditEventType::DailyLossWarning => {
                warn!("[AUDIT] {}", json);
            }
            AuditEventType::OrderFilled
            | AuditEventType::OrderPlaced
            | AuditEventType::ExecutionRequested => {
                info!("[AUDIT] {}", json);
            }
            _ => {
                debug!("[AUDIT] {}", json);
            }
        }

        // Publish to Redis
        if let Some(redis) = &self.redis {
            if let Err(e) = redis.publish(AUDIT_CHANNEL, &json).await {
                warn!("Failed to publish audit event to Redis: {}", e);
            }
        }
    }

    /// Log execution requested event
    pub async fn log_execution_requested(&self, request: &ExecutionRequest) {
        self.log(AuditLogEntry::from_request(
            AuditEventType::ExecutionRequested,
            request,
        ))
        .await;
    }

    /// Log execution result
    pub async fn log_execution_result(&self, result: &ExecutionResult) {
        self.log(AuditLogEntry::from_result(result)).await;
    }

    /// Log rejection with reason
    pub async fn log_rejection(
        &self,
        event_type: AuditEventType,
        request: &ExecutionRequest,
        reason: &str,
    ) {
        self.log(
            AuditLogEntry::from_request(event_type, request)
                .with_reason(reason),
        )
        .await;
    }

    /// Log kill switch event
    pub async fn log_kill_switch(&self, activated: bool, reason: Option<&str>) {
        let event_type = if activated {
            AuditEventType::KillSwitchActivated
        } else {
            AuditEventType::KillSwitchDeactivated
        };

        let mut entry = AuditLogEntry::new(event_type);
        if let Some(r) = reason {
            entry = entry.with_reason(r);
        }

        self.log(entry).await;
    }

    /// Log daily loss warning/exceeded
    pub async fn log_daily_loss(&self, pnl: f64, limit: f64, exceeded: bool) {
        let event_type = if exceeded {
            AuditEventType::DailyLossExceeded
        } else {
            AuditEventType::DailyLossWarning
        };

        let entry = AuditLogEntry::new(event_type).with_metadata(serde_json::json!({
            "daily_pnl": pnl,
            "limit": limit,
            "utilization_pct": (-pnl / limit * 100.0).round(),
        }));

        self.log(entry).await;
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(None, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditLogEntry::new(AuditEventType::ExecutionRequested)
            .with_reason("test reason");

        let json = entry.to_json();
        assert!(json.contains("execution_requested"));
        assert!(json.contains("test reason"));
    }

    #[test]
    fn test_from_result_event_type_mapping() {
        use arbees_rust_core::models::{ExecutionSide, Sport};

        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            idempotency_key: "key-1".to_string(),
            status: ExecutionStatus::Filled,
            rejection_reason: None,
            order_id: Some("order-1".to_string()),
            filled_qty: 10.0,
            avg_price: 0.50,
            fees: 0.07,
            platform: Platform::Kalshi,
            market_id: "market-1".to_string(),
            contract_team: None,
            game_id: "game-1".to_string(),
            sport: Sport::NBA,
            signal_id: "signal-1".to_string(),
            signal_type: "test".to_string(),
            edge_pct: 5.0,
            side: ExecutionSide::Yes,
            requested_at: Utc::now(),
            executed_at: Utc::now(),
            latency_ms: 100.0,
        };

        let entry = AuditLogEntry::from_result(&result);
        assert_eq!(entry.event_type, AuditEventType::OrderFilled);
    }
}
