//! Atomic orderbook and global state for lock-free price tracking.
//!
//! This module provides:
//! - `AtomicOrderbook` - Lock-free orderbook using packed atomic u64
//! - `GlobalState` - Manages multiple market pairs with fast lookups
//! - Kalshi fee calculation

use parking_lot::RwLock;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::PyDict;
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Kalshi fee table: fee in basis points (0.01%) for each price from 0-100 cents.
/// Fee formula: 7 * price * (100 - price) / 10000, capped appropriately.
/// Pre-computed for O(1) lookups.
pub static KALSHI_FEE_TABLE: [u8; 101] = {
    let mut table = [0u8; 101];
    let mut i = 0;
    while i <= 100 {
        // fee = ceil(7 * p * (100 - p) / 10000)
        let p = i as u32;
        let fee_raw = 7 * p * (100 - p);
        let fee = (fee_raw + 9999) / 10000; // ceiling division
        table[i] = fee as u8;
        i += 1;
    }
    table
};

/// Calculate Kalshi fee in cents for a given price in cents (0-100).
#[inline]
pub fn kalshi_fee_cents(price: u16) -> u16 {
    if price > 100 {
        return 0;
    }
    KALSHI_FEE_TABLE[price as usize] as u16
}

/// Pack four u16 values into a single u64 for atomic operations.
/// Layout: [yes_ask:16][no_ask:16][yes_size:16][no_size:16]
#[inline]
pub fn pack_orderbook(yes_ask: u16, no_ask: u16, yes_size: u16, no_size: u16) -> u64 {
    ((yes_ask as u64) << 48)
        | ((no_ask as u64) << 32)
        | ((yes_size as u64) << 16)
        | (no_size as u64)
}

/// Unpack a u64 into four u16 values.
/// Returns: (yes_ask, no_ask, yes_size, no_size)
#[inline]
pub fn unpack_orderbook(packed: u64) -> (u16, u16, u16, u16) {
    let yes_ask = (packed >> 48) as u16;
    let no_ask = (packed >> 32) as u16;
    let yes_size = (packed >> 16) as u16;
    let no_size = packed as u16;
    (yes_ask, no_ask, yes_size, no_size)
}

/// Lock-free orderbook using a single atomic u64.
/// All four values (yes_ask, no_ask, yes_size, no_size) are updated atomically.
#[derive(Debug)]
pub struct AtomicOrderbook {
    packed: AtomicU64,
}

impl Default for AtomicOrderbook {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicOrderbook {
    /// Create a new empty orderbook (all zeros).
    pub fn new() -> Self {
        Self {
            packed: AtomicU64::new(0),
        }
    }

    /// Load current orderbook values atomically.
    /// Returns: (yes_ask, no_ask, yes_size, no_size)
    #[inline]
    pub fn load(&self) -> (u16, u16, u16, u16) {
        unpack_orderbook(self.packed.load(Ordering::SeqCst))
    }

    /// Store new orderbook values atomically.
    #[inline]
    pub fn store(&self, yes_ask: u16, no_ask: u16, yes_size: u16, no_size: u16) {
        self.packed.store(
            pack_orderbook(yes_ask, no_ask, yes_size, no_size),
            Ordering::SeqCst,
        );
    }

