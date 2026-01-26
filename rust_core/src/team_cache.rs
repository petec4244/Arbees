//! Team code cache for cross-platform matching.
//!
//! This module provides:
//! - Bidirectional lookup: Polymarket <-> Kalshi team codes
//! - League-scoped mappings
//! - JSON persistence

#[cfg(feature = "python")]
use pyo3::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Team cache for cross-platform team code mapping.
/// Maps between Polymarket and Kalshi team codes within a league.
#[derive(Debug, Clone, Default)]
pub struct TeamCache {
    /// "league:poly_code" -> "kalshi_code"
    forward: HashMap<String, String>,
    /// "league:kalshi_code" -> "poly_code"
    reverse: HashMap<String, String>,
}

impl TeamCache {
    /// Create a new empty team cache.
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Load from JSON file.
    pub fn load(path: Option<&str>) -> Self {
        let path = path.unwrap_or("kalshi_team_cache.json");
        if !Path::new(path).exists() {
            return Self::new();
        }

        match fs::read_to_string(path) {
            Ok(content) => {
                // JSON format: { "league": { "poly_code": "kalshi_code", ... }, ... }
                let data: Result<HashMap<String, HashMap<String, String>>, _> =
                    serde_json::from_str(&content);

                match data {
                    Ok(leagues) => {
                        let mut cache = Self::new();
                        for (league, mappings) in leagues {
                            for (poly_code, kalshi_code) in mappings {
                                cache.insert(&league, &poly_code, &kalshi_code);
                            }
                        }
                        cache
                    }
                    Err(_) => Self::new(),
                }
            }
            Err(_) => Self::new(),
        }
    }

    /// Save to JSON file.
    pub fn save(&self, path: Option<&str>) -> Result<(), std::io::Error> {
        let path = path.unwrap_or("kalshi_team_cache.json");

        // Convert to nested format: { "league": { "poly_code": "kalshi_code", ... }, ... }
        let mut leagues: HashMap<String, HashMap<String, String>> = HashMap::new();

        for (key, kalshi_code) in &self.forward {
            if let Some((league, poly_code)) = key.split_once(':') {
                leagues
                    .entry(league.to_string())
                    .or_default()
                    .insert(poly_code.to_string(), kalshi_code.clone());
            }
        }

        let content = serde_json::to_string_pretty(&leagues)?;
        fs::write(path, content)
    }

    /// Convert Polymarket code to Kalshi code.
    pub fn poly_to_kalshi(&self, league: &str, poly_code: &str) -> Option<&str> {
        let key = format!("{}:{}", league.to_lowercase(), poly_code.to_lowercase());
        self.forward.get(&key).map(|s| s.as_str())
    }

    /// Convert Kalshi code to Polymarket code.
    pub fn kalshi_to_poly(&self, league: &str, kalshi_code: &str) -> Option<&str> {
        let key = format!("{}:{}", league.to_lowercase(), kalshi_code.to_lowercase());
        self.reverse.get(&key).map(|s| s.as_str())
    }

    /// Insert a mapping.
    pub fn insert(&mut self, league: &str, poly_code: &str, kalshi_code: &str) {
        let forward_key = format!("{}:{}", league.to_lowercase(), poly_code.to_lowercase());
        let reverse_key = format!("{}:{}", league.to_lowercase(), kalshi_code.to_lowercase());

        self.forward.insert(forward_key, kalshi_code.to_string());
        self.reverse.insert(reverse_key, poly_code.to_string());
    }

    /// Get number of mappings.
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Get all leagues in the cache.
    pub fn leagues(&self) -> Vec<String> {
        let mut leagues: Vec<String> = self
            .forward
            .keys()
            .filter_map(|k| k.split_once(':').map(|(l, _)| l.to_string()))
            .collect();
        leagues.sort();
        leagues.dedup();
        leagues
    }

