//! Execution tracking and deduplication.
//!
//! This module provides:
//! - In-flight execution tracking via atomic bitmask
//! - Fast execution request with profit calculation
//! - High-precision timing

use crate::atomic_orderbook::kalshi_fee_cents;
use pyo3::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Execution tracker using 512-bit atomic bitmask.
/// Each bit represents whether an execution is in-flight for a market_id.
/// This provides lock-free deduplication for up to 512 concurrent markets.
pub struct ExecutionTracker {
    /// 8 x 64-bit words = 512 bits
    in_flight: [AtomicU64; 8],
    /// Clock start time for high-precision timing
    clock_start: Instant,
}

impl Default for ExecutionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionTracker {
    /// Create a new execution tracker.
    pub fn new() -> Self {
        Self {
            in_flight: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            clock_start: Instant::now(),
        }
    }

    /// Try to acquire slot for market_id. Returns true if acquired.
    /// Uses atomic compare-and-swap for lock-free operation.
    #[inline]
    pub fn try_acquire(&self, market_id: u16) -> bool {
        if market_id >= 512 {
            return false;
        }

        let word_idx = market_id as usize / 64;
        let bit_idx = market_id as usize % 64;
        let mask = 1u64 << bit_idx;

        let prev = self.in_flight[word_idx].fetch_or(mask, Ordering::SeqCst);
        (prev & mask) == 0 // True if bit was not previously set
    }

    /// Release slot for market_id.
    #[inline]
    pub fn release(&self, market_id: u16) {
        if market_id >= 512 {
            return;
        }

        let word_idx = market_id as usize / 64;
        let bit_idx = market_id as usize % 64;
        let mask = !(1u64 << bit_idx);

        self.in_flight[word_idx].fetch_and(mask, Ordering::SeqCst);
    }

    /// Check if a market is currently in-flight.
    #[inline]
    pub fn is_in_flight(&self, market_id: u16) -> bool {
        if market_id >= 512 {
            return false;
        }

        let word_idx = market_id as usize / 64;
        let bit_idx = market_id as usize % 64;
        let mask = 1u64 << bit_idx;

        (self.in_flight[word_idx].load(Ordering::SeqCst) & mask) != 0
    }

    /// Get count of in-flight executions.
    pub fn in_flight_count(&self) -> u32 {
        self.in_flight
            .iter()
            .map(|word| word.load(Ordering::SeqCst).count_ones())
            .sum()
    }

    /// Get nanoseconds since tracker creation.
    #[inline]
    pub fn now_ns(&self) -> u64 {
        self.clock_start.elapsed().as_nanos() as u64
    }

    /// Get microseconds since tracker creation.
    #[inline]
    pub fn now_us(&self) -> u64 {
        self.clock_start.elapsed().as_micros() as u64
    }

    /// Reset all in-flight markers.
    pub fn reset(&self) {
        for word in &self.in_flight {
            word.store(0, Ordering::SeqCst);
        }
    }
}

/// Arbitrage type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ArbType {
    PolyYesKalshiNo = 0,
    KalshiYesPolyNo = 1,
    PolyOnly = 2,
    KalshiOnly = 3,
}

impl ArbType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ArbType::PolyYesKalshiNo),
            1 => Some(ArbType::KalshiYesPolyNo),
            2 => Some(ArbType::PolyOnly),
            3 => Some(ArbType::KalshiOnly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ArbType::PolyYesKalshiNo => "PolyYes+KalshiNo",
            ArbType::KalshiYesPolyNo => "KalshiYes+PolyNo",
            ArbType::PolyOnly => "PolyOnly",
            ArbType::KalshiOnly => "KalshiOnly",
        }
    }
}

/// Fast execution request with profit calculation.
#[derive(Debug, Clone)]
pub struct FastExecutionRequest {
    pub market_id: u16,
    pub yes_price: u16,   // Price in cents
    pub no_price: u16,
    pub yes_size: u16,    // Size in cents (dollar amount × 100)
    pub no_size: u16,
    pub arb_type: ArbType,
    pub detected_ns: u64, // Timestamp when arb was detected
}

