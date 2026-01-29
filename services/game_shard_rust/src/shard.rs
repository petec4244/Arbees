use anyhow::Result;
use arbees_rust_core::circuit_breaker::ApiCircuitBreaker;
use arbees_rust_core::clients::espn::{EspnClient, Game as EspnGame};
use arbees_rust_core::models::{
    FootballState, GameState, MarketType, Platform, SignalDirection, Sport,
    SportSpecificState, TransportMode,
};
use arbees_rust_core::ProbabilityModelRegistry;
use arbees_rust_core::providers::EventProviderRegistry;
use arbees_rust_core::redis::bus::RedisBus;
use arbees_rust_core::win_prob::{batch_calculate_win_probs, calculate_win_probability};
use chrono::Utc;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use zeromq::{Socket, SocketRecv, SubSocket, PubSocket};

// Import from internal modules
use crate::types::{GameContext, GameEntry, PriceListenerStats, PriceListenerStatsSnapshot, ShardCommand, ZmqEnvelope};
use crate::config::{GameMonitorConfig, load_espn_circuit_breaker_config, load_database_url, load_zmq_sub_endpoints, load_zmq_pub_port};
use crate::price::data::{MarketPriceData, IncomingMarketPrice};
use crate::signals::edge::compute_team_net_edge;
use crate::signals::arbitrage;
use crate::signals::model_edge;
use crate::signals::latency;
use crate::monitoring::espn::{parse_sport, is_overtime, format_time_remaining, check_cross_platform_arb, espn_sport_league};
use crate::price::matching::{find_team_prices, select_best_platform_for_team};
use crate::price::listener::PriceListener;

// Type definitions moved to types.rs
// Configuration constants moved to config.rs


#[derive(Clone)]
pub struct GameShard {
    shard_id: String,
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    games: Arc<Mutex<HashMap<String, GameEntry>>>,
    /// Shared market prices: game_id -> (team|platform -> MarketPrice)
    /// Key format for inner map: "{team}|{platform}" to support prices from multiple platforms
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    max_games: usize,
    min_edge_pct: f64,
    /// Statistics for monitoring price message processing
    price_stats: Arc<PriceListenerStats>,
    /// Circuit breaker for ESPN API calls
    espn_circuit_breaker: Arc<ApiCircuitBreaker>,
    /// Transport mode configuration (kept for event_monitor compatibility)
    transport_mode: TransportMode,
    /// ZMQ PUB socket for signals (wrapped in Arc<Mutex> for sharing)
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    /// ZMQ sequence number for message ordering
    zmq_seq: Arc<AtomicU64>,
    /// ZMQ endpoints to subscribe to for prices
    zmq_sub_endpoints: Vec<String>,
    /// ZMQ PUB port for signals
    zmq_pub_port: u16,
    /// Process identity for restart detection (UUID generated at startup)
    process_id: String,
    /// Process start time (unchanging, for restart detection)
    started_at: chrono::DateTime<Utc>,
    /// Probability model registry for edge calculation (multi-market support)
    probability_registry: Arc<ProbabilityModelRegistry>,
    /// Event provider registry for fetching event state (shared across all events)
    event_provider_registry: Arc<EventProviderRegistry>,
}




impl GameShard {
    pub async fn new(shard_id: String) -> Result<Self> {
        let redis = RedisBus::new().await?;
        let espn = EspnClient::new();

        // Create database pool
        let database_url = load_database_url();
        let db_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        info!("Connected to database");

        // Load configuration
        let config = GameMonitorConfig::from_env();

        // ESPN API circuit breaker configuration
        let espn_circuit_breaker = Arc::new(ApiCircuitBreaker::new(
            "espn_api",
            load_espn_circuit_breaker_config(),
        ));
        info!(
            "ESPN circuit breaker configured: failure_threshold={}, recovery_timeout={}s",
            espn_circuit_breaker.failure_count(),
            30 // Can't easily get config back, log default
        );

        // Transport mode configuration
        let transport_mode = TransportMode::from_env();

        let zmq_sub_endpoints = load_zmq_sub_endpoints();
        let zmq_pub_port = load_zmq_pub_port();

        // Generate process identity for restart detection
        let process_id = Uuid::new_v4().to_string();
        let started_at = Utc::now();

        // Initialize probability model registry for multi-market support
        let probability_registry = Arc::new(ProbabilityModelRegistry::new());

        // Initialize event provider registry once (shared across all events)
        // This creates HTTP clients for ESPN, CoinGecko, Binance, Coinbase, etc.
        let event_provider_registry = Arc::new(EventProviderRegistry::with_defaults());
        info!(
            "Event provider registry initialized with {} providers",
            event_provider_registry.list_providers().len()
        );

        Ok(Self {
            shard_id,
            redis,
            espn,
            db_pool,
            games: Arc::new(Mutex::new(HashMap::new())),
            market_prices: Arc::new(RwLock::new(HashMap::new())),
            poll_interval: config.poll_interval,
            heartbeat_interval: config.heartbeat_interval,
            max_games: config.max_games,
            min_edge_pct: config.min_edge_pct,
            price_stats: Arc::new(PriceListenerStats::default()),
            espn_circuit_breaker,
            transport_mode,
            zmq_pub: None, // Initialized in start()
            zmq_seq: Arc::new(AtomicU64::new(0)),
            zmq_sub_endpoints,
            zmq_pub_port,
            process_id,
            started_at,
            probability_registry,
            event_provider_registry,
        })
    }

