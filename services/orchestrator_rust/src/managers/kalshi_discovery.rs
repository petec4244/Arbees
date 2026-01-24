use crate::clients::team_matching::TeamMatchingClient;
use crate::state::{GameInfo, Sport};
use anyhow::Result;
use arbees_rust_core::clients::kalshi::{KalshiClient, KalshiMarket};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

pub struct KalshiDiscoveryManager {
    kalshi_client: KalshiClient,
    team_matching_client: TeamMatchingClient,
    cache: Arc<RwLock<MarketCache>>,
}

struct MarketCache {
    markets: Vec<KalshiMarket>,
    last_refresh: Instant,
    // (game_id, market_type) -> market_ticker
    mappings: HashMap<(String, String), String>,
}

impl KalshiDiscoveryManager {
    pub fn new(kalshi_client: KalshiClient, team_matching_client: TeamMatchingClient) -> Self {
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

        // Refresh markets if stale
        if self.should_refresh().await {
            if let Err(e) = self.refresh_markets().await {
                error!("Failed to refresh Kalshi markets: {}", e);
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

    async fn refresh_markets(&self) -> Result<()> {
        info!("Refreshing Kalshi markets...");
        // Fetch all sports we care about
        let sports = ["nfl", "nba", "nhl", "mlb", "ncaaf", "ncaab"];
        let mut all_markets = Vec::new();

        for sport in sports {
            match self.kalshi_client.get_markets(Some(sport)).await {
                Ok(mut markets) => all_markets.append(&mut markets),
                Err(e) => error!("Error fetching Kalshi {} markets: {}", sport, e),
            }
        }

        // Also generic fetch
        match self.kalshi_client.get_markets(None).await {
            Ok(mut markets) => all_markets.append(&mut markets),
            Err(e) => error!("Error fetching generic Kalshi markets: {}", e),
        }

        // Dedupe
        all_markets.sort_by(|a, b| a.ticker.cmp(&b.ticker));
        all_markets.dedup_by(|a, b| a.ticker == b.ticker);

        let mut cache = self.cache.write().await;
        cache.markets = all_markets;
        cache.last_refresh = Instant::now();
        info!("Refreshed Kalshi markets: {} items", cache.markets.len());

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
        // Heuristic: if description mentions "Game" or "Match" vs "Season"?
        // For now, allow default to false if unknown pattern to be safe, or check Python logic.
        // Python logic: "Unknown pattern - be conservative and skip" -> return False.
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
}
