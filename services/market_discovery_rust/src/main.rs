mod matching;
mod providers;

use chrono::Utc;
use dotenv::dotenv;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use matching::{is_non_moneyline_market, match_game_in_text};
use providers::{kalshi::KalshiClient, polymarket::PolymarketClient};
use redis::AsyncCommands;
use redis::aio::PubSub;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::env;
use std::time::Duration;
use std::time::Instant;

const DISCOVERY_REQUESTS_CH: &str = "discovery:requests";
const DISCOVERY_RESULTS_CH: &str = "discovery:results";
const CACHE_TTL_SECS: u64 = 60;
const DISCOVERY_GAME_KEY_PREFIX: &str = "discovery:game:";
const DISCOVERY_GAME_KEY_TTL_SECS: u64 = 7 * 24 * 60 * 60; // 7 days

// Heartbeat constants
const HEARTBEAT_KEY_PREFIX: &str = "health:hb";
const HEARTBEAT_CHANNEL: &str = "health:heartbeats";
const HEARTBEAT_INTERVAL_SECS: u64 = 10;
const HEARTBEAT_TTL_SECS: u64 = 35;

#[derive(Debug, Deserialize)]
struct DiscoveryRequest {
    game_id: String,
    sport: String,
    home_team: String,
    away_team: String,
    home_abbr: String,
    away_abbr: String,
}

#[derive(Debug, Serialize)]
struct Heartbeat {
    service: String,
    instance_id: String,
    status: String,
    started_at: String,
    timestamp: String,
    checks: HashMap<String, bool>,
    metrics: HashMap<String, f64>,
    version: Option<String>,
    hostname: Option<String>,
}

#[derive(Debug, Serialize)]
struct DiscoveryResult {
    game_id: String,
    sport: String,
    home_team: String,
    away_team: String,
    home_abbr: String,
    away_abbr: String,
    polymarket_moneyline: Option<String>, // Gamma market id (numeric string)
    kalshi_moneyline: Option<String>,     // Kalshi ticker (currently not populated; Orchestrator handles Kalshi)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    env_logger::init();

    info!("Starting Rust Market Discovery Service...");

    // Redis Connection
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let client = redis::Client::open(redis_url)?;
    let mut con = client.get_async_connection().await?;

    // Separate pubsub connection (redis-rs requires a dedicated connection)
    let pubsub_con = client.get_async_connection().await?;
    let mut pubsub = pubsub_con.into_pubsub();
    pubsub.subscribe(DISCOVERY_REQUESTS_CH).await?;

    let poly_client = PolymarketClient::new();
    let kalshi_client = KalshiClient::new();
    let espn_client = providers::espn::EspnClient::new();

    // Optional: background polling cycle (can be disabled for lowest latency / least load)
    let poll_espn = env::var("DISCOVERY_POLL_ESPN")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    // Map internal sport/league to API path for optional polling
    // (espn_sport, espn_league, poly_tag, redis_sport)
    let sport_configs = vec![
        ("basketball", "nba", "nba", "nba"),
        ("basketball", "mens-college-basketball", "ncaab", "ncaab"),
        ("hockey", "nhl", "nhl", "nhl"),
        ("soccer", "eng.1", "soccer", "soccer"), // Premier League
    ];

    // Spawn request/response listener (lowest-latency path)
    let poly_cache: Arc<RwLock<HashMap<String, (Instant, Vec<providers::polymarket::Market>)>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let kalshi_cache: Arc<RwLock<HashMap<String, (Instant, Vec<providers::kalshi::KalshiMarket>)>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let poly_client_req = poly_client.clone();
    let kalshi_client_req = kalshi_client.clone();
    let redis_client_req = client.clone();
    let poly_cache_req = poly_cache.clone();
    let kalshi_cache_req = kalshi_cache.clone();
    tokio::spawn(async move {
        if let Err(e) = request_listener(
            pubsub,
            redis_client_req,
            poly_client_req,
            kalshi_client_req,
            poly_cache_req,
            kalshi_cache_req,
        )
        .await
        {
            error!("Discovery request listener exited: {}", e);
        }
    });

    // Spawn heartbeat task
    let heartbeat_client = client.clone();
    let started_at = chrono::Utc::now().to_rfc3339();
    tokio::spawn(async move {
        if let Err(e) = heartbeat_loop(heartbeat_client, started_at).await {
            error!("Heartbeat loop exited: {}", e);
        }
    });

