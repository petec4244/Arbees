//! ESPN Event Provider
//!
//! Implements the EventProvider trait for ESPN sports data.
//! Wraps the ESPN API client to provide event discovery and state tracking.

use super::{EventInfo, EventProvider, EventState, EventStatus, StateData, SportStateData};
use crate::clients::espn::{EspnClient as EspnApiClient, Game};
use crate::models::{MarketType, Sport};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// ESPN Event Provider for a specific sport
///
/// Each instance handles one sport (NBA, NFL, etc.) and uses the
/// ESPN API client to fetch game data.
pub struct EspnEventProvider {
    sport: Sport,
    sport_name: &'static str,
    league_name: &'static str,
    client: EspnApiClient,
}

impl EspnEventProvider {
    /// Create a new ESPN provider for a specific sport
    pub fn new(sport: Sport) -> Self {
        let (sport_name, league_name) = match sport {
            Sport::NFL => ("football", "nfl"),
            Sport::NBA => ("basketball", "nba"),
            Sport::NHL => ("hockey", "nhl"),
            Sport::MLB => ("baseball", "mlb"),
            Sport::NCAAF => ("football", "college-football"),
            Sport::NCAAB => ("basketball", "mens-college-basketball"),
            Sport::MLS => ("soccer", "usa.1"),
            Sport::Soccer => ("soccer", "eng.1"), // Premier League as default
            Sport::Tennis => ("tennis", "atp"),
            Sport::MMA => ("mma", "ufc"),
        };

        Self {
            sport,
            sport_name,
            league_name,
            client: EspnApiClient::new(),
        }
    }

    /// Convert ESPN Game to EventInfo
    fn game_to_event_info(&self, game: &Game) -> EventInfo {
        let status = parse_espn_status(&game.status);

        EventInfo {
            event_id: game.id.clone(),
            market_type: MarketType::sport(self.sport),
            entity_a: game.home_team.clone(),
            entity_b: Some(game.away_team.clone()),
            scheduled_time: parse_espn_date(&game.date),
            status,
            venue: None,
            metadata: serde_json::json!({
                "espn_name": game.name,
                "espn_short_name": game.short_name,
                "home_abbr": game.home_abbr,
                "away_abbr": game.away_abbr,
            }),
        }
    }

    /// Convert ESPN Game to EventState
    fn game_to_event_state(&self, game: &Game) -> EventState {
        let status = parse_espn_status(&game.status);

        // Build sport-specific state data
        let sport_details = serde_json::json!({
            "down": game.down,
            "yards_to_go": game.yards_to_go,
            "yard_line": game.yard_line,
            "is_redzone": game.is_redzone,
        });

        let state = StateData::Sport(SportStateData {
            score_a: game.home_score,
            score_b: game.away_score,
            period: game.period,
            time_remaining: game.time_remaining_seconds,
            possession: game.possession.clone(),
            sport_details,
        });

        EventState {
            event_id: game.id.clone(),
            market_type: MarketType::sport(self.sport),
            entity_a: game.home_team.clone(),
            entity_b: Some(game.away_team.clone()),
            status,
            state,
            fetched_at: Utc::now(),
        }
    }
}

#[async_trait]
impl EventProvider for EspnEventProvider {
    async fn get_live_events(&self) -> Result<Vec<EventInfo>> {
        let games = self
            .client
            .get_games(self.sport_name, self.league_name)
            .await?;

        // Filter for live games only
        let live_games: Vec<EventInfo> = games
            .iter()
            .filter(|g| {
                matches!(
                    parse_espn_status(&g.status),
                    EventStatus::Live
                )
            })
            .map(|g| self.game_to_event_info(g))
            .collect();

        Ok(live_games)
    }

    async fn get_scheduled_events(&self, _days: u32) -> Result<Vec<EventInfo>> {
        // ESPN API doesn't have a direct "scheduled only" endpoint
        // We fetch all games and filter for scheduled status
        // TODO: Implement date-range filtering once ESPN API supports it
        let games = self
            .client
            .get_games(self.sport_name, self.league_name)
            .await?;

        let scheduled_games: Vec<EventInfo> = games
            .iter()
            .filter(|g| {
                matches!(
                    parse_espn_status(&g.status),
                    EventStatus::Scheduled
                )
            })
            .map(|g| self.game_to_event_info(g))
            .collect();

        Ok(scheduled_games)
    }

    async fn get_event_state(&self, event_id: &str) -> Result<EventState> {
        let games = self
            .client
            .get_games(self.sport_name, self.league_name)
            .await?;

        let game = games
            .iter()
            .find(|g| g.id == event_id)
            .ok_or_else(|| anyhow!("Game {} not found", event_id))?;

        Ok(self.game_to_event_state(game))
    }

    fn provider_name(&self) -> &str {
        "ESPN"
    }

    fn supported_market_types(&self) -> Vec<MarketType> {
        vec![MarketType::sport(self.sport)]
    }
}

/// Parse ESPN status string to EventStatus
fn parse_espn_status(status: &str) -> EventStatus {
    let status_lower = status.to_lowercase();

    if status_lower.contains("in_progress")
        || status_lower.contains("halftime")
        || status_lower.contains("end_period")
        || status_lower == "live"
    {
        EventStatus::Live
    } else if status_lower.contains("final") || status_lower.contains("completed") {
        EventStatus::Completed
    } else if status_lower.contains("postponed") {
        EventStatus::Postponed
    } else if status_lower.contains("cancelled") || status_lower.contains("canceled") {
        EventStatus::Cancelled
    } else {
        // Default to scheduled for pre-game status
        EventStatus::Scheduled
    }
}

/// Parse ESPN date string to DateTime
fn parse_espn_date(date_str: &str) -> DateTime<Utc> {
    // ESPN date format: "2024-01-15T19:00Z"
    DateTime::parse_from_rfc3339(date_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_espn_status() {
        assert_eq!(parse_espn_status("in_progress"), EventStatus::Live);
        assert_eq!(parse_espn_status("STATUS_IN_PROGRESS"), EventStatus::Live);
        assert_eq!(parse_espn_status("halftime"), EventStatus::Live);
        assert_eq!(parse_espn_status("end_period"), EventStatus::Live);
        assert_eq!(parse_espn_status("final"), EventStatus::Completed);
        assert_eq!(parse_espn_status("STATUS_FINAL"), EventStatus::Completed);
        assert_eq!(parse_espn_status("postponed"), EventStatus::Postponed);
        assert_eq!(parse_espn_status("cancelled"), EventStatus::Cancelled);
        assert_eq!(parse_espn_status("pre_game"), EventStatus::Scheduled);
        assert_eq!(parse_espn_status("scheduled"), EventStatus::Scheduled);
    }

    #[test]
    fn test_espn_provider_creation() {
        let provider = EspnEventProvider::new(Sport::NBA);
        assert_eq!(provider.provider_name(), "ESPN");
        assert_eq!(provider.supported_market_types().len(), 1);
        assert!(provider.supported_market_types()[0].is_sport());
    }

    #[test]
    fn test_sport_mappings() {
        let nfl_provider = EspnEventProvider::new(Sport::NFL);
        assert_eq!(nfl_provider.sport_name, "football");
        assert_eq!(nfl_provider.league_name, "nfl");

        let nba_provider = EspnEventProvider::new(Sport::NBA);
        assert_eq!(nba_provider.sport_name, "basketball");
        assert_eq!(nba_provider.league_name, "nba");

        let nhl_provider = EspnEventProvider::new(Sport::NHL);
        assert_eq!(nhl_provider.sport_name, "hockey");
        assert_eq!(nhl_provider.league_name, "nhl");
    }
}