impl FastExecutionRequest {
    /// Create a new execution request.
    pub fn new(
        market_id: u16,
        yes_price: u16,
        no_price: u16,
        yes_size: u16,
        no_size: u16,
        arb_type: ArbType,
        detected_ns: u64,
    ) -> Self {
        Self {
            market_id,
            yes_price,
            no_price,
            yes_size,
            no_size,
            arb_type,
            detected_ns,
        }
    }

    /// Calculate profit in cents (after fees).
    /// Positive means profit, negative means loss.
    pub fn profit_cents(&self) -> i16 {
        let cost = match self.arb_type {
            ArbType::PolyYesKalshiNo => {
                // Buy YES on Poly (no fee) + Buy NO on Kalshi (fee)
                let fee = kalshi_fee_cents(self.no_price);
                self.yes_price + self.no_price + fee
            }
            ArbType::KalshiYesPolyNo => {
                // Buy YES on Kalshi (fee) + Buy NO on Poly (no fee)
                let fee = kalshi_fee_cents(self.yes_price);
                self.yes_price + fee + self.no_price
            }
            ArbType::PolyOnly => {
                // Buy YES + NO on Poly (no fees)
                self.yes_price + self.no_price
            }
            ArbType::KalshiOnly => {
                // Buy YES + NO on Kalshi (both fees)
                let yes_fee = kalshi_fee_cents(self.yes_price);
                let no_fee = kalshi_fee_cents(self.no_price);
                self.yes_price + yes_fee + self.no_price + no_fee
            }
        };

        100i16 - cost as i16
    }

    /// Calculate estimated total fee in cents.
    pub fn estimated_fee_cents(&self) -> u16 {
        match self.arb_type {
            ArbType::PolyYesKalshiNo => kalshi_fee_cents(self.no_price),
            ArbType::KalshiYesPolyNo => kalshi_fee_cents(self.yes_price),
            ArbType::PolyOnly => 0,
            ArbType::KalshiOnly => {
                kalshi_fee_cents(self.yes_price) + kalshi_fee_cents(self.no_price)
            }
        }
    }

    /// Calculate max contracts based on minimum liquidity.
    /// Size is in cents, so divide by 100 to get contract count.
    pub fn max_contracts(&self) -> i64 {
        std::cmp::min(self.yes_size, self.no_size) as i64 / 100
    }

    /// Calculate expected total profit for max contracts.
    pub fn expected_profit_cents(&self) -> i64 {
        self.profit_cents() as i64 * self.max_contracts()
    }
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for ExecutionTracker.
#[pyclass(name = "ExecutionTracker")]
pub struct PyExecutionTracker {
    inner: ExecutionTracker,
}

#[pymethods]
impl PyExecutionTracker {
    #[new]
    fn new() -> Self {
        Self {
            inner: ExecutionTracker::new(),
        }
    }

    /// Try to acquire slot. Returns true if acquired.
    fn try_acquire(&self, market_id: u16) -> bool {
        self.inner.try_acquire(market_id)
    }

    /// Release slot.
    fn release(&self, market_id: u16) {
        self.inner.release(market_id);
    }

    /// Check if in-flight.
    fn is_in_flight(&self, market_id: u16) -> bool {
        self.inner.is_in_flight(market_id)
    }

    /// Get count of in-flight executions.
    fn in_flight_count(&self) -> u32 {
        self.inner.in_flight_count()
    }

    /// Get nanoseconds since creation.
    fn now_ns(&self) -> u64 {
        self.inner.now_ns()
    }

    /// Get microseconds since creation.
    fn now_us(&self) -> u64 {
        self.inner.now_us()
    }