    loop {
        if !poll_espn {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            continue;
        }

        info!("--- Starting Discovery Cycle (ESPN polling enabled) ---");

        for (e_sport, e_league, _p_tag, r_sport) in &sport_configs {
            info!("Polling ESPN for {}/{}", e_sport, e_league);
            let games = match espn_client.get_games(e_sport, e_league).await {
                Ok(g) => g,
                Err(e) => {
                    error!("ESPN error for {}/{}: {}", e_sport, e_league, e);
                    continue;
                }
            };

            if games.is_empty() {
                continue;
            }
            info!("Found {} games for {}", games.len(), e_league);

            for game in games {
                let res = discover_for_game(
                    &poly_client,
                    &kalshi_client,
                    &game.id,
                    r_sport,
                    &game.home_team,
                    &game.away_team,
                    &game.home_abbr,
                    &game.away_abbr,
                    &poly_cache,
                    &kalshi_cache,
                )
                .await;
                let (poly_id, kalshi_id) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        error!("Discovery error for game {}: {}", game.id, e);
                        (None, None)
                    }
                };

                // 3. Publish to Redis if found
                if poly_id.is_some() || kalshi_id.is_some() {
                    let key = format!(
                        "discovery:game:{}_vs_{}",
                        game.home_team.replace(" ", ""),
                        game.away_team.replace(" ", "")
                    );
                    let value = serde_json::json!({
                        "id": game.id,
                        "sport": r_sport,
                        "home": game.home_team,
                        "away": game.away_team,
                        "home_abbr": game.home_abbr,
                        "away_abbr": game.away_abbr,
                        "time": game.date,
                        "polymarket_moneyline": poly_id,
                        "kalshi_moneyline": kalshi_id,
                    });

                    let _: () = con.set(&key, value.to_string()).await.unwrap_or_else(|e| {
                        error!("Redis error: {}", e);
                    });
                    info!("Wrote discovery data to Redis: {}", key);
                }
            }
        }

        info!("Cycle complete. Waiting 30 seconds...");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn request_listener(
    mut pubsub: PubSub,
    redis_client: redis::Client,
    poly_client: PolymarketClient,
    kalshi_client: KalshiClient,
    poly_cache: Arc<RwLock<HashMap<String, (Instant, Vec<providers::polymarket::Market>)>>>,
    kalshi_cache: Arc<RwLock<HashMap<String, (Instant, Vec<providers::kalshi::KalshiMarket>)>>>,
) -> anyhow::Result<()> {
    info!("Listening for discovery requests on {}", DISCOVERY_REQUESTS_CH);

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: Vec<u8> = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!("Discovery request: failed to read payload: {}", e);
                continue;
            }
        };

        let req: DiscoveryRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(e) => {
                // IMPORTANT: Never crash the listener due to one bad message.
                // PubSub has no retry; crashing here would stop all discovery.
                let preview = String::from_utf8_lossy(&payload);
                warn!(
                    "Discovery request: invalid JSON ({}). payload='{}'",
                    e,
                    preview.chars().take(400).collect::<String>()
                );
                continue;
            }
        };

        let (poly_id, kalshi_id) = match discover_for_game(
            &poly_client,
            &kalshi_client,
            &req.game_id,
            &req.sport,
            &req.home_team,
            &req.away_team,
            &req.home_abbr,
            &req.away_abbr,
            &poly_cache,
            &kalshi_cache,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "Discovery request: error discovering markets for game {}: {}",
                    req.game_id, e
                );
                (None, None)
            }
        };

        let result = DiscoveryResult {
            game_id: req.game_id.clone(),
            sport: req.sport.clone(),
            home_team: req.home_team.clone(),
            away_team: req.away_team.clone(),
            home_abbr: req.home_abbr.clone(),
            away_abbr: req.away_abbr.clone(),
            polymarket_moneyline: poly_id,
            kalshi_moneyline: kalshi_id,
        };

        let encoded = serde_json::to_vec(&result)?;
        let json_str = String::from_utf8_lossy(&encoded).to_string();
        let key = format!("{}{}", DISCOVERY_GAME_KEY_PREFIX, req.game_id);

        // NOTE: Keep the listener alive even if Redis has transient errors.
        let mut con = match redis_client.get_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                warn!("Discovery request: Redis connection error: {}", e);
                continue;
            }
        };
        // Publish for consumers needing low latency
        if let Err(e) = con.publish::<&str, Vec<u8>, i64>(DISCOVERY_RESULTS_CH, encoded).await {
            warn!("Discovery request: Redis publish error: {}", e);
        }
        // Persist for pregame lookups / restarts
        if let Err(e) = con
            .set_ex::<String, String, ()>(key, json_str, DISCOVERY_GAME_KEY_TTL_SECS)
            .await
        {
            warn!("Discovery request: Redis set_ex error: {}", e);
        }
    }

    Ok(())
}

