use crate::clients::team_matching::TeamMatchingClient;
use crate::state::GameInfo;
use anyhow::Result;
use arbees_rust_core::clients::kalshi::{KalshiClient, KalshiMarket};
use arbees_rust_core::league_config::LEAGUE_CONFIGS;
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub struct KalshiDiscoveryManager {
    kalshi_client: KalshiClient,
    team_matching_client: TeamMatchingClient,
    cache: Arc<RwLock<MarketCache>>,
    request_delay_ms: u64,
}

struct MarketCache {
    markets: Vec<KalshiMarket>,
    last_refresh: Instant,
    // (game_id, market_type) -> market_ticker
    mappings: HashMap<(String, String), String>,
}

impl KalshiDiscoveryManager {
    pub fn new(kalshi_client: KalshiClient, team_matching_client: TeamMatchingClient) -> Self {
        let request_delay_ms = env::var("KALSHI_REQUEST_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(250);

        info!("KalshiDiscoveryManager initialized with {}ms delay between requests", request_delay_ms);

        Self {
            kalshi_client,
            team_matching_client,
            cache: Arc::new(RwLock::new(MarketCache {
                markets: Vec::new(),
                last_refresh: Instant::now()
                    .checked_sub(Duration::from_secs(1000))
                    .unwrap(),
                mappings: HashMap::new(),
            })),
            request_delay_ms,
        }
    }

    pub async fn find_moneyline_market(&self, game: &GameInfo) -> Option<String> {
        self.find_market_by_type(game, "moneyline").await
    }

    pub async fn find_market_by_type(&self, game: &GameInfo, market_type: &str) -> Option<String> {
        // Check mapping cache first
        {
            let cache = self.cache.read().await;
            if let Some(ticker) = cache
                .mappings
                .get(&(game.game_id.clone(), market_type.to_string()))
            {
                return Some(ticker.clone());
            }
        }

        // Refresh markets if stale or missing this sport's series
        let (refresh_needed, missing_series) = {
            let cache = self.cache.read().await;
            let stale = cache.last_refresh.elapsed() > Duration::from_secs(300);
            let series = self.series_ticker_for_sport(game.sport.as_str());
            let has_series = series.map_or(false, |s| {
                cache.markets.iter().any(|m| m.ticker.starts_with(s))
            });
            // If we don't have any markets for this sport, refresh more aggressively (30s)
            let missing_series = !has_series && cache.last_refresh.elapsed() > Duration::from_secs(30);
            (stale || missing_series, missing_series)
        };

        if refresh_needed {
            if missing_series {
                if let Err(e) = self.refresh_markets(None).await {
                    error!("Failed to refresh Kalshi markets (all sports): {}", e);
                }
            } else {
                let mut active = HashSet::new();
                active.insert(game.sport.as_str().to_string());
                if let Err(e) = self.refresh_markets(Some(&active)).await {
                    error!("Failed to refresh Kalshi markets: {}", e);
                }
            }
        }

        let markets_snapshot = { self.cache.read().await.markets.clone() };

        // Search markets
        // 1. Filter by single game market heuristics
        // 2. Filter by market type
        // 3. Match teams

        // Parallel matching is hard here without streaming, so we loop sequentially or use concurrent futures
        // For simplicity and to avoid overwhelming Redis, we'll try sequential logic with smart pre-filtering

        for market in markets_snapshot {
            if !self.is_single_game_market(&market.ticker) {
                continue;
            }

            let detected_type = self.detect_market_type(&market.title);
            if detected_type != market_type {
                continue;
            }

            // Combined text for matching
            let combined = format!("{} {}", market.title, market.ticker);

            // Check Home Team
            match self
                .team_matching_client
                .match_teams(&game.home_team, &combined, game.sport.as_str())
                .await
            {
                Ok(Some(res)) if res.is_match && res.confidence >= 0.7 => {
                    // Check Away Team to be sure it's the right game
                    match self
                        .team_matching_client
                        .match_teams(&game.away_team, &combined, game.sport.as_str())
                        .await
                    {
                        Ok(Some(res_away)) if res_away.is_match && res_away.confidence >= 0.7 => {
                            // Found it!
                            info!("Kalshi match found: {} -> {}", game.game_id, market.ticker);

                            // Cache mapping
                            let mut cache = self.cache.write().await;
                            cache.mappings.insert(
                                (game.game_id.clone(), market_type.to_string()),
                                market.ticker.clone(),
                            );

                            return Some(market.ticker);
                        }
                        _ => continue,
                    }
                }
                _ => continue,
            }
        }

        None
    }

    async fn should_refresh(&self) -> bool {
        let cache = self.cache.read().await;
        cache.last_refresh.elapsed() > Duration::from_secs(300) // 5 minutes
    }

    /// Refresh Kalshi markets for the given active sports only.
    /// If no active sports provided, falls back to all supported sports.
    pub async fn refresh_markets(&self, active_sports: Option<&HashSet<String>>) -> Result<()> {
        let all_sports = ["nfl", "nba", "nhl", "mlb", "ncaaf", "ncaab"];

        // Filter to only active sports, or use all if none provided
        let sports_to_fetch: Vec<&str> = match active_sports {
            Some(active) if !active.is_empty() => {
                all_sports
                    .iter()
                    .filter(|s| active.contains(&s.to_string()))
                    .copied()
                    .collect()
            }
            _ => all_sports.to_vec(),
        };

        if sports_to_fetch.is_empty() {
            info!("No active sports to fetch Kalshi markets for");
            return Ok(());
        }

        info!(
            "Refreshing Kalshi markets for {} sports: {:?} ({}ms delay between requests)",
            sports_to_fetch.len(),
            sports_to_fetch,
            self.request_delay_ms
        );

        let mut all_markets = Vec::new();
        let delay = Duration::from_millis(self.request_delay_ms);

        for (i, sport) in sports_to_fetch.iter().enumerate() {
            // Add delay between requests (not before first one)
            if i > 0 && self.request_delay_ms > 0 {
                tokio::time::sleep(delay).await;
            }

            match self.kalshi_client.get_markets(Some(sport)).await {
                Ok(mut markets) => {
                    info!("Fetched {} Kalshi markets for {}", markets.len(), sport);
                    all_markets.append(&mut markets);
                }
                Err(e) => {
                    warn!("Error fetching Kalshi {} markets: {}", sport, e);
                }
            }
        }

        // Dedupe
        all_markets.sort_by(|a, b| a.ticker.cmp(&b.ticker));
        all_markets.dedup_by(|a, b| a.ticker == b.ticker);

        let mut cache = self.cache.write().await;
        cache.markets = all_markets;
        cache.last_refresh = Instant::now();
        info!("Refreshed Kalshi markets: {} items total", cache.markets.len());

        Ok(())
    }

    fn is_single_game_market(&self, ticker: &str) -> bool {
        let t = ticker.to_uppercase();
        if t.contains("MULTIGAME") || t.contains("PARLAY") {
            return false;
        }
        if t.contains("SINGLEGAME") || t.contains("FLOORGAME") {
            return true;
        }
        // Accept known per-game series tickers (KXNBAGAME, KXNHLGAME, etc.)
        for cfg in LEAGUE_CONFIGS {
            if t.starts_with(cfg.kalshi_series_game) {
                return true;
            }
        }
        false
    }

    fn detect_market_type(&self, title: &str) -> &'static str {
        let t = title.to_lowercase();
        // Simple heuristics mirroring Python
        if t.contains("spread") || t.contains("handicap") || t.contains("cover") {
            return "spread";
        }
        if t.contains("total") || t.contains("over") || t.contains("under") || t.contains("o/u") {
            return "total";
        }
        // Default to moneyline if nothing else
        "moneyline"
    }

    pub fn get_markets(&self) -> Vec<KalshiMarket> {
        // Exposed for testing or debug if needed, technically not needed for core loop
        vec![]
    }

    fn series_ticker_for_sport(&self, sport: &str) -> Option<&'static str> {
        LEAGUE_CONFIGS
            .iter()
            .find(|cfg| cfg.league_code.eq_ignore_ascii_case(sport))
            .map(|cfg| cfg.kalshi_series_game)
    }
}