    /// Reset all in-flight markers.
    fn reset(&self) {
        self.inner.reset();
    }
}

/// Python wrapper for FastExecutionRequest.
#[pyclass(name = "FastExecutionRequest")]
#[derive(Clone)]
pub struct PyFastExecutionRequest {
    #[pyo3(get, set)]
    pub market_id: u16,
    #[pyo3(get, set)]
    pub yes_price: u16,
    #[pyo3(get, set)]
    pub no_price: u16,
    #[pyo3(get, set)]
    pub yes_size: u16,
    #[pyo3(get, set)]
    pub no_size: u16,
    #[pyo3(get, set)]
    pub arb_type: u8,
    #[pyo3(get, set)]
    pub detected_ns: u64,
}

#[pymethods]
impl PyFastExecutionRequest {
    #[new]
    #[pyo3(signature = (market_id, yes_price, no_price, yes_size, no_size, arb_type, detected_ns = 0))]
    fn new(
        market_id: u16,
        yes_price: u16,
        no_price: u16,
        yes_size: u16,
        no_size: u16,
        arb_type: u8,
        detected_ns: u64,
    ) -> Self {
        Self {
            market_id,
            yes_price,
            no_price,
            yes_size,
            no_size,
            arb_type,
            detected_ns,
        }
    }

    /// Calculate profit in cents.
    fn profit_cents(&self) -> i16 {
        let arb_type = ArbType::from_u8(self.arb_type).unwrap_or(ArbType::PolyYesKalshiNo);
        let req = FastExecutionRequest::new(
            self.market_id,
            self.yes_price,
            self.no_price,
            self.yes_size,
            self.no_size,
            arb_type,
            self.detected_ns,
        );
        req.profit_cents()
    }

    /// Calculate estimated fee in cents.
    fn estimated_fee_cents(&self) -> u16 {
        let arb_type = ArbType::from_u8(self.arb_type).unwrap_or(ArbType::PolyYesKalshiNo);
        let req = FastExecutionRequest::new(
            self.market_id,
            self.yes_price,
            self.no_price,
            self.yes_size,
            self.no_size,
            arb_type,
            self.detected_ns,
        );
        req.estimated_fee_cents()
    }

    /// Calculate max contracts.
    fn max_contracts(&self) -> i64 {
        let arb_type = ArbType::from_u8(self.arb_type).unwrap_or(ArbType::PolyYesKalshiNo);
        let req = FastExecutionRequest::new(
            self.market_id,
            self.yes_price,
            self.no_price,
            self.yes_size,
            self.no_size,
            arb_type,
            self.detected_ns,
        );
        req.max_contracts()
    }

    /// Calculate expected profit in cents.
    fn expected_profit_cents(&self) -> i64 {
        let arb_type = ArbType::from_u8(self.arb_type).unwrap_or(ArbType::PolyYesKalshiNo);
        let req = FastExecutionRequest::new(
            self.market_id,
            self.yes_price,
            self.no_price,
            self.yes_size,
            self.no_size,
            arb_type,
            self.detected_ns,
        );
        req.expected_profit_cents()
    }

    /// Get arb type as string.
    fn arb_type_str(&self) -> &'static str {
        ArbType::from_u8(self.arb_type)
            .unwrap_or(ArbType::PolyYesKalshiNo)
            .as_str()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_acquire_release() {
        let tracker = ExecutionTracker::new();
        assert!(tracker.try_acquire(0));
        assert!(!tracker.try_acquire(0)); // Already acquired
        tracker.release(0);
        assert!(tracker.try_acquire(0)); // Can acquire again
    }

    #[test]
    fn test_is_in_flight() {
        let tracker = ExecutionTracker::new();
        assert!(!tracker.is_in_flight(42));
        tracker.try_acquire(42);
        assert!(tracker.is_in_flight(42));
        tracker.release(42);
        assert!(!tracker.is_in_flight(42));
    }