async fn discover_for_game(
    poly_client: &PolymarketClient,
    _kalshi_client: &KalshiClient,
    game_id: &str,
    sport: &str,
    home_team: &str,
    away_team: &str,
    home_abbr: &str,
    away_abbr: &str,
    poly_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<providers::polymarket::Market>)>>>,
    kalshi_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<providers::kalshi::KalshiMarket>)>>>,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    // Strategy:
    // 1) Fetch by sport tag(s) - use SPECIFIC tags to avoid cross-league matches
    //    (e.g., "nba" NOT "basketball" to avoid matching NBA teams to NCAAB markets)
    // 2) Narrow by name matching (city/team/abbr fuzzy match)
    let sport_lower = sport.to_lowercase();
    let poly_tags: Vec<String> = match sport_lower.as_str() {
        // IMPORTANT: Use specific league tags, not broad category tags
        // Using "basketball" would match NBA teams to NCAAB markets (e.g., Cleveland Cavaliers -> Cleveland State)
        "ncaab" => vec!["ncaab".to_string()],  // College basketball only
        "nba" => vec!["nba".to_string()],      // NBA only
        "ncaaf" => vec!["ncaaf".to_string()],  // College football only
        "nfl" => vec!["nfl".to_string()],      // NFL only
        "nhl" => vec!["nhl".to_string()],
        other => vec![other.to_string()],
    };

    // Pull Polymarket markets from cache (fetch on miss/expiry)
    let mut poly_markets: Vec<providers::polymarket::Market> = Vec::new();
    for tag in poly_tags.iter() {
        let tag_key = tag.clone();
        let now = Instant::now();

        // Check cache
        let cached = {
            let guard = poly_cache.read().await;
            guard.get(&tag_key).cloned()
        };

        let markets_for_tag = if let Some((ts, markets)) = cached {
            if now.duration_since(ts).as_secs() <= CACHE_TTL_SECS {
                markets
            } else {
                // Refresh
                let fetched = poly_client.search_markets("", &tag_key).await.unwrap_or_default();
                let mut guard = poly_cache.write().await;
                guard.insert(tag_key.clone(), (Instant::now(), fetched.clone()));
                fetched
            }
        } else {
            let fetched = poly_client.search_markets("", &tag_key).await.unwrap_or_default();
            let mut guard = poly_cache.write().await;
            guard.insert(tag_key.clone(), (Instant::now(), fetched.clone()));
            fetched
        };

        poly_markets.extend(markets_for_tag);
    }

    // Kalshi requires authenticated requests (RSA signatures). To keep this service fast and
    // dependency-light, Orchestrator (Python) handles Kalshi market ID discovery from its cache.
    let _ = kalshi_cache; // keep cache wiring for future use

    // Polymarket scan: return Gamma market id (numeric string)
    // IMPORTANT: Only match MONEYLINE markets, skip totals (O/U), spreads, props, etc.
    let mut poly_market_id: Option<String> = None;
    let mut best_match_score: f64 = 0.0;

    for market in &poly_markets {
        // Skip non-moneyline markets (totals, spreads, props)
        if is_non_moneyline_market(&market.question) {
            continue;
        }

        // Use the improved game matching that requires BOTH teams to match
        let (is_match, home_result, away_result) = match_game_in_text(
            home_team,
            away_team,
            home_abbr,
            away_abbr,
            &market.question,
            sport,
        );

        if is_match {
            // Calculate combined confidence score
            let combined_score = (home_result.score + away_result.score) / 2.0;

            // Only accept if this is the best match so far
            if combined_score > best_match_score {
                best_match_score = combined_score;
                poly_market_id = Some(market.id.clone());

                info!(
                    "[DISCOVERY] Polymarket MONEYLINE match: game={} {} vs {} -> market_id={} (home: {:.2} {}, away: {:.2} {})",
                    game_id, away_team, home_team, market.id,
                    home_result.score, home_result.reason,
                    away_result.score, away_result.reason
                );

                // If we have a high-confidence match, stop searching
                if combined_score >= 0.9 {
                    break;
                }
            }
        } else if home_result.is_match() || away_result.is_match() {
            // Log near-misses for debugging
            warn!(
                "[DISCOVERY] Partial match (skipped): game={} {} vs {} question='{}' home={:?} away={:?}",
                game_id, away_team, home_team, market.question,
                home_result.is_match(), away_result.is_match()
            );
        }
    }

    let kalshi_ticker: Option<String> = None;

    Ok((poly_market_id, kalshi_ticker))
}

/// Heartbeat loop - publishes periodic health status to Redis
async fn heartbeat_loop(
    client: redis::Client,
    started_at: String,
) -> anyhow::Result<()> {
    let mut con = client.get_async_connection().await?;
    let instance_id = env::var("HOSTNAME").unwrap_or_else(|_| "market-discovery-rust-1".to_string());
    let version = env::var("BUILD_VERSION").ok();
    let hostname = hostname::get().ok().and_then(|h| h.into_string().ok());

    info!("Heartbeat loop started for {}", instance_id);

    loop {
        let now = Utc::now().to_rfc3339();

        let mut checks = HashMap::new();
        checks.insert("redis_ok".to_string(), true);

        let metrics = HashMap::new();

        let heartbeat = Heartbeat {
            service: "market_discovery_rust".to_string(),
            instance_id: instance_id.clone(),
            status: "healthy".to_string(),
            started_at: started_at.clone(),
            timestamp: now,
            checks,
            metrics,
            version: version.clone(),
            hostname: hostname.clone(),
        };

        let payload = serde_json::to_string(&heartbeat)?;
        let key = format!("{}:market_discovery_rust:{}", HEARTBEAT_KEY_PREFIX, instance_id);

        // SETEX for liveness
        let _: () = con.set_ex(&key, &payload, HEARTBEAT_TTL_SECS).await?;

        // Publish for real-time observability
        let _: () = con.publish(HEARTBEAT_CHANNEL, &payload).await?;

        debug!("Heartbeat published: {}", key);

        tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)).await;
    }
}
