use arbees_rust_core::clients::{
    espn::EspnClient,
    kalshi::{KalshiClient, KalshiMarket},
    polymarket::{Market as PolyMarket, PolymarketClient},
};
use arbees_rust_core::utils::matching::{
    is_non_moneyline_market, match_game_in_text, match_team_in_text,
    match_teams_with_context, GameContext, MarketContext,
};
use chrono::Utc;
use dotenv::dotenv;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use redis::aio::PubSub;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::RwLock;

const DISCOVERY_REQUESTS_CH: &str = "discovery:requests";
const DISCOVERY_RESULTS_CH: &str = "discovery:results";
const CACHE_TTL_SECS: u64 = 60;
const DISCOVERY_GAME_KEY_PREFIX: &str = "discovery:game:";
const DISCOVERY_GAME_KEY_TTL_SECS: u64 = 7 * 24 * 60 * 60; // 7 days

// Team matching RPC channels
const TEAM_MATCH_REQUEST_CH: &str = "team:match:request";
const TEAM_MATCH_RESPONSE_PREFIX: &str = "team:match:response:";

// Heartbeat constants
const HEARTBEAT_KEY_PREFIX: &str = "health:hb";
const HEARTBEAT_CHANNEL: &str = "health:heartbeats";
const HEARTBEAT_INTERVAL_SECS: u64 = 10;
const HEARTBEAT_TTL_SECS: u64 = 35;

/// Validates that a market question doesn't contain sport-specific keywords
/// that indicate a DIFFERENT sport than expected.
/// Returns (is_valid, reason) - (true, _) if the sport matches or is ambiguous
fn validate_market_sport(question: &str, expected_sport: &str) -> (bool, String) {
    let q_lower = question.to_lowercase();
    let expected_lower = expected_sport.to_lowercase();

    // Keywords that strongly indicate specific sports/leagues
    // If we find keywords for a DIFFERENT sport, reject the match
    let nba_keywords = ["nba", "lakers", "celtics", "warriors", "76ers", "knicks", "nets", "clippers"];
    let ncaab_keywords = ["ncaa", "college", "march madness", "final four", "wildcats", "bluejays", "zags", "huskies"];
    let nfl_keywords = ["nfl", "chiefs", "eagles", "cowboys", "49ers", "steelers", "patriots", "super bowl"];
    let ncaaf_keywords = ["college football", "cfb", "bowl game", "playoff"];
    let nhl_keywords = ["nhl", "stanley cup", "bruins", "rangers", "penguins", "avalanche"];
    let mlb_keywords = ["mlb", "yankees", "dodgers", "red sox", "world series"];
    let soccer_keywords = ["premier league", "epl", "uefa", "champions league", "la liga", "bundesliga"];
    let mma_keywords = ["ufc", "mma", "bellator", "pfl"];

    // Check for cross-league indicators
    match expected_lower.as_str() {
        "nba" => {
            // If expecting NBA but see college keywords, reject
            for kw in ncaab_keywords.iter() {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains college basketball keyword: '{}'", kw));
                }
            }
        }
        "ncaab" => {
            // If expecting college basketball but see NBA keywords, reject
            for kw in nba_keywords.iter() {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains NBA keyword: '{}'", kw));
                }
            }
        }
        "nfl" => {
            // If expecting NFL but see college football keywords, reject
            for kw in ncaaf_keywords.iter() {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains college football keyword: '{}'", kw));
                }
            }
        }
        "ncaaf" => {
            // If expecting college football but see NFL keywords, reject
            for kw in nfl_keywords.iter() {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains NFL keyword: '{}'", kw));
                }
            }
        }
        "nhl" => {
            // NHL is fairly distinct, but check for other sports
            for kw in nba_keywords.iter().chain(nfl_keywords.iter()) {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains non-NHL keyword: '{}'", kw));
                }
            }
        }
        "mlb" => {
            // MLB should not match other sports
            for kw in nba_keywords.iter().chain(nfl_keywords.iter()).chain(nhl_keywords.iter()) {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains non-MLB keyword: '{}'", kw));
                }
            }
        }
        "soccer" | "mls" => {
            // Soccer should not match American sports leagues
            for kw in nba_keywords.iter().chain(nfl_keywords.iter()).chain(mlb_keywords.iter()) {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains non-soccer keyword: '{}'", kw));
                }
            }
        }
        "mma" => {
            // MMA should not match team sports
            for kw in nba_keywords.iter().chain(nfl_keywords.iter()).chain(nhl_keywords.iter()) {
                if q_lower.contains(kw) {
                    return (false, format!("Market contains non-MMA keyword: '{}'", kw));
                }
            }
        }
        _ => {}
    }

    (true, "Sport validation passed".to_string())
}

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
    kalshi_moneyline: Option<String>, // Kalshi ticker (currently not populated; Orchestrator handles Kalshi)
}