    #[test]
    fn test_in_flight_count() {
        let tracker = ExecutionTracker::new();
        assert_eq!(tracker.in_flight_count(), 0);

        tracker.try_acquire(0);
        tracker.try_acquire(100);
        tracker.try_acquire(200);
        assert_eq!(tracker.in_flight_count(), 3);

        tracker.release(100);
        assert_eq!(tracker.in_flight_count(), 2);
    }

    #[test]
    fn test_concurrent_acquire() {
        use std::sync::atomic::AtomicU64;

        let tracker = Arc::new(ExecutionTracker::new());
        let acquired = Arc::new(AtomicU64::new(0));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let t = tracker.clone();
                let a = acquired.clone();
                thread::spawn(move || {
                    if t.try_acquire(42) {
                        a.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Only one thread should have succeeded
        assert_eq!(acquired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_reset() {
        let tracker = ExecutionTracker::new();
        tracker.try_acquire(0);
        tracker.try_acquire(100);
        tracker.try_acquire(200);
        assert_eq!(tracker.in_flight_count(), 3);

        tracker.reset();
        assert_eq!(tracker.in_flight_count(), 0);
    }

    #[test]
    fn test_profit_calculation() {
        // PolyYes=40, KalshiNo=55, fee=2 -> cost=97, profit=3
        let req = FastExecutionRequest::new(
            0,
            40,  // yes_price
            55,  // no_price
            1000, // yes_size
            800,  // no_size
            ArbType::PolyYesKalshiNo,
            0,
        );

        assert_eq!(req.profit_cents(), 3, "Expected 3¢ profit");
        assert_eq!(req.max_contracts(), 8); // min(1000, 800) / 100
        assert_eq!(req.expected_profit_cents(), 24); // 3 * 8
    }

    #[test]
    fn test_kalshi_yes_poly_no_profit() {
        // KalshiYes=40 (fee~2), PolyNo=55 -> cost=97, profit=3
        let req = FastExecutionRequest::new(
            0,
            40,
            55,
            1000,
            800,
            ArbType::KalshiYesPolyNo,
            0,
        );

        assert_eq!(req.profit_cents(), 3);
    }

    #[test]
    fn test_poly_only_profit() {
        // PolyYes=45, PolyNo=50 -> cost=95, profit=5 (no fees)
        let req = FastExecutionRequest::new(
            0,
            45,
            50,
            1000,
            1000,
            ArbType::PolyOnly,
            0,
        );

        assert_eq!(req.profit_cents(), 5);
        assert_eq!(req.estimated_fee_cents(), 0);
    }

    #[test]
    fn test_kalshi_only_profit() {
        // KalshiYes=40 (fee~2), KalshiNo=50 (fee~2) -> cost=94, profit=6
        let req = FastExecutionRequest::new(
            0,
            40,
            50,
            1000,
            1000,
            ArbType::KalshiOnly,
            0,
        );

        // fee(40) = ceil(7*40*60/10000) = ceil(1.68) = 2
        // fee(50) = ceil(7*50*50/10000) = ceil(1.75) = 2
        // cost = 40 + 2 + 50 + 2 = 94
        // profit = 100 - 94 = 6
        assert_eq!(req.profit_cents(), 6);
        assert_eq!(req.estimated_fee_cents(), 4);
    }

    #[test]
    fn test_timing() {
        let tracker = ExecutionTracker::new();
        let start_ns = tracker.now_ns();

        // Small delay
        std::thread::sleep(std::time::Duration::from_micros(100));

        let end_ns = tracker.now_ns();
        assert!(end_ns > start_ns, "Time should increase");
    }

    #[test]
    fn test_edge_market_ids() {
        let tracker = ExecutionTracker::new();

        // Test boundary values
        assert!(tracker.try_acquire(0));
        assert!(tracker.try_acquire(511));
        assert!(!tracker.try_acquire(512)); // Out of bounds

        tracker.release(0);
        tracker.release(511);
    }
}
