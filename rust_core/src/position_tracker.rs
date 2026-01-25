//! Position tracking and P&L calculation.
//!
//! This module provides:
//! - Per-market position tracking
//! - Cost basis and P&L calculation
//! - Position resolution on market settlement

use parking_lot::RwLock;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::PyDict;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Single position leg (one side of a position)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PositionLeg {
    /// Number of contracts (can be fractional)
    pub contracts: f64,
    /// Total cost basis in dollars
    pub cost_basis: f64,
    /// Average price per contract (0-1)
    pub avg_price: f64,
}

impl PositionLeg {
    /// Add to position
    pub fn add(&mut self, contracts: f64, price: f64) {
        let new_cost = contracts * price;
        self.cost_basis += new_cost;
        self.contracts += contracts;
        if self.contracts > 0.0 {
            self.avg_price = self.cost_basis / self.contracts;
        }
    }

    /// Calculate value at resolution (YES won or NO won)
    pub fn resolution_value(&self, won: bool) -> f64 {
        if won {
            self.contracts // Each contract pays $1
        } else {
            0.0
        }
    }
}

/// Full arbitrage position across both platforms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbPosition {
    pub market_id: String,
    pub description: String,
    pub kalshi_yes: PositionLeg,
    pub kalshi_no: PositionLeg,
    pub poly_yes: PositionLeg,
    pub poly_no: PositionLeg,
    pub total_fees: f64,
    pub status: String, // "open", "closed", "resolved"
    pub realized_pnl: Option<f64>,
    pub created_at: i64, // Unix timestamp
}

impl Default for ArbPosition {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl ArbPosition {
    pub fn new(market_id: &str, description: &str) -> Self {
        Self {
            market_id: market_id.to_string(),
            description: description.to_string(),
            kalshi_yes: PositionLeg::default(),
            kalshi_no: PositionLeg::default(),
            poly_yes: PositionLeg::default(),
            poly_no: PositionLeg::default(),
            total_fees: 0.0,
            status: "open".to_string(),
            realized_pnl: None,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Total contracts across all legs
    pub fn total_contracts(&self) -> f64 {
        self.kalshi_yes.contracts
            + self.kalshi_no.contracts
            + self.poly_yes.contracts
            + self.poly_no.contracts
    }

    /// Total cost across all legs plus fees
    pub fn total_cost(&self) -> f64 {
        self.kalshi_yes.cost_basis
            + self.kalshi_no.cost_basis
            + self.poly_yes.cost_basis
            + self.poly_no.cost_basis
            + self.total_fees
    }

    /// Calculate guaranteed profit (minimum payout - cost)
    /// For a perfectly balanced arb, this should be positive
    pub fn guaranteed_profit(&self) -> f64 {
        let matched = self.matched_contracts();
        let guaranteed_payout = matched; // $1 per matched contract
        let cost = self.total_cost();
        guaranteed_payout - cost
    }

    /// Number of fully matched contracts (min of YES total and NO total)
    pub fn matched_contracts(&self) -> f64 {
        let total_yes = self.kalshi_yes.contracts + self.poly_yes.contracts;
        let total_no = self.kalshi_no.contracts + self.poly_no.contracts;
        total_yes.min(total_no)
    }

    /// Unmatched exposure (contracts at risk if market moves)
    pub fn unmatched_exposure(&self) -> f64 {
        let total_yes = self.kalshi_yes.contracts + self.poly_yes.contracts;
        let total_no = self.kalshi_no.contracts + self.poly_no.contracts;
        (total_yes - total_no).abs()
    }

    /// Resolve the position based on outcome
    pub fn resolve(&mut self, yes_won: bool) -> f64 {
        // Calculate payout
        let payout = if yes_won {
            self.kalshi_yes.contracts + self.poly_yes.contracts
        } else {
            self.kalshi_no.contracts + self.poly_no.contracts
        };

        let pnl = payout - self.total_cost();
        self.realized_pnl = Some(pnl);
        self.status = "resolved".to_string();
        pnl
    }
}

/// Position tracker managing all positions
pub struct PositionTracker {
    positions: HashMap<String, ArbPosition>,
    daily_realized_pnl: f64,
    all_time_pnl: f64,
    trading_date: String,
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            daily_realized_pnl: 0.0,
            all_time_pnl: 0.0,
            trading_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        }
    }

