//! Price ingestion and management
//!
//! Modules:
//! - `data`: MarketPriceData types
//! - `matching`: Team matching and platform selection (Phase 1)
//! - `listener`: Unified ZMQ price listener (Phase 4)

pub mod data;
pub mod matching;
pub mod listener;