// Team matching RPC types
#[derive(Debug, Deserialize)]
struct TeamMatchRequest {
    request_id: String,
    target_team: String,
    candidate_team: String,
    sport: String,
    // NEW: Optional context fields (backward compatible)
    #[serde(default)]
    game_context: Option<GameContext>,
    #[serde(default)]
    market_context: Option<MarketContext>,
    /// Whether target_team is the home team (for opponent validation)
    #[serde(default)]
    target_is_home: bool,
}

#[derive(Debug, Serialize)]
struct TeamMatchResponse {
    request_id: String,
    is_match: bool,
    confidence: f64,
    method: String,
    reason: String,
    // NEW: Additional context validation fields
    #[serde(skip_serializing_if = "Option::is_none")]
    sport_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opponent_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    score_correlation: Option<f64>,
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
    pubsub.subscribe(TEAM_MATCH_REQUEST_CH).await?;
    info!(
        "Subscribed to channels: {}, {}",
        DISCOVERY_REQUESTS_CH, TEAM_MATCH_REQUEST_CH
    );

    let poly_client = PolymarketClient::new();
    let kalshi_client = KalshiClient::new();
    let espn_client = EspnClient::new();

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
    let poly_cache: Arc<RwLock<HashMap<String, (Instant, Vec<PolyMarket>)>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let kalshi_cache: Arc<RwLock<HashMap<String, (Instant, Vec<KalshiMarket>)>>> =
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

                    let json_str = value.to_string();

                    let _: () = con.set(&key, &json_str).await.unwrap_or_else(|e| {
                        error!("Redis error: {}", e);
                    });

                    // Also publish to channel so Orchestrator picks it up immediately
                    let _: () = con
                        .publish(DISCOVERY_RESULTS_CH, &json_str)
                        .await
                        .unwrap_or_else(|e| {
                            error!("Redis publish error: {}", e);
                        });

                    info!("Wrote discovery data to Redis and Published: {}", key);
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
    poly_cache: Arc<RwLock<HashMap<String, (Instant, Vec<PolyMarket>)>>>,
    kalshi_cache: Arc<RwLock<HashMap<String, (Instant, Vec<KalshiMarket>)>>>,
) -> anyhow::Result<()> {
    info!(
        "Listening for requests on {} and {}",
        DISCOVERY_REQUESTS_CH, TEAM_MATCH_REQUEST_CH
    );

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let channel: String = msg.get_channel_name().to_string();
        let payload: Vec<u8> = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!("Request listener: failed to read payload: {}", e);
                continue;
            }
        };

        // Route by channel
        if channel == TEAM_MATCH_REQUEST_CH {
            // Handle team matching RPC
            handle_team_match_request(&redis_client, &payload).await;
        } else if channel == DISCOVERY_REQUESTS_CH {
            // Handle discovery request (existing logic)
            handle_discovery_request(
                &redis_client,
                &poly_client,
                &kalshi_client,
                &poly_cache,
                &kalshi_cache,
                &payload,
            )
            .await;
        } else {
            warn!("Unknown channel: {}", channel);
        }
    }

    Ok(())
}