    /// Load from JSON file
    pub fn load(path: Option<&str>) -> Self {
        let path = path.unwrap_or("positions.json");
        if !Path::new(path).exists() {
            return Self::new();
        }

        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(tracker) => tracker,
                Err(_) => Self::new(),
            },
            Err(_) => Self::new(),
        }
    }

    /// Save to JSON file
    pub fn save(&self, path: Option<&str>) -> Result<(), std::io::Error> {
        let path = path.unwrap_or("positions.json");
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)
    }

    /// Record a fill (trade execution)
    pub fn record_fill(
        &mut self,
        market_id: &str,
        description: &str,
        platform: &str,
        side: &str,
        contracts: f64,
        price: f64,
        fees: f64,
    ) {
        let pos = self
            .positions
            .entry(market_id.to_string())
            .or_insert_with(|| ArbPosition::new(market_id, description));

        pos.total_fees += fees;

        match (
            platform.to_lowercase().as_str(),
            side.to_lowercase().as_str(),
        ) {
            ("kalshi", "yes") => pos.kalshi_yes.add(contracts, price),
            ("kalshi", "no") => pos.kalshi_no.add(contracts, price),
            ("polymarket" | "poly", "yes") => pos.poly_yes.add(contracts, price),
            ("polymarket" | "poly", "no") => pos.poly_no.add(contracts, price),
            _ => {}
        }
    }

    /// Get position for a market
    pub fn get_position(&self, market_id: &str) -> Option<&ArbPosition> {
        self.positions.get(market_id)
    }

    /// Resolve a position and return P&L
    pub fn resolve_position(&mut self, market_id: &str, yes_won: bool) -> Option<f64> {
        let pos = self.positions.get_mut(market_id)?;
        let pnl = pos.resolve(yes_won);
        self.daily_realized_pnl += pnl;
        self.all_time_pnl += pnl;
        Some(pnl)
    }

    /// Get summary statistics
    pub fn summary(&self) -> PositionSummary {
        let open_positions: Vec<_> = self
            .positions
            .values()
            .filter(|p| p.status == "open")
            .collect();

        let total_exposure: f64 = open_positions.iter().map(|p| p.total_cost()).sum();
        let total_guaranteed: f64 = open_positions.iter().map(|p| p.guaranteed_profit()).sum();
        let total_unmatched: f64 = open_positions.iter().map(|p| p.unmatched_exposure()).sum();

        PositionSummary {
            open_count: open_positions.len(),
            total_exposure,
            total_guaranteed_profit: total_guaranteed,
            total_unmatched_exposure: total_unmatched,
            daily_realized_pnl: self.daily_realized_pnl,
            all_time_pnl: self.all_time_pnl,
        }
    }

    /// Get all open positions
    pub fn open_positions(&self) -> Vec<&ArbPosition> {
        self.positions
            .values()
            .filter(|p| p.status == "open")
            .collect()
    }

    /// Reset daily P&L (call at start of trading day)
    pub fn reset_daily_pnl(&mut self) {
        self.daily_realized_pnl = 0.0;
        self.trading_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    }

    /// Get daily P&L
    pub fn daily_pnl(&self) -> f64 {
        self.daily_realized_pnl
    }

    /// Get all-time P&L
    pub fn all_time_pnl(&self) -> f64 {
        self.all_time_pnl
    }
}

// Implement Serialize/Deserialize for persistence
impl Serialize for PositionTracker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("PositionTracker", 4)?;
        state.serialize_field("positions", &self.positions)?;
        state.serialize_field("daily_realized_pnl", &self.daily_realized_pnl)?;
        state.serialize_field("all_time_pnl", &self.all_time_pnl)?;
        state.serialize_field("trading_date", &self.trading_date)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for PositionTracker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TrackerData {
            positions: HashMap<String, ArbPosition>,
            daily_realized_pnl: f64,
            all_time_pnl: f64,
            trading_date: String,
        }

        let data = TrackerData::deserialize(deserializer)?;
        Ok(Self {
            positions: data.positions,
            daily_realized_pnl: data.daily_realized_pnl,
            all_time_pnl: data.all_time_pnl,
            trading_date: data.trading_date,
        })
    }
}

