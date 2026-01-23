mod matching;
mod providers;

use dotenv::dotenv;
use futures_util::StreamExt;
use log::{error, info};
use matching::names_match;
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
        let payload: Vec<u8> = msg.get_payload()?;
        let req: DiscoveryRequest = serde_json::from_slice(&payload)?;

        let (poly_id, kalshi_id) = discover_for_game(
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
        .await?;

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

        let mut con = redis_client.get_async_connection().await?;
        // Publish for consumers needing low latency
        let _: i64 = con.publish(DISCOVERY_RESULTS_CH, encoded).await?;
        // Persist for pregame lookups / restarts
        let _: () = con
            .set_ex(key, json_str, DISCOVERY_GAME_KEY_TTL_SECS)
            .await?;
    }

    Ok(())
}

/// Check if a market question indicates a non-moneyline market (totals, spreads, props).
/// We only want to match moneyline (winner) markets.
fn is_non_moneyline_market(question: &str) -> bool {
    let q = question.to_lowercase();

    // Totals indicators
    if q.contains("o/u")
        || q.contains("over/under")
        || q.contains("total points")
        || q.contains("total goals")
        || q.contains("total runs")
        || q.contains("combined score")
        || q.contains("combined points")
    {
        return true;
    }

    // Spread indicators (look for +/- followed by numbers)
    // e.g., "+5.5", "-3", "spread"
    if q.contains("spread") || q.contains("handicap") {
        return true;
    }

    // Check for spread patterns like "+5.5" or "-3.5"
    // Simple heuristic: if there's a +/- followed by digits after team names
    let spread_patterns = [" +", " -"];
    for pattern in spread_patterns {
        if let Some(pos) = q.find(pattern) {
            // Check if followed by a digit
            let rest = &q[pos + pattern.len()..];
            if rest.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                return true;
            }
        }
    }

    // Props/special markets
    if q.contains("first to")
        || q.contains("most")
        || q.contains("mvp")
        || q.contains("player")
        || q.contains("quarter")
        || q.contains("half")
        || q.contains("period")
        || q.contains("inning")
        || q.contains("how many")
        || q.contains("exact score")
    {
        return true;
    }

    false
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
    // 1) Fetch by sport tag(s) (broad)
    // 2) Narrow by name matching (city/team/abbr fuzzy match)
    let sport_lower = sport.to_lowercase();
    let poly_tags: Vec<String> = match sport_lower.as_str() {
        "ncaab" | "nba" => vec![sport_lower.clone(), "basketball".to_string()],
        "ncaaf" | "nfl" => vec![sport_lower.clone(), "football".to_string()],
        "nhl" => vec!["nhl".to_string(), "hockey".to_string()],
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
    for market in &poly_markets {
        // Fast prefilter: require at least one of the abbreviations or a city token to appear.
        let q = market.question.to_lowercase();
        let pre_ok = q.contains(&home_abbr.to_lowercase())
            || q.contains(&away_abbr.to_lowercase())
            || q.contains(&home_team.split_whitespace().next().unwrap_or("").to_lowercase())
            || q.contains(&away_team.split_whitespace().next().unwrap_or("").to_lowercase());
        if !pre_ok {
            continue;
        }

        // Skip non-moneyline markets (totals, spreads, props)
        // These contain patterns like "O/U", "Over/Under", "Total", spread numbers (+/-), etc.
        if is_non_moneyline_market(&q) {
            continue;
        }

        let h_match = names_match(home_team, &market.question, sport) || names_match(home_abbr, &market.question, sport);
        let a_match = names_match(away_team, &market.question, sport) || names_match(away_abbr, &market.question, sport);
        if h_match && a_match {
            poly_market_id = Some(market.id.clone());
            info!("[DISCOVERY] Polymarket MONEYLINE match game={} {} vs {} -> market_id={}", game_id, away_team, home_team, market.id);
            break;
        }
    }

    let kalshi_ticker: Option<String> = None;

    Ok((poly_market_id, kalshi_ticker))
}