/// Handle team matching RPC request
async fn handle_team_match_request(redis_client: &redis::Client, payload: &[u8]) {
    let req: TeamMatchRequest = match serde_json::from_slice(payload) {
        Ok(r) => r,
        Err(e) => {
            let preview = String::from_utf8_lossy(payload);
            warn!(
                "Team match request: invalid JSON ({}). payload='{}'",
                e,
                preview.chars().take(200).collect::<String>()
            );
            return;
        }
    };

    let has_context = req.game_context.is_some() || req.market_context.is_some();

    debug!(
        "Team match request: '{}' vs '{}' (sport: {}, has_context: {})",
        req.target_team, req.candidate_team, req.sport, has_context
    );

    // Use context-aware matching if context provided, otherwise fall back to name-only
    let response = if has_context {
        // Context-enhanced matching
        let ctx_result = match_teams_with_context(
            &req.target_team,
            &req.candidate_team,
            &req.sport,
            req.game_context.as_ref(),
            req.market_context.as_ref(),
            req.target_is_home,
        );

        TeamMatchResponse {
            request_id: req.request_id.clone(),
            is_match: ctx_result.final_confidence >= 0.5 && ctx_result.name_match.is_match,
            confidence: ctx_result.final_confidence,
            method: ctx_result.name_match.confidence_level.clone(),
            reason: ctx_result.rejection_reason.unwrap_or_else(|| {
                format!(
                    "{} (opponent: {:.2}, score_corr: {:?})",
                    ctx_result.name_match.reason,
                    ctx_result.opponent_score,
                    ctx_result.score_correlation
                )
            }),
            sport_valid: Some(ctx_result.sport_valid),
            opponent_score: Some(ctx_result.opponent_score),
            score_correlation: ctx_result.score_correlation,
        }
    } else {
        // Backward compatible: name-only matching
        let result = match_team_in_text(&req.target_team, &req.candidate_team, &req.sport);

        TeamMatchResponse {
            request_id: req.request_id.clone(),
            is_match: result.is_match(),
            confidence: result.score,
            method: format!("{:?}", result.confidence),
            reason: result.reason.clone(),
            sport_valid: None,
            opponent_score: None,
            score_correlation: None,
        }
    };

    // Publish response to specific channel
    let response_channel = format!("{}{}", TEAM_MATCH_RESPONSE_PREFIX, req.request_id);

    let mut con = match redis_client.get_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            warn!("Team match request: Redis connection error: {}", e);
            return;
        }
    };

    let response_json = match serde_json::to_string(&response) {
        Ok(j) => j,
        Err(e) => {
            warn!("Team match request: JSON serialization error: {}", e);
            return;
        }
    };

    if let Err(e) = con
        .publish::<&str, &str, i64>(&response_channel, &response_json)
        .await
    {
        warn!("Team match request: Redis publish error: {}", e);
    } else {
        debug!(
            "Team match response: match={}, confidence={:.2}, method={}, has_context={} -> {}",
            response.is_match, response.confidence, response.method, has_context, response_channel
        );
    }
}