/// Summary statistics
#[derive(Debug, Clone)]
pub struct PositionSummary {
    pub open_count: usize,
    pub total_exposure: f64,
    pub total_guaranteed_profit: f64,
    pub total_unmatched_exposure: f64,
    pub daily_realized_pnl: f64,
    pub all_time_pnl: f64,
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for ArbPosition
#[cfg_attr(feature = "python", pyclass(name = "ArbPosition"))]
#[derive(Clone)]
pub struct PyArbPosition {
    inner: ArbPosition,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyArbPosition {
    #[cfg_attr(feature = "python", new)]
    fn new(market_id: String, description: String) -> Self {
        Self {
            inner: ArbPosition::new(&market_id, &description),
        }
    }

    #[cfg_attr(feature = "python", getter)]
    fn market_id(&self) -> &str {
        &self.inner.market_id
    }

    #[cfg_attr(feature = "python", getter)]
    fn description(&self) -> &str {
        &self.inner.description
    }

    #[cfg_attr(feature = "python", getter)]
    fn status(&self) -> &str {
        &self.inner.status
    }

    #[cfg_attr(feature = "python", getter)]
    fn total_contracts(&self) -> f64 {
        self.inner.total_contracts()
    }

    #[cfg_attr(feature = "python", getter)]
    fn total_cost(&self) -> f64 {
        self.inner.total_cost()
    }

    #[cfg_attr(feature = "python", getter)]
    fn guaranteed_profit(&self) -> f64 {
        self.inner.guaranteed_profit()
    }

    #[cfg_attr(feature = "python", getter)]
    fn matched_contracts(&self) -> f64 {
        self.inner.matched_contracts()
    }

    #[cfg_attr(feature = "python", getter)]
    fn unmatched_exposure(&self) -> f64 {
        self.inner.unmatched_exposure()
    }

    #[cfg_attr(feature = "python", getter)]
    fn realized_pnl(&self) -> Option<f64> {
        self.inner.realized_pnl
    }

    #[cfg_attr(feature = "python", getter)]
    fn total_fees(&self) -> f64 {
        self.inner.total_fees
    }

    #[cfg(feature = "python")]
    fn to_dict(&self, py: Python) -> PyObject {
        let dict = PyDict::new(py);
        dict.set_item("market_id", &self.inner.market_id).unwrap();
        dict.set_item("description", &self.inner.description)
            .unwrap();
        dict.set_item("status", &self.inner.status).unwrap();
        dict.set_item("total_contracts", self.inner.total_contracts())
            .unwrap();
        dict.set_item("total_cost", self.inner.total_cost())
            .unwrap();
        dict.set_item("guaranteed_profit", self.inner.guaranteed_profit())
            .unwrap();
        dict.set_item("unmatched_exposure", self.inner.unmatched_exposure())
            .unwrap();
        dict.set_item("total_fees", self.inner.total_fees).unwrap();
        dict.set_item("realized_pnl", self.inner.realized_pnl)
            .unwrap();
        dict.into()
    }
}

/// Python wrapper for PositionTracker
#[cfg_attr(feature = "python", pyclass(name = "PositionTracker"))]
pub struct PyPositionTracker {
    inner: Arc<RwLock<PositionTracker>>,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyPositionTracker {
    #[cfg_attr(feature = "python", new)]
    fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(PositionTracker::new())),
        }
    }