    /// Get a snapshot of price listener statistics for monitoring
    pub fn get_price_stats(&self) -> PriceListenerStatsSnapshot {
        self.price_stats.snapshot()
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("Starting GameShard {}", self.shard_id);

        // Initialize ZMQ PUB socket for signals if enabled
        // ZMQ publishing (always enabled - ZMQ-only transport mode)
        match Self::init_zmq_pub(self.zmq_pub_port).await {
            Ok(pub_socket) => {
                self.zmq_pub = Some(Arc::new(Mutex::new(pub_socket)));
                info!("ZMQ PUB socket bound to port {}", self.zmq_pub_port);
            }
            Err(e) => {
                error!("Failed to initialize ZMQ PUB socket: {}", e);
                // Continue without ZMQ PUB
            }
        }

        // Heartbeat loop
        let heartbeat_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = heartbeat_shard.heartbeat_loop().await {
                error!("Heartbeat loop exited: {}", e);
            }
        });

        // Command loop (receives game assignments)
        let command_shard = self.clone();
        tokio::spawn(async move {
            if let Err(e) = command_shard.command_loop().await {
                error!("Command loop exited: {}", e);
            }
        });

        // Unified ZMQ price listener (primary, low-latency)
        if !self.zmq_sub_endpoints.is_empty() {
            let listener = PriceListener::new(
                self.zmq_sub_endpoints.clone(),
                self.market_prices.clone(),
                self.price_stats.clone(),
            );
            tokio::spawn(async move {
                if let Err(e) = listener.start().await {
                    error!("Unified ZMQ price listener exited: {}", e);
                }
            });
            info!("Started unified ZMQ price listener for endpoints: {:?}", self.zmq_sub_endpoints);
        }

        Ok(())
    }

    /// Initialize ZMQ PUB socket for publishing signals
    async fn init_zmq_pub(port: u16) -> Result<PubSocket> {
        let mut socket = PubSocket::new();
        // Use 0.0.0.0 instead of * for zeromq-rs compatibility
        let addr = format!("tcp://0.0.0.0:{}", port);
        socket.bind(&addr).await?;
        Ok(socket)
    }

    pub async fn add_game(
        &self,
        game_id: String,
        sport: String,
        polymarket_id: Option<String>,
        kalshi_id: Option<String>,
    ) -> Result<()> {
        info!("Adding game: {} ({})", game_id, sport);

        let mut games = self.games.lock().await;
        if games.contains_key(&game_id) {
            warn!("Game already tracked: {}", game_id);
            return Ok(());
        }

        let context = GameContext {
            game_id: game_id.clone(),
            sport: sport.clone(),
            market_type: None,  // Will be set to Sport by monitor_game based on sport string
            entity_a: None,     // Will be set to home_team by monitor_game
            entity_b: None,     // Will be set to away_team by monitor_game
            polymarket_id,
            kalshi_id,
        };

        let last_prob = Arc::new(RwLock::new(None));
        let last_prob_clone = last_prob.clone();
        let redis = self.redis.clone();
        let espn = self.espn.clone();
        let db_pool = self.db_pool.clone();
        let poll_interval = self.poll_interval;
        let market_prices = self.market_prices.clone();
        let min_edge = self.min_edge_pct;
        let espn_cb = self.espn_circuit_breaker.clone();
        let gid = game_id.clone();
        let sp = sport.clone();
        let zmq_pub = self.zmq_pub.clone();
        let zmq_seq = self.zmq_seq.clone();
        let transport_mode = self.transport_mode;

        let task = tokio::spawn(async move {
            monitor_game(
                redis,
                espn,
                db_pool,
                gid,
                sp,
                poll_interval,
                last_prob_clone,
                market_prices,
                min_edge,
                espn_cb,
                zmq_pub,
                zmq_seq,
                transport_mode,
            )
            .await;
        });

        games.insert(
            context.game_id.clone(),
            GameEntry {
                context,
                task,
                last_home_win_prob: last_prob,
                opening_home_prob: Arc::new(RwLock::new(None)),
            },
        );

        Ok(())
    }

    /// Add a universal event (crypto, economics, politics) to be monitored
    pub async fn add_event(
        &self,
        event_id: String,
        market_type: MarketType,
        entity_a: String,
        entity_b: Option<String>,
        polymarket_id: Option<String>,
        kalshi_id: Option<String>,
    ) -> Result<()> {
        info!(
            "Adding event: {} ({:?}) entity_a={}, entity_b={:?}",
            event_id,
            market_type.type_name(),
            entity_a,
            entity_b
        );

        let mut games = self.games.lock().await;
        if games.contains_key(&event_id) {
            warn!("Event already tracked: {}", event_id);
            return Ok(());
        }

        let context = GameContext {
            game_id: event_id.clone(),
            sport: market_type.type_name().to_string(),
            market_type: Some(market_type.clone()),
            entity_a: Some(entity_a.clone()),
            entity_b: entity_b.clone(),
            polymarket_id,
            kalshi_id,
        };

        // Use shared provider registry (created once at shard startup)
        let provider_registry = self.event_provider_registry.clone();

        // Create config from environment
        let config = crate::event_monitor::EventMonitorConfig::from_env();

        // Clone shared state for the monitoring task
        let redis = self.redis.clone();
        let db_pool = self.db_pool.clone();
        let market_prices = self.market_prices.clone();
        let zmq_pub = self.zmq_pub.clone();
        let zmq_seq = self.zmq_seq.clone();
        let probability_registry = self.probability_registry.clone();
        let eid = event_id.clone();
        let mt = market_type.clone();
        let ea = entity_a.clone();
        let eb = entity_b.clone();
        let transport_mode = self.transport_mode;

        // Spawn the monitoring task
        let last_prob = Arc::new(RwLock::new(None));
        let task = tokio::spawn(async move {
            crate::event_monitor::monitor_event(
                redis,
                db_pool,
                eid,
                mt,
                ea,
                eb,
                config,
                provider_registry,
                probability_registry,
                market_prices,
                zmq_pub,
                zmq_seq,
                transport_mode,
            )
            .await;
        });

        games.insert(
            context.game_id.clone(),
            GameEntry {
                context,
                task,
                last_home_win_prob: last_prob,
                opening_home_prob: Arc::new(RwLock::new(None)),
            },
        );

        Ok(())
    }

    pub async fn remove_game(&self, game_id: String) -> Result<()> {
        info!("Removing game: {}", game_id);
        let mut games = self.games.lock().await;
        if let Some(entry) = games.remove(&game_id) {
            entry.task.abort();
        }
        // Also remove market prices
        let mut prices = self.market_prices.write().await;
        prices.remove(&game_id);
        Ok(())
    }

    async fn command_loop(&self) -> Result<()> {
        let channel = format!("shard:{}:command", self.shard_id);
        let mut pubsub = self.redis.subscribe(&channel).await?;
        info!("Subscribed to {}", channel);

        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let payload: Vec<u8> = match msg.get_payload::<Vec<u8>>() {
                Ok(p) => p,
                Err(e) => {
                    warn!("Command payload read error: {}", e);
                    continue;
                }
            };

            let command: ShardCommand = match serde_json::from_slice(&payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Command JSON parse error: {}", e);
                    continue;
                }
            };

            match command.command_type.as_str() {
                "add_game" => {
                    if let (Some(game_id), Some(sport)) =
                        (command.game_id.clone(), command.sport.clone())
                    {
                        info!(
                            "Received add_game: {} ({}) kalshi={:?} poly={:?}",
                            game_id, sport, command.kalshi_market_id, command.polymarket_market_id
                        );
                        if let Err(e) = self
                            .add_game(
                                game_id,
                                sport,
                                command.polymarket_market_id,
                                command.kalshi_market_id,
                            )
                            .await
                        {
                            error!("Failed to add_game: {}", e);
                        }
                    } else {
                        warn!("add_game command missing game_id or sport");
                    }
                }
                "remove_game" => {
                    if let Some(game_id) = command.game_id.clone() {
                        if let Err(e) = self.remove_game(game_id).await {
                            error!("Failed to remove_game: {}", e);
                        }
                    } else {
                        warn!("remove_game command missing game_id");
                    }
                }
                "add_event" => {
                    // Universal path for all market types (sports, crypto, economics, politics)
                    // Falls back to event_id if game_id not provided
                    let event_id = command.event_id.clone().or(command.game_id.clone());
                    if let (Some(event_id), Some(market_type)) = (event_id, command.market_type.clone()) {
                        info!(
                            "Received add_event: {} ({:?}) kalshi={:?} poly={:?}",
                            event_id, market_type, command.kalshi_market_id, command.polymarket_market_id
                        );

                        if market_type.is_sport() {
                            // Sports: use legacy monitor_game for backward compatibility
                            let sport = market_type.as_sport()
                                .map(|s| format!("{:?}", s).to_lowercase())
                                .unwrap_or_else(|| "nba".to_string());
                            if let Err(e) = self
                                .add_game(
                                    event_id,
                                    sport,
                                    command.polymarket_market_id,
                                    command.kalshi_market_id,
                                )
                                .await
                            {
                                error!("Failed to add_event (sport): {}", e);
                            }
                        } else {
                            // Non-sports: use new monitor_event
                            let entity_a = command.entity_a.clone().unwrap_or_else(|| event_id.clone());
                            let entity_b = command.entity_b.clone();
                            if let Err(e) = self
                                .add_event(
                                    event_id,
                                    market_type,
                                    entity_a,
                                    entity_b,
                                    command.polymarket_market_id,
                                    command.kalshi_market_id,
                                )
                                .await
                            {
                                error!("Failed to add_event (non-sport): {}", e);
                            }
                        }
                    } else {
                        warn!("add_event command missing event_id or market_type");
                    }
                }
                other => {
                    warn!("Unknown command type: {}", other);
                }
            }
        }

        Ok(())
    }

    /// Process an incoming price message (shared between Redis and ZMQ paths)
    async fn process_incoming_price(&self, price: IncomingMarketPrice) {
        self.price_stats.messages_received.fetch_add(1, Ordering::Relaxed);

        let game_id = price.game_id.clone();

        // Check if contract_team is present
        let team = match &price.contract_team {
            Some(t) => t,
            None => {
                self.price_stats.no_team_skipped.fetch_add(1, Ordering::Relaxed);
                debug!("Skipping price message without contract_team: game={}", game_id);
                return;
            }
        };

        // Skip prices with no real liquidity
        let has_liquidity = price.yes_bid > 0.01 || price.yes_ask < 0.99;
        if !has_liquidity {
            self.price_stats.no_liquidity_skipped.fetch_add(1, Ordering::Relaxed);
            debug!(
                "Skipping price with no liquidity: game={} team={} bid={} ask={}",
                game_id, team, price.yes_bid, price.yes_ask
            );
            return;
        }

        let mid = price
            .mid_price
            .or(price.implied_probability)
            .unwrap_or((price.yes_bid + price.yes_ask) / 2.0);

        let data = MarketPriceData {
            market_id: price.market_id.clone(),
            platform: price.platform.clone(),
            contract_team: team.clone(),
            yes_bid: price.yes_bid,
            yes_ask: price.yes_ask,
            mid_price: mid,
            timestamp: Utc::now(),
            yes_bid_size: price.yes_bid_size,
            yes_ask_size: price.yes_ask_size,
            total_liquidity: price.liquidity,
        };

        let mut prices = self.market_prices.write().await;
        let game_prices = prices.entry(game_id.clone()).or_insert_with(HashMap::new);
        let key = format!("{}|{}", team, price.platform.to_lowercase());
        game_prices.insert(key, data);

        self.price_stats.messages_processed.fetch_add(1, Ordering::Relaxed);
    }

    async fn heartbeat_loop(&self) -> Result<()> {
        let channel = format!("shard:{}:heartbeat", self.shard_id);
        loop {
            let (game_ids, count) = {
                let games = self.games.lock().await;
                let ids = games.keys().cloned().collect::<Vec<_>>();
                (ids, games.len())
            };

            // Determine status based on health checks
            // Redis is OK if we can successfully publish heartbeat (checked below)
            let redis_ok = true; // Will be checked implicitly by publish success
            let espn_ok = self.espn_circuit_breaker.is_available();
            let zmq_ok = self.zmq_pub.is_some();

            let status = if redis_ok && espn_ok {
                "healthy"
            } else if redis_ok {
                "degraded"
            } else {
                "unhealthy"
            };

            // Get build version from environment or use "dev"
            let version = env::var("BUILD_VERSION").unwrap_or_else(|_| "dev".to_string());

            // Shard type: "sports" (default) or "crypto"
            // Used by orchestrator to route markets to appropriate shards
            let shard_type = env::var("SHARD_TYPE").unwrap_or_else(|_| "sports".to_string());

            let payload = json!({
                "shard_id": self.shard_id,
                "shard_type": shard_type,
                "game_count": count,
                "max_games": self.max_games,
                "games": game_ids,
                "timestamp": Utc::now().to_rfc3339(),
                // NEW FIELDS for fault tolerance
                "started_at": self.started_at.to_rfc3339(),
                "process_id": self.process_id,
                "version": version,
                "status": status,
                "checks": {
                    "redis_ok": redis_ok,
                    "espn_api_ok": espn_ok,
                    "zmq_ok": zmq_ok,
                },
                "metrics": {
                    "max_games": self.max_games,
                },
            });

            if let Err(e) = self.redis.publish(&channel, &payload).await {
                warn!("Heartbeat publish error: {}", e);
            }

            tokio::time::sleep(self.heartbeat_interval).await;
        }
    }
}

