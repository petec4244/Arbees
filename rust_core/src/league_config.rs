//! League configuration for supported sports.
//!
//! This module provides:
//! - Static configuration for all supported leagues
//! - Market series mappings for Kalshi/Polymarket

use pyo3::prelude::*;

/// Configuration for a single league.
#[derive(Debug, Clone)]
pub struct LeagueConfig {
    /// League code (e.g., "nfl", "epl")
    pub league_code: &'static str,
    /// Polymarket prefix for this league
    pub poly_prefix: &'static str,
    /// Kalshi series for game/match markets
    pub kalshi_series_game: &'static str,
    /// Kalshi series for spread markets (optional)
    pub kalshi_series_spread: Option<&'static str>,
    /// Kalshi series for total/over-under markets (optional)
    pub kalshi_series_total: Option<&'static str>,
    /// Kalshi series for both-teams-to-score markets (soccer, optional)
    pub kalshi_series_btts: Option<&'static str>,
}

/// Static configuration for all supported leagues.
pub static LEAGUE_CONFIGS: &[LeagueConfig] = &[
    // Football
    LeagueConfig {
        league_code: "nfl",
        poly_prefix: "nfl",
        kalshi_series_game: "KXNFLGAME",
        kalshi_series_spread: Some("KXNFLSPREAD"),
        kalshi_series_total: Some("KXNFLTOTAL"),
        kalshi_series_btts: None,
    },
    LeagueConfig {
        league_code: "ncaaf",
        poly_prefix: "cfb",
        kalshi_series_game: "KXNCAAFGAME",
        kalshi_series_spread: Some("KXNCAAFSPREAD"),
        kalshi_series_total: Some("KXNCAAFTOTAL"),
        kalshi_series_btts: None,
    },
    // Basketball
    LeagueConfig {
        league_code: "nba",
        poly_prefix: "nba",
        kalshi_series_game: "KXNBAGAME",
        kalshi_series_spread: Some("KXNBASPREAD"),
        kalshi_series_total: Some("KXNBATOTAL"),
        kalshi_series_btts: None,
    },
    LeagueConfig {
        league_code: "ncaab",
        poly_prefix: "cbb",
        kalshi_series_game: "KXNCAABGAME",
        kalshi_series_spread: Some("KXNCAABSPREAD"),
        kalshi_series_total: Some("KXNCAABTOTAL"),
        kalshi_series_btts: None,
    },
    // Hockey
    LeagueConfig {
        league_code: "nhl",
        poly_prefix: "nhl",
        kalshi_series_game: "KXNHLGAME",
        kalshi_series_spread: Some("KXNHLSPREAD"),
        kalshi_series_total: Some("KXNHLTOTAL"),
        kalshi_series_btts: None,
    },
    // Baseball
    LeagueConfig {
        league_code: "mlb",
        poly_prefix: "mlb",
        kalshi_series_game: "KXMLBGAME",
        kalshi_series_spread: Some("KXMLBSPREAD"),
        kalshi_series_total: Some("KXMLBTOTAL"),
        kalshi_series_btts: None,
    },
    // Soccer - Major Leagues
    LeagueConfig {
        league_code: "epl",
        poly_prefix: "epl",
        kalshi_series_game: "KXEPLGAME",
        kalshi_series_spread: Some("KXEPLSPREAD"),
        kalshi_series_total: Some("KXEPLTOTAL"),
        kalshi_series_btts: Some("KXEPLBTTS"),
    },
    LeagueConfig {
        league_code: "laliga",
        poly_prefix: "laliga",
        kalshi_series_game: "KXLALIGAGAME",
        kalshi_series_spread: Some("KXLALIGASPREAD"),
        kalshi_series_total: Some("KXLALIGATOTAL"),
        kalshi_series_btts: Some("KXLALIGABTTS"),
    },
    LeagueConfig {
        league_code: "bundesliga",
        poly_prefix: "bundesliga",
        kalshi_series_game: "KXBUNDESLIGAGAME",
        kalshi_series_spread: Some("KXBUNDESLIGASPREAD"),
        kalshi_series_total: Some("KXBUNDESLIGATOTAL"),
        kalshi_series_btts: Some("KXBUNDESLIGABTTS"),
    },
    LeagueConfig {
        league_code: "seriea",
        poly_prefix: "seriea",
        kalshi_series_game: "KXSERIEAGAME",
        kalshi_series_spread: Some("KXSERIEASPREAD"),
        kalshi_series_total: Some("KXSERIEATOTAL"),
        kalshi_series_btts: Some("KXSERIEABTTS"),
    },
    LeagueConfig {
        league_code: "ligue1",
        poly_prefix: "ligue1",
        kalshi_series_game: "KXLIGUE1GAME",
        kalshi_series_spread: Some("KXLIGUE1SPREAD"),
        kalshi_series_total: Some("KXLIGUE1TOTAL"),
        kalshi_series_btts: Some("KXLIGUE1BTTS"),
    },
    LeagueConfig {
        league_code: "mls",
        poly_prefix: "mls",
        kalshi_series_game: "KXMLSGAME",
        kalshi_series_spread: Some("KXMLSSPREAD"),
        kalshi_series_total: Some("KXMLSTOTAL"),
        kalshi_series_btts: Some("KXMLSBTTS"),
    },
    // Other sports
    LeagueConfig {
        league_code: "ufc",
        poly_prefix: "ufc",
        kalshi_series_game: "KXUFCFIGHT",
        kalshi_series_spread: None,
        kalshi_series_total: None,
        kalshi_series_btts: None,
    },
];

