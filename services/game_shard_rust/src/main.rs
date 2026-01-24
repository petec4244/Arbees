mod shard;

use anyhow::Result;
use dotenv::dotenv;
use log::info;
use shard::GameShard;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting GameShard Rust Service...");

    let shard_id = env::var("SHARD_ID").unwrap_or_else(|_| "default_shard".to_string());
    let shard = GameShard::new(shard_id).await?;

    shard.start().await?;

    // Keep running
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