    /// Get all mappings for a league.
    pub fn get_league_mappings(&self, league: &str) -> Vec<(String, String)> {
        let prefix = format!("{}:", league.to_lowercase());
        self.forward
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| {
                let poly_code = k.strip_prefix(&prefix).unwrap_or("").to_string();
                (poly_code, v.clone())
            })
            .collect()
    }
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for TeamCache
#[cfg(feature = "python")]
#[cfg_attr(feature = "python", pyclass(name = "TeamCache"))]
pub struct PyTeamCache {
    inner: TeamCache,
}

#[cfg(feature = "python")]
#[cfg_attr(feature = "python", pymethods)]
impl PyTeamCache {
    #[cfg_attr(feature = "python", new)]
    fn new() -> Self {
        Self {
            inner: TeamCache::new(),
        }
    }

    #[cfg_attr(feature = "python", staticmethod)]
    fn load(path: Option<&str>) -> Self {
        Self {
            inner: TeamCache::load(path),
        }
    }

    #[cfg(feature = "python")]
    fn save(&self, path: Option<&str>) -> PyResult<()> {
        self.inner
            .save(path)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))
    }

    fn poly_to_kalshi(&self, league: &str, poly_code: &str) -> Option<String> {
        self.inner
            .poly_to_kalshi(league, poly_code)
            .map(|s| s.to_string())
    }

    fn kalshi_to_poly(&self, league: &str, kalshi_code: &str) -> Option<String> {
        self.inner
            .kalshi_to_poly(league, kalshi_code)
            .map(|s| s.to_string())
    }

    fn insert(&mut self, league: &str, poly_code: &str, kalshi_code: &str) {
        self.inner.insert(league, poly_code, kalshi_code);
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn leagues(&self) -> Vec<String> {
        self.inner.leagues()
    }

    fn get_league_mappings(&self, league: &str) -> Vec<(String, String)> {
        self.inner.get_league_mappings(league)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_lookup() {
        let mut cache = TeamCache::new();
        cache.insert("nfl", "kc", "chiefs");
        cache.insert("nfl", "sf", "49ers");

        assert_eq!(cache.poly_to_kalshi("nfl", "kc"), Some("chiefs"));
        assert_eq!(cache.poly_to_kalshi("nfl", "sf"), Some("49ers"));
        assert_eq!(cache.kalshi_to_poly("nfl", "chiefs"), Some("kc"));
        assert_eq!(cache.kalshi_to_poly("nfl", "49ers"), Some("sf"));
    }

    #[test]
    fn test_case_insensitivity() {
        let mut cache = TeamCache::new();
        cache.insert("NFL", "KC", "CHIEFS");

        // Should work with different cases
        assert_eq!(cache.poly_to_kalshi("nfl", "kc"), Some("CHIEFS"));
        assert_eq!(cache.poly_to_kalshi("NFL", "KC"), Some("CHIEFS"));
    }

    #[test]
    fn test_missing_lookup() {
        let cache = TeamCache::new();
        assert_eq!(cache.poly_to_kalshi("nfl", "kc"), None);
        assert_eq!(cache.kalshi_to_poly("nfl", "chiefs"), None);
    }

    #[test]
    fn test_leagues() {
        let mut cache = TeamCache::new();
        cache.insert("nfl", "kc", "chiefs");
        cache.insert("nba", "lal", "lakers");
        cache.insert("epl", "che", "chelsea");

        let leagues = cache.leagues();
        assert!(leagues.contains(&"nfl".to_string()));
        assert!(leagues.contains(&"nba".to_string()));
        assert!(leagues.contains(&"epl".to_string()));
    }

    #[test]
    fn test_get_league_mappings() {
        let mut cache = TeamCache::new();
        cache.insert("nfl", "kc", "chiefs");
        cache.insert("nfl", "sf", "49ers");
        cache.insert("nba", "lal", "lakers");

        let nfl_mappings = cache.get_league_mappings("nfl");
        assert_eq!(nfl_mappings.len(), 2);

        let nba_mappings = cache.get_league_mappings("nba");
        assert_eq!(nba_mappings.len(), 1);
    }
}
