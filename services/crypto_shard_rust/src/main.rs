use anyhow::Result;
use crypto_shard_rust::{CryptoShard, CryptoShardConfig};
use log::info;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    info!("Starting crypto_shard_rust...");

    let config = CryptoShardConfig::from_env()?;
    let mut shard = CryptoShard::new(config).await?;

    shard.run().await
}
