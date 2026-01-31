//! crypto_shard_rust - Self-contained crypto arbitrage and probability-based trading

pub mod config;
pub mod types;
pub mod shard;
pub mod price;
pub mod signals;
pub mod monitoring;
pub mod db;

pub use shard::CryptoShard;
pub use config::CryptoShardConfig;
pub use types::{CryptoEventContext, CryptoExecutionRequest};
