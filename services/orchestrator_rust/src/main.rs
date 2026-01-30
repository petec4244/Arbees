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
use redis::AsyncCommands;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};
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

    // Database with standardized pool configuration
    let db = arbees_rust_core::db::create_pool(
        &config.database_url,
        arbees_rust_core::db::DbPoolConfig::default(),
    )
    .await
    .context("Failed to create database pool")?;

    // Start database health monitoring
    let health_monitor = arbees_rust_core::db::PoolHealthMonitor::new(
        db.clone(),
        arbees_rust_core::db::PoolHealthConfig::default(),
    );
    let _health_handle = health_monitor.start_background();
    info!("Database health monitoring started");

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
            service_registry.clone(),
            config.clone(),
        )))
    } else {
        info!("Multi-market discovery disabled");
        None
    };

    // System monitor for critical alerts
    let alert_client = arbees_rust_core::alerts::CriticalAlertClient::from_env();
    let system_monitor = Arc::new(crate::managers::system_monitor::SystemMonitor::new(
        service_registry.clone(),
        redis_bus.clone(),
        db.clone(),
        alert_client,
    ));
    info!("System monitor initialized with critical alerting");

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

    // 2. Shard Monitor (Heartbeats) - with auto-reconnect
    let sm_clone = shard_manager.clone();
    let gm_clone_hb = game_manager.clone();
    let redis_bus_clone = redis_bus.clone();
    tasks.push(tokio::spawn(async move {
        info!("Listening for shard heartbeats (auto-reconnect enabled)...");
        let mut stream = redis_bus_clone
            .psubscribe_with_reconnect("shard:*:heartbeat".to_string())
            .into_message_stream();

        while let Some(msg) = stream.next().await {
            if let Ok(payload_str) = msg.get_payload::<String>() {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                    sm_clone.handle_heartbeat(payload.clone()).await;
                    gm_clone_hb.handle_shard_heartbeat(payload).await;
                }
            }
        }
        // This loop now continues even after disconnects (reconnects automatically)
        error!("Shard heartbeat listener exited unexpectedly");
    }));

    // 3. Monitor Heartbeats (kalshi_monitor, polymarket_monitor) - with auto-reconnect
    let sr_clone_hb = service_registry.clone();
    let redis_bus_clone_hb = redis_bus.clone();
    tasks.push(tokio::spawn(async move {
        info!("Listening for monitor heartbeats (auto-reconnect enabled)...");
        let mut stream = redis_bus_clone_hb
            .subscribe_with_reconnect(vec!["health:heartbeats".to_string()])
            .into_message_stream();

        while let Some(msg) = stream.next().await {
            if let Ok(payload_str) = msg.get_payload::<String>() {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                    if let Err(e) = sr_clone_hb.handle_heartbeat(payload).await {
                        error!("Failed to handle monitor heartbeat: {}", e);
                    }
                }
            }
        }
        error!("Monitor heartbeat listener exited unexpectedly");
    }));

    // 4. Market Discovery Listener - with auto-reconnect
    let gm_clone2 = game_manager.clone();
    let redis_bus_clone2 = redis_bus.clone();
    tasks.push(tokio::spawn(async move {
        info!("Listening for discovery results (auto-reconnect enabled)...");
        let mut stream = redis_bus_clone2
            .subscribe_with_reconnect(vec!["discovery:results".to_string()])
            .into_message_stream();

        while let Some(msg) = stream.next().await {
            if let Ok(payload_str) = msg.get_payload::<String>() {
                if let Ok(payload) = serde_json::from_str(&payload_str) {
                    gm_clone2.handle_discovery_result(payload).await;
                }
            }
        }
        error!("Market discovery listener exited unexpectedly");
    }));

    // 5. Scheduled Sync Loop
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

    // 6. Health Check Loop
    let sm_clone2 = shard_manager.clone();
    let health_interval = config.health_check_interval_secs;
    tasks.push(tokio::spawn(async move {
        info!("Health check loop started");
        loop {
            sm_clone2.check_health().await;
            tokio::time::sleep(Duration::from_secs(health_interval)).await;
        }
    }));

    // 7. Service Resync Loop (for fault tolerance)
    let sr_clone = service_registry.clone();
    let gm_clone4 = game_manager.clone();
    tasks.push(tokio::spawn(async move {
        info!("Service resync loop started");
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            sr_clone.process_pending_resyncs(gm_clone4.get_assignments()).await;
        }
    }));

    // Store multi_market_manager for reuse
    let mm_clone_discovery = multi_market_manager.clone();
    let mm_clone_heartbeat = multi_market_manager.clone();
    let mm_clone_startup = multi_market_manager.clone();

    // 8. Multi-Market Discovery Loop (for crypto, economics, politics)
    if let Some(mm_manager) = mm_clone_discovery {
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

    // 9. Multi-Market Heartbeat Handler - with auto-reconnect
    if let Some(mm_manager) = mm_clone_heartbeat {
        let redis_bus_clone3 = redis_bus.clone();
        tasks.push(tokio::spawn(async move {
            info!("Multi-market manager listening for shard heartbeats (auto-reconnect enabled)...");
            let mut stream = redis_bus_clone3
                .psubscribe_with_reconnect("shard:*:heartbeat".to_string())
                .into_message_stream();

            while let Some(msg) = stream.next().await {
                if let Ok(payload_str) = msg.get_payload::<String>() {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                        mm_manager.handle_shard_heartbeat(payload).await;
                    }
                }
            }
            error!("Multi-market heartbeat listener exited unexpectedly");
        }));
    }

    // 10. System Monitor - Health checks and critical alerts
    let system_monitor_clone = system_monitor.clone();
    let monitor_interval = 30; // Check every 30 seconds
    tasks.push(tokio::spawn(async move {
        info!("System monitor loop started (interval: {}s)", monitor_interval);
        loop {
            system_monitor_clone.check_system_health().await;
            tokio::time::sleep(Duration::from_secs(monitor_interval)).await;
        }
    }));

    // 11. Startup State Request Handler - Service restart recovery
    let gm_clone_startup = game_manager.clone();
    let redis_bus_startup = redis_bus.clone();
    tasks.push(tokio::spawn(async move {
        info!("Startup state request handler initialized (auto-reconnect enabled)...");
        let mut stream = redis_bus_startup
            .subscribe_with_reconnect(vec!["orchestrator:startup_state_request".to_string()])
            .into_message_stream();

        while let Some(msg) = stream.next().await {
            debug!("Received message on startup_state_request stream");
            if let Ok(payload_str) = msg.get_payload::<String>() {
                info!("Parsed startup state request payload: {}", payload_str);
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                    // Get monitor_type from request
                    if let Some(monitor_type) = payload.get("monitor_type").and_then(|v| v.as_str()) {
                        let response_channel = format!("orchestrator:startup_state_response:{}", monitor_type);
                        info!("Received startup state request from monitor: {}", monitor_type);

                        // Collect assignments for the requesting monitor type
                        let mut assignments_map = serde_json::Map::new();

                        match monitor_type {
                            "kalshi" => {
                                // For Kalshi: send all multi-market assignments with kalshi_market_id
                                if let Some(mm) = &mm_clone_startup {
                                    let assignments_lock = mm.get_assignments();
                                    let assignments = assignments_lock.read().await;
                                    let kalshi_markets: Vec<_> = assignments
                                        .values()
                                        .filter(|a| a.kalshi_market_id.is_some())
                                        .cloned()
                                        .collect();

                                    info!(
                                        "Responding to Kalshi startup state request with {} markets",
                                        kalshi_markets.len()
                                    );
                                    if let Ok(json_value) = serde_json::to_value(&kalshi_markets) {
                                        assignments_map.insert("markets".to_string(), json_value);
                                    }
                                } else {
                                    info!("Multi-market manager not available for Kalshi request");
                                }
                            }
                            "polymarket" => {
                                // For Polymarket: send all multi-market assignments with polymarket_market_id
                                if let Some(mm) = &mm_clone_startup {
                                    let assignments_lock = mm.get_assignments();
                                    let assignments = assignments_lock.read().await;
                                    let polymarket_markets: Vec<_> = assignments
                                        .values()
                                        .filter(|a| a.polymarket_market_id.is_some())
                                        .cloned()
                                        .collect();

                                    info!(
                                        "Responding to Polymarket startup state request with {} markets",
                                        polymarket_markets.len()
                                    );
                                    if let Ok(json_value) = serde_json::to_value(&polymarket_markets) {
                                        assignments_map.insert("markets".to_string(), json_value);
                                    }
                                } else {
                                    info!("Multi-market manager not available for Polymarket request");
                                }
                            }
                            "game_shard" => {
                                // For game_shard: send sports market assignments
                                let assignments_lock = gm_clone_startup.get_assignments();
                                let assignments = assignments_lock.read().await;
                                if !assignments.is_empty() {
                                    debug!(
                                        "Responding to game_shard startup state request with {} games",
                                        assignments.len()
                                    );
                                    let games: Vec<_> = assignments.values().cloned().collect();
                                    if let Ok(json_value) = serde_json::to_value(&games) {
                                        assignments_map.insert("games".to_string(), json_value);
                                    }
                                }
                            }
                            _ => {
                                debug!("Unknown monitor type for startup state request: {}", monitor_type);
                            }
                        }

                        // Publish response
                        if let Ok(response_json) = serde_json::to_string(&assignments_map) {
                            let mut conn = match redis_bus_startup.get_connection().await {
                                Ok(c) => c,
                                Err(e) => {
                                    error!("Failed to get Redis connection for startup state response: {}", e);
                                    continue;
                                }
                            };

                            match conn.publish::<&str, &str, ()>(&response_channel, &response_json).await {
                                Ok(_) => {
                                    info!(
                                        "Published startup state response to channel: {}",
                                        response_channel
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to publish startup state response to {}: {}", response_channel, e);
                                }
                            }
                        } else {
                            error!("Failed to serialize startup state response for {}", monitor_type);
                        }
                    }
                }
            }
        }
        error!("Startup state request handler exited unexpectedly");
    }));

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
