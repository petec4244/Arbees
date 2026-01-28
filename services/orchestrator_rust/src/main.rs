mod clients;
mod config;
mod managers;
mod providers;
mod state;

use crate::clients::team_matching::TeamMatchingClient;
use crate::config::Config;
use crate::managers::game_manager::GameManager;
use crate::managers::kalshi_discovery::KalshiDiscoveryManager;
use crate::managers::multi_market::MultiMarketManager;
use crate::managers::service_registry::ServiceRegistry;
use crate::managers::shard_manager::ShardManager;
use anyhow::{Context, Result};
use arbees_rust_core::redis::bus::RedisBus;
use arbees_rust_core::clients::kalshi::KalshiClient;
use dotenv::dotenv;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    info!("Starting Rust Orchestrator Service...");

    // Config
    let config = Config::from_env();

    // Redis
    let redis_client = redis::Client::open(config.redis_url.clone())?;
    let redis_bus = Arc::new(RedisBus::new().await?);

    // Database
    let db = sqlx::PgPool::connect(&config.database_url)
        .await
        .context("Failed to connect to database")?;

    // Clients
    let team_matching = TeamMatchingClient::new(&config.redis_url)
        .await
        .context("Failed to initialize TeamMatchingClient")?;

    let kalshi_client = KalshiClient::new()?;

    // Managers
    // Initialize ServiceRegistry for fault tolerance
    let service_registry = Arc::new(ServiceRegistry::new(redis_bus.clone(), config.clone()));
    let shard_manager = Arc::new(ShardManager::new(service_registry.clone(), config.clone()));
    let kalshi_manager = Arc::new(KalshiDiscoveryManager::new(kalshi_client, team_matching));

    let game_manager = Arc::new(GameManager::new(
        redis_client.clone(),
        shard_manager.clone(),
        kalshi_manager.clone(),
        db.clone(),
        config.clone(),
    ));

    // Multi-market manager (for crypto, economics, politics)
    let multi_market_manager = if config.has_multi_market_enabled() {
        info!(
            "Multi-market enabled: crypto={}, economics={}, politics={}",
            config.enable_crypto_markets,
            config.enable_economics_markets,
            config.enable_politics_markets
        );
        Some(Arc::new(MultiMarketManager::new(
            redis_client.clone(),
            shard_manager.clone(),
            config.clone(),
        )))
    } else {
        info!("Multi-market discovery disabled");
        None
    };

    // Tasks
    let mut tasks = Vec::new();

    // 1. Discovery Loop
    let gm_clone = game_manager.clone();
    let interval_secs = config.discovery_interval_secs;
    tasks.push(tokio::spawn(async move {
        info!("Discovery loop started (interval: {}s)", interval_secs);
        loop {
            gm_clone.run_discovery_cycle().await;
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        }
    }));

    // 2. Shard Monitor (Heartbeats)
    let sm_clone = shard_manager.clone();
    let gm_clone_hb = game_manager.clone();
    let redis_url = config.redis_url.clone();
    tasks.push(tokio::spawn(async move {
        match redis::Client::open(redis_url) {
            Ok(client) => match client.get_async_connection().await {
                Ok(conn) => {
                    let mut pubsub = conn.into_pubsub();
                    if let Err(e) = pubsub.psubscribe("shard:*:heartbeat").await {
                        error!("Failed to subscribe to heartbeats: {}", e);
                        return;
                    }
                    info!("Listening for shard heartbeats...");
                    let mut stream = pubsub.on_message();
                    while let Some(msg) = stream.next().await {
                        if let Ok(payload_str) = msg.get_payload::<String>() {
                            if let Ok(payload) =
                                serde_json::from_str::<serde_json::Value>(&payload_str)
                            {
                                sm_clone.handle_heartbeat(payload.clone()).await;
                                gm_clone_hb.handle_shard_heartbeat(payload).await;
                            }
                        }
                    }
                }
                Err(e) => error!("Redis connection error in shard monitor: {}", e),
            },
            Err(e) => error!("Redis client error: {}", e),
        }
    }));

    // 3. Market Discovery Listener
    let gm_clone2 = game_manager.clone();
    let redis_url2 = config.redis_url.clone();
    tasks.push(tokio::spawn(async move {
        match redis::Client::open(redis_url2) {
            Ok(client) => match client.get_async_connection().await {
                Ok(conn) => {
                    let mut pubsub = conn.into_pubsub();
                    if let Err(e) = pubsub.subscribe("discovery:results").await {
                        error!("Failed to subscribe to discovery results: {}", e);
                        return;
                    }
                    info!("Listening for discovery results...");
                    let mut stream = pubsub.on_message();
                    while let Some(msg) = stream.next().await {
                        if let Ok(payload_str) = msg.get_payload::<String>() {
                            if let Ok(payload) = serde_json::from_str(&payload_str) {
                                gm_clone2.handle_discovery_result(payload).await;
                            }
                        }
                    }
                }
                Err(e) => error!("Redis connection error in market listener: {}", e),
            },
            Err(e) => error!("Redis client error: {}", e),
        }
    }));

    // 4. Scheduled Sync Loop
    let gm_clone3 = game_manager.clone();
    let sync_interval = config.scheduled_sync_interval_secs;
    tasks.push(tokio::spawn(async move {
        info!("Scheduled sync loop started (interval: {}s)", sync_interval);
        // Initial sync
        gm_clone3.run_scheduled_sync().await;

        loop {
            tokio::time::sleep(Duration::from_secs(sync_interval)).await;
            gm_clone3.run_scheduled_sync().await;
        }
    }));

    // 5. Health Check Loop
    let sm_clone2 = shard_manager.clone();
    let health_interval = config.health_check_interval_secs;
    tasks.push(tokio::spawn(async move {
        info!("Health check loop started");
        loop {
            sm_clone2.check_health().await;
            tokio::time::sleep(Duration::from_secs(health_interval)).await;
        }
    }));

    // 6. Service Resync Loop (for fault tolerance)
    let sr_clone = service_registry.clone();
    let gm_clone4 = game_manager.clone();
    tasks.push(tokio::spawn(async move {
        info!("Service resync loop started");
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            sr_clone.process_pending_resyncs(gm_clone4.get_assignments()).await;
        }
    }));

    // 7. Multi-Market Discovery Loop (for crypto, economics, politics)
    if let Some(mm_manager) = multi_market_manager.clone() {
        let mm_interval = config.multi_market_discovery_interval_secs;
        tasks.push(tokio::spawn(async move {
            info!(
                "Multi-market discovery loop started (interval: {}s)",
                mm_interval
            );
            loop {
                mm_manager.run_discovery_cycle().await;
                tokio::time::sleep(Duration::from_secs(mm_interval)).await;
            }
        }));
    }

    // 8. Multi-Market Heartbeat Handler
    if let Some(mm_manager) = multi_market_manager {
        let redis_url3 = config.redis_url.clone();
        tasks.push(tokio::spawn(async move {
            match redis::Client::open(redis_url3) {
                Ok(client) => match client.get_async_connection().await {
                    Ok(conn) => {
                        let mut pubsub = conn.into_pubsub();
                        if let Err(e) = pubsub.psubscribe("shard:*:heartbeat").await {
                            error!("Failed to subscribe to heartbeats for multi-market: {}", e);
                            return;
                        }
                        info!("Multi-market manager listening for shard heartbeats...");
                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            if let Ok(payload_str) = msg.get_payload::<String>() {
                                if let Ok(payload) =
                                    serde_json::from_str::<serde_json::Value>(&payload_str)
                                {
                                    mm_manager.handle_shard_heartbeat(payload).await;
                                }
                            }
                        }
                    }
                    Err(e) => error!("Redis connection error in multi-market heartbeat: {}", e),
                },
                Err(e) => error!("Redis client error: {}", e),
            }
        }));
    }

    // Wait for signal
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            info!("Received shutdown signal");
        }
        Err(err) => {
            error!("Unable to listen for shutdown signal: {}", err);
        }
    }

    Ok(())
}