    /// Update only the YES side (ask and size).
    #[inline]
    pub fn update_yes(&self, yes_ask: u16, yes_size: u16) {
        loop {
            let current = self.packed.load(Ordering::SeqCst);
            let (_, no_ask, _, no_size) = unpack_orderbook(current);
            let new = pack_orderbook(yes_ask, no_ask, yes_size, no_size);
            if self
                .packed
                .compare_exchange(current, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Update only the NO side (ask and size).
    #[inline]
    pub fn update_no(&self, no_ask: u16, no_size: u16) {
        loop {
            let current = self.packed.load(Ordering::SeqCst);
            let (yes_ask, _, yes_size, _) = unpack_orderbook(current);
            let new = pack_orderbook(yes_ask, no_ask, yes_size, no_size);
            if self
                .packed
                .compare_exchange(current, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Get raw packed value (for debugging).
    pub fn raw(&self) -> u64 {
        self.packed.load(Ordering::SeqCst)
    }
}

/// Combined market state for both Kalshi and Polymarket orderbooks.
#[derive(Debug)]
pub struct AtomicMarketState {
    pub kalshi: AtomicOrderbook,
    pub poly: AtomicOrderbook,
}

impl Default for AtomicMarketState {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicMarketState {
    pub fn new() -> Self {
        Self {
            kalshi: AtomicOrderbook::new(),
            poly: AtomicOrderbook::new(),
        }
    }
}

/// Market pair metadata for tracking.
#[derive(Debug, Clone)]
pub struct MarketPair {
    pub kalshi_ticker: String,
    pub poly_condition_id: String,
    pub description: String,
    pub league: String,
}

/// Global state managing all market pairs with fast lookups.
/// Uses fixed-size array for O(1) access by market_id and hash maps for ticker lookups.
pub struct GlobalState {
    /// Fixed array of market states (max 1024 markets).
    markets: Box<[AtomicMarketState; 1024]>,
    /// Metadata for each market.
    metadata: RwLock<Vec<Option<MarketPair>>>,
    /// Kalshi ticker -> market_id lookup.
    kalshi_lookup: RwLock<FxHashMap<String, u16>>,
    /// Polymarket condition_id -> market_id lookup.
    poly_lookup: RwLock<FxHashMap<String, u16>>,
    /// Next available market_id.
    next_id: RwLock<u16>,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalState {
    /// Create a new global state.
    pub fn new() -> Self {
        // Initialize 1024 AtomicMarketState instances
        let markets: Vec<AtomicMarketState> = (0..1024).map(|_| AtomicMarketState::new()).collect();
        let markets_array: Box<[AtomicMarketState; 1024]> =
            markets.into_boxed_slice().try_into().unwrap();

        let mut metadata_vec = Vec::with_capacity(1024);
        metadata_vec.resize_with(1024, || None);

        Self {
            markets: markets_array,
            metadata: RwLock::new(metadata_vec),
            kalshi_lookup: RwLock::new(FxHashMap::default()),
            poly_lookup: RwLock::new(FxHashMap::default()),
            next_id: RwLock::new(0),
        }
    }

    /// Add a new market pair. Returns the assigned market_id or None if at capacity.
    pub fn add_pair(&self, pair: MarketPair) -> Option<u16> {
        let mut next_id = self.next_id.write();
        if *next_id >= 1024 {
            return None;
        }

        let market_id = *next_id;
        *next_id += 1;

        // Update lookups
        self.kalshi_lookup
            .write()
            .insert(pair.kalshi_ticker.clone(), market_id);
        self.poly_lookup
            .write()
            .insert(pair.poly_condition_id.clone(), market_id);

        // Store metadata
        self.metadata.write()[market_id as usize] = Some(pair);

        Some(market_id)
    }

    /// Get market state by market_id.
    pub fn get_by_id(&self, market_id: u16) -> Option<&AtomicMarketState> {
        if market_id < 1024 {
            Some(&self.markets[market_id as usize])
        } else {
            None
        }
    }

    /// Get market_id by Kalshi ticker.
    pub fn get_id_by_kalshi(&self, ticker: &str) -> Option<u16> {
        self.kalshi_lookup.read().get(ticker).copied()
    }

    /// Get market_id by Polymarket condition_id.
    pub fn get_id_by_poly(&self, condition_id: &str) -> Option<u16> {
        self.poly_lookup.read().get(condition_id).copied()
    }

    /// Get metadata for a market.
    pub fn get_metadata(&self, market_id: u16) -> Option<MarketPair> {
        if market_id < 1024 {
            self.metadata.read()[market_id as usize].clone()
        } else {
            None
        }
    }

    /// Update Kalshi orderbook for a market.
    pub fn update_kalshi(
        &self,
        market_id: u16,
        yes_ask: u16,
        no_ask: u16,
        yes_size: u16,
        no_size: u16,
    ) {
        if market_id < 1024 {
            self.markets[market_id as usize]
                .kalshi
                .store(yes_ask, no_ask, yes_size, no_size);
        }
    }

    /// Update Polymarket orderbook for a market.
    pub fn update_poly(
        &self,
        market_id: u16,
        yes_ask: u16,
        no_ask: u16,
        yes_size: u16,
        no_size: u16,
    ) {
        if market_id < 1024 {
            self.markets[market_id as usize]
                .poly
                .store(yes_ask, no_ask, yes_size, no_size);
        }
    }

    /// Get both orderbooks for a market.
    /// Returns: ((k_yes, k_no, k_yes_sz, k_no_sz), (p_yes, p_no, p_yes_sz, p_no_sz))
    pub fn get_both(&self, market_id: u16) -> Option<((u16, u16, u16, u16), (u16, u16, u16, u16))> {
        if market_id < 1024 {
            let kalshi = self.markets[market_id as usize].kalshi.load();
            let poly = self.markets[market_id as usize].poly.load();
            Some((kalshi, poly))
        } else {
            None
        }
    }

    /// Get total number of markets.
    pub fn market_count(&self) -> usize {
        *self.next_id.read() as usize
    }
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for AtomicOrderbook.
#[cfg_attr(feature = "python", pyclass(name = "AtomicOrderbook"))]
pub struct PyAtomicOrderbook {
    inner: AtomicOrderbook,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyAtomicOrderbook {
    #[cfg_attr(feature = "python", new)]
    fn new() -> Self {
        Self {
            inner: AtomicOrderbook::new(),
        }
    }

    /// Load current values: (yes_ask, no_ask, yes_size, no_size)
    fn load(&self) -> (u16, u16, u16, u16) {
        self.inner.load()
    }

    /// Store new values atomically.
    fn store(&self, yes_ask: u16, no_ask: u16, yes_size: u16, no_size: u16) {
        self.inner.store(yes_ask, no_ask, yes_size, no_size);
    }

    /// Update YES side only.
    fn update_yes(&self, yes_ask: u16, yes_size: u16) {
        self.inner.update_yes(yes_ask, yes_size);
    }

    /// Update NO side only.
    fn update_no(&self, no_ask: u16, no_size: u16) {
        self.inner.update_no(no_ask, no_size);
    }
}

/// Python wrapper for GlobalState.
#[cfg_attr(feature = "python", pyclass(name = "GlobalState"))]
pub struct PyGlobalState {
    inner: Arc<GlobalState>,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyGlobalState {
    #[cfg_attr(feature = "python", new)]
    fn new() -> Self {
        Self {
            inner: Arc::new(GlobalState::new()),
        }
    }

    /// Add a new market pair. Returns market_id or None if at capacity.
    fn add_pair(
        &self,
        kalshi_ticker: String,
        poly_condition_id: String,
        description: String,
        league: String,
    ) -> Option<u16> {
        let pair = MarketPair {
            kalshi_ticker,
            poly_condition_id,
            description,
            league,
        };
        self.inner.add_pair(pair)
    }

    /// Get market_id by Kalshi ticker.
    fn get_id_by_kalshi(&self, ticker: &str) -> Option<u16> {
        self.inner.get_id_by_kalshi(ticker)
    }

    /// Get market_id by Polymarket condition_id.
    fn get_id_by_poly(&self, condition_id: &str) -> Option<u16> {
        self.inner.get_id_by_poly(condition_id)
    }

    /// Get metadata for a market as a dict.
    #[cfg(feature = "python")]
    fn get_metadata(&self, py: Python, market_id: u16) -> Option<PyObject> {
        self.inner.get_metadata(market_id).map(|m| {
            let dict = PyDict::new(py);
            dict.set_item("kalshi_ticker", &m.kalshi_ticker).unwrap();
            dict.set_item("poly_condition_id", &m.poly_condition_id)
                .unwrap();
            dict.set_item("description", &m.description).unwrap();
            dict.set_item("league", &m.league).unwrap();
            dict.into()
        })
    }

    /// Update Kalshi orderbook.
    fn update_kalshi(
        &self,
        market_id: u16,
        yes_ask: u16,
        no_ask: u16,
        yes_size: u16,
        no_size: u16,
    ) {
        self.inner
            .update_kalshi(market_id, yes_ask, no_ask, yes_size, no_size);
    }

    /// Update Polymarket orderbook.
    fn update_poly(&self, market_id: u16, yes_ask: u16, no_ask: u16, yes_size: u16, no_size: u16) {
        self.inner
            .update_poly(market_id, yes_ask, no_ask, yes_size, no_size);
    }

    /// Get both orderbooks as ((k_yes, k_no, k_yes_sz, k_no_sz), (p_yes, p_no, p_yes_sz, p_no_sz))
    fn get_both(&self, market_id: u16) -> Option<((u16, u16, u16, u16), (u16, u16, u16, u16))> {
        self.inner.get_both(market_id)
    }

    /// Get total number of markets.
    fn market_count(&self) -> usize {
        self.inner.market_count()
    }
}

/// Calculate Kalshi fee for a price (Python function).
#[cfg_attr(feature = "python", pyfunction)]
pub fn py_kalshi_fee_cents(price: u16) -> u16 {
    kalshi_fee_cents(price)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_kalshi_fee() {
        // At price 50: fee = ceil(7 * 50 * 50 / 10000) = ceil(1.75) = 2
        assert_eq!(kalshi_fee_cents(50), 2);
        // At price 0: fee = 0
        assert_eq!(kalshi_fee_cents(0), 0);
        // At price 100: fee = 0
        assert_eq!(kalshi_fee_cents(100), 0);
        // At price 25: fee = ceil(7 * 25 * 75 / 10000) = ceil(1.3125) = 2
        assert_eq!(kalshi_fee_cents(25), 2);
        // At price 10: fee = ceil(7 * 10 * 90 / 10000) = ceil(0.63) = 1
        assert_eq!(kalshi_fee_cents(10), 1);
    }

    #[test]
    fn test_pack_unpack_roundtrip() {
        let (y_ask, n_ask, y_size, n_size) = (45, 55, 1000, 800);
        let packed = pack_orderbook(y_ask, n_ask, y_size, n_size);
        assert_eq!(unpack_orderbook(packed), (y_ask, n_ask, y_size, n_size));

        // Test edge cases
        let packed_max = pack_orderbook(u16::MAX, u16::MAX, u16::MAX, u16::MAX);
        assert_eq!(
            unpack_orderbook(packed_max),
            (u16::MAX, u16::MAX, u16::MAX, u16::MAX)
        );

        let packed_zero = pack_orderbook(0, 0, 0, 0);
        assert_eq!(unpack_orderbook(packed_zero), (0, 0, 0, 0));
    }

    #[test]
    fn test_atomic_orderbook_store_load() {
        let ob = AtomicOrderbook::new();
        assert_eq!(ob.load(), (0, 0, 0, 0));

        ob.store(45, 55, 1000, 800);
        assert_eq!(ob.load(), (45, 55, 1000, 800));
    }

    #[test]
    fn test_atomic_orderbook_update_yes() {
        let ob = AtomicOrderbook::new();
        ob.store(45, 55, 1000, 800);
        ob.update_yes(48, 1200);
        assert_eq!(ob.load(), (48, 55, 1200, 800));
    }

    #[test]
    fn test_atomic_orderbook_update_no() {
        let ob = AtomicOrderbook::new();
        ob.store(45, 55, 1000, 800);
        ob.update_no(52, 900);
        assert_eq!(ob.load(), (45, 52, 1000, 900));
    }

    #[test]
    fn test_atomic_concurrent_update() {
        use std::sync::atomic::AtomicU32;

        let ob = Arc::new(AtomicOrderbook::new());
        ob.store(50, 50, 100, 100);

        let success_count = Arc::new(AtomicU32::new(0));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ob = ob.clone();
                let sc = success_count.clone();
                thread::spawn(move || {
                    // Each thread tries to update with different values
                    ob.update_yes(50 + i as u16, 100 + i as u16);
                    sc.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // All threads should have succeeded
        assert_eq!(success_count.load(Ordering::SeqCst), 10);

        // Verify no data corruption - values should be valid
        let (yes_ask, no_ask, yes_size, no_size) = ob.load();
        assert!(yes_ask >= 50 && yes_ask < 60);
        assert_eq!(no_ask, 50);
        assert!(yes_size >= 100 && yes_size < 110);
        assert_eq!(no_size, 100);
    }

    #[test]
    fn test_global_state_add_lookup() {
        let state = GlobalState::new();
        let pair = MarketPair {
            kalshi_ticker: "KXNFL-KC".to_string(),
            poly_condition_id: "0x123abc".to_string(),
            description: "KC Chiefs win".to_string(),
            league: "nfl".to_string(),
        };

        let id = state.add_pair(pair).unwrap();
        assert_eq!(id, 0);

        // Test Kalshi lookup
        assert_eq!(state.get_id_by_kalshi("KXNFL-KC"), Some(0));
        assert_eq!(state.get_id_by_kalshi("NONEXISTENT"), None);

        // Test Poly lookup
        assert_eq!(state.get_id_by_poly("0x123abc"), Some(0));
        assert_eq!(state.get_id_by_poly("NONEXISTENT"), None);

        // Test metadata
        let meta = state.get_metadata(0).unwrap();
        assert_eq!(meta.kalshi_ticker, "KXNFL-KC");
        assert_eq!(meta.description, "KC Chiefs win");

        // Test market count
        assert_eq!(state.market_count(), 1);
    }

    #[test]
    fn test_global_state_update_orderbooks() {
        let state = GlobalState::new();
        let pair = MarketPair {
            kalshi_ticker: "TEST".to_string(),
            poly_condition_id: "0xtest".to_string(),
            description: "Test".to_string(),
            league: "test".to_string(),
        };

        let id = state.add_pair(pair).unwrap();

        // Update Kalshi
        state.update_kalshi(id, 45, 55, 1000, 800);

        // Update Poly
        state.update_poly(id, 40, 60, 900, 700);

        // Get both
        let (kalshi, poly) = state.get_both(id).unwrap();
        assert_eq!(kalshi, (45, 55, 1000, 800));
        assert_eq!(poly, (40, 60, 900, 700));
    }
}