    #[cfg_attr(feature = "python", staticmethod)]
    fn load(path: Option<&str>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(PositionTracker::load(path))),
        }
    }

    #[cfg(feature = "python")]
    fn save(&self, path: Option<&str>) -> PyResult<()> {
        self.inner
            .read()
            .save(path)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))
    }

    fn record_fill(
        &self,
        market_id: &str,
        description: &str,
        platform: &str,
        side: &str,
        contracts: f64,
        price: f64,
        fees: f64,
    ) {
        self.inner.write().record_fill(
            market_id,
            description,
            platform,
            side,
            contracts,
            price,
            fees,
        );
    }

    fn get_position(&self, market_id: &str) -> Option<PyArbPosition> {
        self.inner
            .read()
            .get_position(market_id)
            .map(|p| PyArbPosition { inner: p.clone() })
    }

    fn resolve_position(&self, market_id: &str, yes_won: bool) -> Option<f64> {
        self.inner.write().resolve_position(market_id, yes_won)
    }

    #[cfg(feature = "python")]
    fn summary(&self, py: Python) -> PyObject {
        let summary = self.inner.read().summary();
        let dict = PyDict::new(py);
        dict.set_item("open_count", summary.open_count).unwrap();
        dict.set_item("total_exposure", summary.total_exposure)
            .unwrap();
        dict.set_item("total_guaranteed_profit", summary.total_guaranteed_profit)
            .unwrap();
        dict.set_item("total_unmatched_exposure", summary.total_unmatched_exposure)
            .unwrap();
        dict.set_item("daily_realized_pnl", summary.daily_realized_pnl)
            .unwrap();
        dict.set_item("all_time_pnl", summary.all_time_pnl).unwrap();
        dict.into()
    }

    fn daily_pnl(&self) -> f64 {
        self.inner.read().daily_pnl()
    }

    fn all_time_pnl(&self) -> f64 {
        self.inner.read().all_time_pnl()
    }

    fn reset_daily_pnl(&self) {
        self.inner.write().reset_daily_pnl();
    }

    fn open_positions(&self) -> Vec<PyArbPosition> {
        self.inner
            .read()
            .open_positions()
            .iter()
            .map(|p| PyArbPosition {
                inner: (*p).clone(),
            })
            .collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_leg_add() {
        let mut leg = PositionLeg::default();
        leg.add(10.0, 0.45);
        assert!((leg.contracts - 10.0).abs() < 0.01);
        assert!((leg.cost_basis - 4.5).abs() < 0.01);
        assert!((leg.avg_price - 0.45).abs() < 0.01);

        leg.add(5.0, 0.50);
        assert!((leg.contracts - 15.0).abs() < 0.01);
        assert!((leg.cost_basis - 7.0).abs() < 0.01);
        // avg = 7.0 / 15.0 = 0.467
        assert!((leg.avg_price - 0.467).abs() < 0.01);
    }

    #[test]
    fn test_guaranteed_profit() {
        let mut pos = ArbPosition::new("test", "Test Market");

        // Balanced arb: 10 contracts YES @ 40c, 10 contracts NO @ 55c
        pos.kalshi_no.add(10.0, 0.55);
        pos.poly_yes.add(10.0, 0.40);
        pos.total_fees = 0.20; // 2c per contract

        // Cost: 5.5 + 4.0 + 0.2 = 9.7
        // Payout: 10 * 1 = 10.0
        // Profit: 10.0 - 9.7 = 0.3
        assert!((pos.total_cost() - 9.7).abs() < 0.01);
        assert!((pos.guaranteed_profit() - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_resolve_yes_won() {
        let mut pos = ArbPosition::new("test", "Test");
        pos.kalshi_no.add(10.0, 0.55);
        pos.poly_yes.add(10.0, 0.40);
        pos.total_fees = 0.20;

        // YES won: Poly YES pays $10
        let pnl = pos.resolve(true);

        // Payout: 10.0 (poly_yes contracts)
        // Cost: 5.5 + 4.0 + 0.2 = 9.7
        // P&L: 10.0 - 9.7 = 0.3
        assert!((pnl - 0.3).abs() < 0.01);
        assert_eq!(pos.status, "resolved");
    }

    #[test]
    fn test_resolve_no_won() {
        let mut pos = ArbPosition::new("test", "Test");
        pos.kalshi_no.add(10.0, 0.55);
        pos.poly_yes.add(10.0, 0.40);
        pos.total_fees = 0.20;

        // NO won: Kalshi NO pays $10
        let pnl = pos.resolve(false);

        // Payout: 10.0 (kalshi_no contracts)
        // Cost: 5.5 + 4.0 + 0.2 = 9.7
        // P&L: 10.0 - 9.7 = 0.3
        assert!((pnl - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_position_tracker() {
        let mut tracker = PositionTracker::new();

        tracker.record_fill("market1", "Test", "kalshi", "no", 10.0, 0.55, 0.10);
        tracker.record_fill("market1", "Test", "polymarket", "yes", 10.0, 0.40, 0.0);

        let pos = tracker.get_position("market1").unwrap();
        assert!((pos.kalshi_no.contracts - 10.0).abs() < 0.01);
        assert!((pos.poly_yes.contracts - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_tracker_resolve() {
        let mut tracker = PositionTracker::new();

        tracker.record_fill("market1", "Test", "kalshi", "no", 10.0, 0.55, 0.10);
        tracker.record_fill("market1", "Test", "poly", "yes", 10.0, 0.40, 0.0);

        let pnl = tracker.resolve_position("market1", true).unwrap();
        assert!(pnl > 0.0);

        assert!((tracker.daily_pnl() - pnl).abs() < 0.01);
        assert!((tracker.all_time_pnl() - pnl).abs() < 0.01);
    }

    #[test]
    fn test_unmatched_exposure() {
        let mut pos = ArbPosition::new("test", "Test");

        // Unbalanced: more YES than NO
        pos.poly_yes.add(10.0, 0.45);
        pos.kalshi_no.add(8.0, 0.55);

        assert!((pos.matched_contracts() - 8.0).abs() < 0.01);
        assert!((pos.unmatched_exposure() - 2.0).abs() < 0.01);
    }
}
