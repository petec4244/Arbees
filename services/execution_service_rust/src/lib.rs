//! Execution Service Library
//!
//! This module exposes the execution engine and safeguard components for testing purposes.

pub mod audit;
pub mod balance;
pub mod config;
pub mod engine;
pub mod idempotency;
pub mod kill_switch;
pub mod polymarket_executor;
pub mod rate_limiter;

// Re-export commonly used types
pub use audit::{AuditEventType, AuditLogEntry, AuditLogger};
pub use balance::{BalanceCache, DailyPnlTracker};
pub use config::SafeguardConfig;
pub use engine::{ExecutionEngine, RejectionReason};
pub use idempotency::{IdempotencyResult, IdempotencyTracker};
pub use kill_switch::{KillSwitch, KillSwitchReason};
pub use rate_limiter::{RateLimitExceeded, RateLimiter};
