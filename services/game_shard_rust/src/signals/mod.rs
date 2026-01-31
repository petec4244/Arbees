//! Signal generation and publishing (ZMQ-only)
//!
//! Modules:
//! - `edge`: Edge calculation and fee handling
//! - `emission`: Core ZMQ signal publishing
//! - `arbitrage`: Cross-platform arbitrage signal detection and emission
//! - `model_edge`: Model-based signal detection and emission
//! - `latency`: Score-change latency signal detection (disabled by default)

pub mod edge;
pub mod emission;
pub mod arbitrage;
pub mod model_edge;
pub mod latency;