/// Get league configuration by code.
pub fn get_league_config(league: &str) -> Option<&'static LeagueConfig> {
    LEAGUE_CONFIGS
        .iter()
        .find(|c| c.league_code.eq_ignore_ascii_case(league))
}

/// Get all league configurations.
pub fn get_all_league_configs() -> &'static [LeagueConfig] {
    LEAGUE_CONFIGS
}

/// Get list of all league codes.
pub fn get_all_league_codes() -> Vec<&'static str> {
    LEAGUE_CONFIGS.iter().map(|c| c.league_code).collect()
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for LeagueConfig
#[pyclass(name = "LeagueConfig")]
#[derive(Clone)]
pub struct PyLeagueConfig {
    #[pyo3(get)]
    pub league_code: String,
    #[pyo3(get)]
    pub poly_prefix: String,
    #[pyo3(get)]
    pub kalshi_series_game: String,
    #[pyo3(get)]
    pub kalshi_series_spread: Option<String>,
    #[pyo3(get)]
    pub kalshi_series_total: Option<String>,
    #[pyo3(get)]
    pub kalshi_series_btts: Option<String>,
}

impl From<&LeagueConfig> for PyLeagueConfig {
    fn from(config: &LeagueConfig) -> Self {
        Self {
            league_code: config.league_code.to_string(),
            poly_prefix: config.poly_prefix.to_string(),
            kalshi_series_game: config.kalshi_series_game.to_string(),
            kalshi_series_spread: config.kalshi_series_spread.map(|s| s.to_string()),
            kalshi_series_total: config.kalshi_series_total.map(|s| s.to_string()),
            kalshi_series_btts: config.kalshi_series_btts.map(|s| s.to_string()),
        }
    }
}

/// Get all league configurations (Python function).
#[pyfunction]
#[pyo3(name = "get_league_configs")]
pub fn py_get_league_configs() -> Vec<PyLeagueConfig> {
    LEAGUE_CONFIGS.iter().map(|c| c.into()).collect()
}

/// Get league configuration by code (Python function).
#[pyfunction]
#[pyo3(name = "get_league_config")]
pub fn py_get_league_config(league: &str) -> Option<PyLeagueConfig> {
    get_league_config(league).map(|c| c.into())
}

/// Get all league codes (Python function).
#[pyfunction]
#[pyo3(name = "get_league_codes")]
pub fn py_get_league_codes() -> Vec<String> {
    get_all_league_codes().iter().map(|s| s.to_string()).collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_league_config() {
        let nfl = get_league_config("nfl").unwrap();
        assert_eq!(nfl.league_code, "nfl");
        assert_eq!(nfl.kalshi_series_game, "KXNFLGAME");
    }

    #[test]
    fn test_case_insensitivity() {
        assert!(get_league_config("NFL").is_some());
        assert!(get_league_config("nfl").is_some());
        assert!(get_league_config("Nfl").is_some());
    }

    #[test]
    fn test_missing_league() {
        assert!(get_league_config("nonexistent").is_none());
    }

    #[test]
    fn test_all_leagues_count() {
        // Should have 13 leagues configured
        assert_eq!(LEAGUE_CONFIGS.len(), 13);
    }

    #[test]
    fn test_soccer_has_btts() {
        let epl = get_league_config("epl").unwrap();
        assert!(epl.kalshi_series_btts.is_some());

        let nfl = get_league_config("nfl").unwrap();
        assert!(nfl.kalshi_series_btts.is_none());
    }

    #[test]
    fn test_all_league_codes() {
        let codes = get_all_league_codes();
        assert!(codes.contains(&"nfl"));
        assert!(codes.contains(&"nba"));
        assert!(codes.contains(&"epl"));
    }
}