async fn monitor_game(
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    game_id: String,
    sport: String,
    poll_interval: Duration,
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    min_edge_pct: f64,
    espn_circuit_breaker: Arc<ApiCircuitBreaker>,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,
    transport_mode: TransportMode,
) {
    let sport_enum = match parse_sport(&sport) {
        Some(s) => s,
        None => {
            warn!("Unsupported sport: {}", sport);
            return;
        }
    };

    // Signal debouncing: (team, direction) -> last_signal_time
    let mut last_signal_times: HashMap<(String, String), Instant> = HashMap::new();
    let signal_debounce_secs: u64 = env::var("SIGNAL_DEBOUNCE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    // Price staleness TTL - synchronized with signal_processor and position_tracker
    let price_staleness_secs: i64 = env::var("PRICE_STALENESS_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    // Game state staleness TTL - should match or exceed price staleness
    // Default: 30 seconds (if ESPN data is older than 30s, skip signal generation)
    let game_state_staleness_secs: i64 = env::var("GAME_STATE_STALENESS_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    // Track previous score for latency-based signals
    let mut prev_home_score: Option<u16> = None;
    let mut prev_away_score: Option<u16> = None;
    let mut stale_state_warnings = 0u32;

    loop {
        // Fetch game state from ESPN (with circuit breaker protection)
        if let Some((game, state, home_win_prob)) =
            fetch_game_state(&espn, &game_id, &sport, &espn_circuit_breaker).await
        {
            // Check game state staleness - critical for preventing trades on old data
            let state_age_secs = (Utc::now() - state.fetched_at).num_seconds();
            if state_age_secs > game_state_staleness_secs {
                stale_state_warnings += 1;
                if stale_state_warnings % 10 == 1 {  // Log every 10th warning to avoid spam
                    warn!(
                        "Game state for {} is stale: {}s old (max {}s) - skipping signal generation. \
                         ESPN API might be slow or game data not updating.",
                        game_id, state_age_secs, game_state_staleness_secs
                    );
                }
                // Skip signal generation with stale game state
                tokio::time::sleep(poll_interval).await;
                continue;
            }
            // Reset warning counter on fresh state
            if stale_state_warnings > 0 {
                info!("Game state for {} is fresh again after {} stale warnings", game_id, stale_state_warnings);
                stale_state_warnings = 0;
            }

            // Detect score changes for latency-based signals
            let home_scored = prev_home_score.map_or(false, |p| game.home_score > p);
            let away_scored = prev_away_score.map_or(false, |p| game.away_score > p);
            let score_changed = home_scored || away_scored;

            // Update previous scores
            prev_home_score = Some(game.home_score);
            prev_away_score = Some(game.away_score);

            // Format time remaining as string
            let time_remaining_str = format_time_remaining(game.time_remaining_seconds);

            // Insert into database
            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO game_states (game_id, sport, home_score, away_score, period, time_remaining, status, possession, home_win_prob, time)
                VALUES ($1, $2::sport_enum, $3, $4, $5, $6, $7, $8, $9, NOW())
                "#,
            )
            .bind(&game.id)
            .bind(&sport.to_lowercase())
            .bind(game.home_score as i32)
            .bind(game.away_score as i32)
            .bind(game.period as i32)
            .bind(&time_remaining_str)
            .bind(&game.status)
            .bind(&game.possession)
            .bind(home_win_prob)
            .execute(&db_pool)
            .await
            {
                warn!("Database insert error: {}", e);
            }

            // Publish game state to Redis
            let state_channel = format!("game:{}:state", game_id);
            let state_json = json!({
                "game_id": game.id,
                "sport": sport,
                "name": game.name,
                "short_name": game.short_name,
                "scheduled_time": game.date,
                "home_team": game.home_team,
                "away_team": game.away_team,
                "home_abbr": game.home_abbr,
                "away_abbr": game.away_abbr,
                "home_score": game.home_score,
                "away_score": game.away_score,
                "period": game.period,
                "time_remaining": game.time_remaining_seconds,
                "status": game.status,
                "source": "espn_scoreboard",
                "timestamp": Utc::now().to_rfc3339(),
            });

            if let Err(e) = redis.publish(&state_channel, &state_json).await {
                warn!("Game state publish error: {}", e);
            }

            // Check for signals
            let old_prob = *last_home_win_prob.read().await;

            // Update last probability
            *last_home_win_prob.write().await = Some(home_win_prob);

            // Skip signal generation if game is not in progress
            if game.status != "STATUS_IN_PROGRESS" && game.status != "in" {
                continue;
            }

            // Skip signal generation if game is in overtime (too volatile)
            if is_overtime(sport_enum, game.period) {
                debug!("OVERTIME: Skipping signals for {} (period {})", game_id, game.period);
                continue;
            }

            // Skip signal generation if score is 0-0 (no real game information yet)
            // At 0-0, our model only has home advantage - not team strength
            // This leads to bad signals against favored away teams
            if game.home_score == 0 && game.away_score == 0 {
                debug!("SCORELESS: Skipping signals for {} (0-0)", game_id);
                continue;
            }

            // Get market prices for this game
            // FIX: Only emit ONE signal per game - the team with the strongest edge
            // This prevents betting on both teams to win the same game!
            let prices = market_prices.read().await;
            if let Some(game_prices) = prices.get(&game_id) {
                // Get prices from both platforms for each team (filtered by staleness)
                let (home_kalshi, home_poly) = find_team_prices(game_prices, &game.home_team, sport_enum, price_staleness_secs);
                let (away_kalshi, away_poly) = find_team_prices(game_prices, &game.away_team, sport_enum, price_staleness_secs);

                // ===== CROSS-PLATFORM ARBITRAGE CHECK (SIMD) =====
                // Check for arbitrage opportunities when we have prices from both platforms
                // Arbs are higher priority than model-edge signals (guaranteed profit)
                const MIN_ARB_PROFIT_CENTS: i16 = 1;

                // Debug: Log when we have both platforms for a team
                if home_kalshi.is_some() && home_poly.is_some() {
                    let hk = home_kalshi.unwrap();
                    let hp = home_poly.unwrap();
                    // Calculate the NO prices (what we'd need to buy for the other side)
                    let k_no = 1.0 - hk.yes_bid;
                    let p_no = 1.0 - hp.yes_bid;
                    debug!(
                        "ARB check {} {}: Kalshi YES={:.0}¢ NO={:.0}¢ (bid={:.2}) | Poly YES={:.0}¢ NO={:.0}¢ (bid={:.2}) | K+P_NO={:.0}¢ P+K_NO={:.0}¢",
                        game_id, game.home_team,
                        hk.yes_ask * 100.0, k_no * 100.0, hk.yes_bid,
                        hp.yes_ask * 100.0, p_no * 100.0, hp.yes_bid,
                        (hk.yes_ask + p_no) * 100.0,
                        (hp.yes_ask + k_no) * 100.0
                    );
                }

                if let Some((arb_mask, profit)) = check_cross_platform_arb(home_kalshi, home_poly, MIN_ARB_PROFIT_CENTS) {
                    let arb_key = (game.home_team.clone(), "arb".to_string());
                    let should_emit_arb = match last_signal_times.get(&arb_key) {
                        Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                        None => true,
                    };
                    if should_emit_arb {
                        if arbitrage::detect_and_emit(
                            &game_id, sport_enum, &game.home_team,
                            arb_mask, profit, home_kalshi.unwrap(), home_poly.unwrap(),
                            &zmq_pub, &zmq_seq
                        ).await {
                            last_signal_times.insert(arb_key, Instant::now());
                        }
                    }
                }
                // NOTE: We only check home team for arbitrage. In a two-outcome market,
                // away team arb is mathematically equivalent (TeamA YES = TeamB NO),
                // so checking both would emit duplicate signals for the same trade.
                // ===== END ARBITRAGE CHECK =====

                // ===== SCORE-CHANGE LATENCY SIGNALS (DISABLED) =====
                // DISABLED: ESPN scoreboard API is too slow - markets adjust before we see scores.
                // When we detect a score change, the market has already adjusted, so there's no edge.
                // To enable: Set LATENCY_SIGNALS_ENABLED=true (requires faster data source).
                let latency_signals_enabled = env::var("LATENCY_SIGNALS_ENABLED")
                    .ok()
                    .and_then(|v| v.parse::<bool>().ok())
                    .unwrap_or(false);

                if latency_signals_enabled && score_changed {
                    if home_scored {
                        // Home team scored → BUY home (their prob goes UP)
                        if let Some(price) = home_poly.or(home_kalshi) {
                            let platform = if home_poly.is_some() { Platform::Polymarket } else { Platform::Kalshi };
                            let signal_key = (game.home_team.clone(), "buy".to_string());
                            let should_emit = match last_signal_times.get(&signal_key) {
                                Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                                None => true,
                            };
                            if should_emit {
                                info!(
                                    "SCORE: {} scored! BUY at {:.1}% (latency edge, new model={:.1}%)",
                                    game.home_team,
                                    price.yes_ask * 100.0,
                                    home_win_prob * 100.0
                                );
                                if latency::detect_and_emit(
                                    &game_id, sport_enum, &game.home_team,
                                    SignalDirection::Buy, price, platform, home_win_prob,
                                    &zmq_pub, &zmq_seq
                                ).await {
                                    last_signal_times.insert(signal_key, Instant::now());
                                }
                            }
                        }
                    }
                    if away_scored {
                        // Away team scored → BUY away (their prob goes UP)
                        if let Some(price) = away_poly.or(away_kalshi) {
                            let platform = if away_poly.is_some() { Platform::Polymarket } else { Platform::Kalshi };
                            let signal_key = (game.away_team.clone(), "buy".to_string());
                            let should_emit = match last_signal_times.get(&signal_key) {
                                Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                                None => true,
                            };
                            if should_emit {
                                info!(
                                    "SCORE: {} scored! BUY at {:.1}% (latency edge, new model={:.1}%)",
                                    game.away_team,
                                    price.yes_ask * 100.0,
                                    (1.0 - home_win_prob) * 100.0
                                );
                                if latency::detect_and_emit(
                                    &game_id, sport_enum, &game.away_team,
                                    SignalDirection::Buy, price, platform, 1.0 - home_win_prob,
                                    &zmq_pub, &zmq_seq
                                ).await {
                                    last_signal_times.insert(signal_key, Instant::now());
                                }
                            }
                        }
                    }
                    // Skip model-based signals when we just had a score change
                    // The latency signal is our edge, not model vs market disagreement
                    continue;
                }
                // ===== END SCORE-CHANGE LATENCY SIGNALS =====

                let away_win_prob = 1.0 - home_win_prob;

                // ===== MODEL-BASED EDGE SIGNALS (DISABLED) =====
                // These are DISABLED because our model doesn't account for team strength.
                // When we disagree with the market, the market is usually right because
                // they know which teams are strong/weak, and we don't.
                // Only arbitrage signals (cross-platform) are enabled.
                let model_signals_enabled = env::var("MODEL_SIGNALS_ENABLED")
                    .ok()
                    .and_then(|v| v.parse::<bool>().ok())
                    .unwrap_or(false);

                if !model_signals_enabled {
                    // Skip model-based signals
                    continue;
                }

                // Select the best platform per-team using executable prices + fees.
                let home_best = select_best_platform_for_team(home_win_prob, home_kalshi, home_poly)
                    .map(|(hp, platform, net_edge)| {
                        (net_edge, hp, platform, home_win_prob, &game.home_team, old_prob)
                    });

                let away_best = select_best_platform_for_team(away_win_prob, away_kalshi, away_poly)
                    .map(|(ap, platform, net_edge)| {
                        (
                            net_edge,
                            ap,
                            platform,
                            away_win_prob,
                            &game.away_team,
                            old_prob.map(|p| 1.0 - p),
                        )
                    });

                // Determine which team has stronger *net* edge (already fee-adjusted).
                let stronger_signal = match (home_best, away_best) {
                    (Some(home), Some(away)) => {
                        if home.0 >= away.0 { Some(home) } else { Some(away) }
                    }
                    (Some(home), None) => Some(home),
                    (None, Some(away)) => Some(away),
                    (None, None) => None,
                };

                // Only emit signal for the team with stronger edge, using the best platform
                if let Some((net_edge, price, platform, prob, team, old_p)) = stronger_signal {
                    if net_edge >= min_edge_pct {
                        let (direction_enum, _ty, _net, _gross_abs, _mid) =
                            compute_team_net_edge(prob, price, platform);
                        let direction = match direction_enum {
                            SignalDirection::Buy => "buy",
                            SignalDirection::Sell => "sell",
                            SignalDirection::Hold => "hold",
                        };
                        let signal_key = (team.clone(), direction.to_string());

                        // Check debounce
                        let should_emit = match last_signal_times.get(&signal_key) {
                            Some(last_time) => last_time.elapsed().as_secs() >= signal_debounce_secs,
                            None => true,
                        };

                        if should_emit {
                            // Log platform selection for debugging
                            let (team_kalshi, team_poly) = if team == &game.home_team {
                                (home_kalshi, home_poly)
                            } else {
                                (away_kalshi, away_poly)
                            };
                            debug!(
                                "Selected {:?} for {} (kalshi={}, poly={})",
                                platform,
                                team,
                                team_kalshi
                                    .map(|p| format!("{:.3}", p.yes_ask))
                                    .unwrap_or_else(|| "N/A".to_string()),
                                team_poly
                                    .map(|p| format!("{:.3}", p.yes_ask))
                                    .unwrap_or_else(|| "N/A".to_string())
                            );

                            if model_edge::detect_and_emit(
                                &game_id,
                                sport_enum,
                                team,
                                prob,
                                price,
                                platform,
                                min_edge_pct,
                                &zmq_pub,
                                &zmq_seq,
                            )
                            .await
                            {
                                last_signal_times.insert(signal_key, Instant::now());
                            }
                        } else {
                            debug!(
                                "DEBOUNCE: {} {} - {}s remaining",
                                team,
                                direction,
                                signal_debounce_secs.saturating_sub(
                                    last_signal_times.get(&signal_key).map(|t| t.elapsed().as_secs()).unwrap_or(0)
                                )
                            );
                        }
                    }
                }
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

// Signal emission functions moved to signals/ modules:
// - signals/emission.rs: Core ZMQ publishing
// - signals/model_edge.rs: check_and_emit_signal -> model_edge::detect_and_emit
// - signals/arbitrage.rs: emit_arb_signal -> arbitrage::detect_and_emit
// - signals/latency.rs: emit_latency_signal -> latency::detect_and_emit

fn build_game_state(game: &EspnGame, sport_enum: Sport) -> GameState {
    let sport_specific = match sport_enum {
        Sport::NFL | Sport::NCAAF => SportSpecificState::Football(FootballState {
            down: game.down,
            yards_to_go: game.yards_to_go,
            yard_line: game.yard_line,
            is_redzone: game.is_redzone,
            timeouts_home: 3, // Default to full timeouts (ESPN doesn't always provide)
            timeouts_away: 3,
        }),
        Sport::NBA | Sport::NCAAB => SportSpecificState::Basketball(Default::default()),
        Sport::NHL => SportSpecificState::Hockey(Default::default()),
        Sport::MLB => SportSpecificState::Baseball(Default::default()),
        Sport::MLS | Sport::Soccer => SportSpecificState::Soccer(Default::default()),
        Sport::Tennis | Sport::MMA => SportSpecificState::Other,
    };

    GameState {
        // Universal fields (new - Phase 1-5)
        event_id: game.id.clone(),
        market_type: Some(MarketType::sport(sport_enum)),
        entity_a: Some(game.home_team.clone()),
        entity_b: Some(game.away_team.clone()),
        event_start: None,
        event_end: None,
        resolution_criteria: None,
        // Legacy fields
        game_id: game.id.clone(),
        sport: sport_enum,
        home_team: game.home_team.clone(),
        away_team: game.away_team.clone(),
        home_score: game.home_score,
        away_score: game.away_score,
        period: game.period,
        time_remaining_seconds: game.time_remaining_seconds,
        possession: game.possession.clone(),
        fetched_at: Utc::now(), // Track when this state was fetched
        pregame_home_prob: None,
        sport_specific,
        market_specific: None,
    }
}

async fn fetch_game_state(
    espn: &EspnClient,
    game_id: &str,
    sport: &str,
    espn_circuit_breaker: &ApiCircuitBreaker,
) -> Option<(EspnGame, GameState, f64)> {
    let (espn_sport, espn_league) = espn_sport_league(sport)?;

    // Check circuit breaker before making ESPN API call
    if !espn_circuit_breaker.is_available() {
        debug!(
            "ESPN circuit breaker OPEN - skipping fetch for game {}",
            game_id
        );
        return None;
    }

    let games = match espn.get_games(espn_sport, espn_league).await {
        Ok(g) => {
            espn_circuit_breaker.record_success();
            g
        }
        Err(e) => {
            espn_circuit_breaker.record_failure();
            warn!(
                "ESPN fetch error (failures: {}): {}",
                espn_circuit_breaker.failure_count(),
                e
            );
            return None;
        }
    };

    let sport_enum = parse_sport(sport)?;
    let batch_enabled = env::var("BATCH_PROBABILITY")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(true);

    if batch_enabled && games.len() > 1 {
        let states: Vec<GameState> = games.iter().map(|g| build_game_state(g, sport_enum)).collect();
        let start = Instant::now();
        let probs = batch_calculate_win_probs(&states, true);
        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(50) {
            info!(
                "Batch win_prob calc: {} games in {:?}",
                states.len(),
                elapsed
            );
        }

        if let Some(index) = games.iter().position(|g| g.id == game_id) {
            let game = games[index].clone();
            let state = states[index].clone();
            let home_win_prob = probs.get(index).copied().unwrap_or(0.5);
            return Some((game, state, home_win_prob));
        }
    }

    let game = games.into_iter().find(|g| g.id == game_id)?;
    let state = build_game_state(&game, sport_enum);
    let home_win_prob = calculate_win_probability(&state, true);
    Some((game, state, home_win_prob))
}



// ============================================================================
// Unit Tests
// ============================================================================
//
// Tests moved to submodules:
// - signals/edge.rs: compute_team_net_edge, fee_for_price tests
// - monitoring/espn.rs: parse_sport, is_overtime, format_time_remaining, check_cross_platform_arb tests
// - price/matching.rs: find_team_prices, select_best_platform_for_team tests
//
// Run all tests with:
// cargo test --package game_shard_rust