/// Handle discovery request (refactored from inline code)
async fn handle_discovery_request(
    redis_client: &redis::Client,
    poly_client: &PolymarketClient,
    kalshi_client: &KalshiClient,
    poly_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<PolyMarket>)>>>,
    kalshi_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<KalshiMarket>)>>>,
    payload: &[u8],
) {
    let req: DiscoveryRequest = match serde_json::from_slice(payload) {
        Ok(r) => r,
        Err(e) => {
            // IMPORTANT: Never crash the listener due to one bad message.
            let preview = String::from_utf8_lossy(payload);
            warn!(
                "Discovery request: invalid JSON ({}). payload='{}'",
                e,
                preview.chars().take(400).collect::<String>()
            );
            return;
        }
    };

    let (poly_id, kalshi_id) = match discover_for_game(
        poly_client,
        kalshi_client,
        &req.game_id,
        &req.sport,
        &req.home_team,
        &req.away_team,
        &req.home_abbr,
        &req.away_abbr,
        poly_cache,
        kalshi_cache,
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

    let encoded = match serde_json::to_vec(&result) {
        Ok(e) => e,
        Err(e) => {
            warn!("Discovery request: JSON serialization error: {}", e);
            return;
        }
    };
    let json_str = String::from_utf8_lossy(&encoded).to_string();
    let key = format!("{}{}", DISCOVERY_GAME_KEY_PREFIX, req.game_id);

    // NOTE: Keep the listener alive even if Redis has transient errors.
    let mut con = match redis_client.get_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            warn!("Discovery request: Redis connection error: {}", e);
            return;
        }
    };
    // Publish for consumers needing low latency
    if let Err(e) = con
        .publish::<&str, Vec<u8>, i64>(DISCOVERY_RESULTS_CH, encoded)
        .await
    {
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

async fn discover_for_game(
    poly_client: &PolymarketClient,
    _kalshi_client: &KalshiClient,
    game_id: &str,
    sport: &str,
    home_team: &str,
    away_team: &str,
    home_abbr: &str,
    away_abbr: &str,
    poly_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<PolyMarket>)>>>,
    kalshi_cache: &Arc<RwLock<HashMap<String, (Instant, Vec<KalshiMarket>)>>>,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    // Strategy:
    // 1) Fetch by sport tag(s) - use SPECIFIC tags to avoid cross-league matches
    //    (e.g., "nba" NOT "basketball" to avoid matching NBA teams to NCAAB markets)
    // 2) Narrow by name matching (city/team/abbr fuzzy match)
    let sport_lower = sport.to_lowercase();
    let poly_tags: Vec<String> = match sport_lower.as_str() {
        // IMPORTANT: Use specific league tags, not broad category tags
        // Using "basketball" would match NBA teams to NCAAB markets (e.g., Cleveland Cavaliers -> Cleveland State)
        "ncaab" => vec!["ncaab".to_string()], // College basketball only
        "nba" => vec!["nba".to_string()],     // NBA only
        "ncaaf" => vec!["ncaaf".to_string()], // College football only
        "nfl" => vec!["nfl".to_string()],     // NFL only
        "nhl" => vec!["nhl".to_string()],
        other => vec![other.to_string()],
    };

    // Pull Polymarket markets from cache (fetch on miss/expiry)
    let mut poly_markets: Vec<PolyMarket> = Vec::new();
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
                let fetched = poly_client
                    .search_markets("", &tag_key)
                    .await
                    .unwrap_or_default();
                let mut guard = poly_cache.write().await;
                guard.insert(tag_key.clone(), (Instant::now(), fetched.clone()));
                fetched
            }
        } else {
            let fetched = poly_client
                .search_markets("", &tag_key)
                .await
                .unwrap_or_default();
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

        // SPORT VALIDATION: Check for cross-league keywords BEFORE team matching
        // This prevents matching NBA teams to NCAAB markets (e.g., Cleveland Cavaliers -> Cleveland State)
        let (sport_valid, sport_reason) = validate_market_sport(&market.question, sport);
        if !sport_valid {
            debug!(
                "[DISCOVERY] Sport validation failed for market '{}': {}",
                market.question.chars().take(80).collect::<String>(),
                sport_reason
            );
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
async fn heartbeat_loop(client: redis::Client, started_at: String) -> anyhow::Result<()> {
    let mut con = client.get_async_connection().await?;
    let instance_id =
        env::var("HOSTNAME").unwrap_or_else(|_| "market-discovery-rust-1".to_string());
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
        let key = format!(
            "{}:market_discovery_rust:{}",
            HEARTBEAT_KEY_PREFIX, instance_id
        );

        // SETEX for liveness
        let _: () = con.set_ex(&key, &payload, HEARTBEAT_TTL_SECS).await?;

        // Publish for real-time observability
        let _: () = con.publish(HEARTBEAT_CHANNEL, &payload).await?;

        debug!("Heartbeat published: {}", key);

        tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)).await;
    }
}
