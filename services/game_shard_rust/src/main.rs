//! GameShard Rust Service
//!
//! Live game monitoring and arbitrage signal generation for sports markets.
//!
//! This service:
//! - Monitors live sports games via ESPN API
//! - Ingests prediction market prices from Kalshi and Polymarket
//! - Calculates win probabilities based on game state
//! - Detects arbitrage opportunities between markets
//! - Emits trading signals to signal_processor_rust
//!
//! **SPORTS-ONLY**: Crypto/economics/politics markets are handled by crypto_shard_rust

mod event_monitor;
mod shard;

// Shared modules
mod types;
mod config;

// Submodules
mod price;
mod signals;
mod monitoring;

use anyhow::Result;
use dotenv::dotenv;
use log::info;
use shard::GameShard;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting GameShard Rust Service (SPORTS-ONLY)...");

    let shard_id = env::var("SHARD_ID").unwrap_or_else(|_| "default_shard".to_string());
    let mut shard = GameShard::new(shard_id).await?;

    shard.start().await?;

    // Keep running
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
