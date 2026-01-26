//! Circuit breaker for trading risk management.
//!
//! This module provides:
//! - Position limits per market and total
//! - Daily loss limits
//! - Consecutive error tracking
//! - Cooldown periods

use parking_lot::RwLock;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reason for circuit breaker trip
#[derive(Debug, Clone)]
pub enum TripReason {
    ManualHalt,
    MaxPositionPerMarket {
        market_id: String,
        current: i64,
        limit: i64,
    },
    MaxTotalPosition {
        current: i64,
        limit: i64,
    },
    MaxDailyLoss {
        current_cents: i64,
        limit_cents: i64,
    },
    ConsecutiveErrors {
        count: u32,
        limit: u32,
    },
}

impl TripReason {
    pub fn to_string(&self) -> String {
        match self {
            TripReason::ManualHalt => "Manual halt".to_string(),
            TripReason::MaxPositionPerMarket {
                market_id,
                current,
                limit,
            } => format!(
                "Max position per market exceeded: {} has {} contracts (limit: {})",
                market_id, current, limit
            ),
            TripReason::MaxTotalPosition { current, limit } => {
                format!(
                    "Max total position exceeded: {} contracts (limit: {})",
                    current, limit
                )
            }
            TripReason::MaxDailyLoss {
                current_cents,
                limit_cents,
            } => {
                format!(
                    "Max daily loss exceeded: ${:.2} (limit: ${:.2})",
                    *current_cents as f64 / 100.0,
                    *limit_cents as f64 / 100.0
                )
            }
            TripReason::ConsecutiveErrors { count, limit } => {
                format!(
                    "Consecutive errors exceeded: {} errors (limit: {})",
                    count, limit
                )
            }
        }
    }
}

/// Position tracking for a single market
#[derive(Debug, Clone, Default)]
pub struct MarketPosition {
    pub kalshi_contracts: i64,
    pub poly_contracts: i64,
}

impl MarketPosition {
    pub fn total(&self) -> i64 {
        self.kalshi_contracts.abs() + self.poly_contracts.abs()
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Max contracts per individual market
    pub max_position_per_market: i64,
    /// Max total contracts across all markets
    pub max_total_position: i64,
    /// Max daily loss in cents (negative = loss)
    pub max_daily_loss_cents: i64,
    /// Max consecutive errors before halting
    pub max_consecutive_errors: u32,
    /// Cooldown duration after trip before auto-reset
    pub cooldown_duration: Duration,
    /// Whether circuit breaker is enabled
    pub enabled: bool,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_position_per_market: 50_000, // 500 contracts (size in cents)
            max_total_position: 100_000,     // 1000 contracts total
            max_daily_loss_cents: 50_000,    // $500 max loss
            max_consecutive_errors: 5,
            cooldown_duration: Duration::from_secs(300), // 5 minutes
            enabled: true,
        }
    }
}

/// Circuit breaker for trading risk management
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    /// Trading is halted when true
    halted: AtomicBool,
    /// Consecutive error count (reset on success)
    consecutive_errors: AtomicI64,
    /// Daily P&L in cents (negative = loss)
    daily_pnl_cents: AtomicI64,
    /// Per-market positions
    positions: RwLock<HashMap<String, MarketPosition>>,
    /// When the circuit breaker was tripped
    tripped_at: RwLock<Option<Instant>>,
    /// Reason for most recent trip
    trip_reason: RwLock<Option<TripReason>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            halted: AtomicBool::new(false),
            consecutive_errors: AtomicI64::new(0),
            daily_pnl_cents: AtomicI64::new(0),
            positions: RwLock::new(HashMap::new()),
            tripped_at: RwLock::new(None),
            trip_reason: RwLock::new(None),
        }
    }

    /// Check if trading is currently allowed
    pub fn is_trading_allowed(&self) -> bool {
        if !self.config.enabled {
            return true;
        }

        // Check if halted
        if self.halted.load(Ordering::SeqCst) {
            // Check if cooldown has passed
            if let Some(tripped) = *self.tripped_at.read() {
                if tripped.elapsed() >= self.config.cooldown_duration {
                    // Auto-reset after cooldown
                    self.reset();
                    return true;
                }
            }
            return false;
        }

        true
    }

    /// Check if a specific execution is allowed
    /// Returns Ok(()) if allowed, Err(reason) if not
    pub fn can_execute(&self, market_id: &str, contracts: i64) -> Result<(), TripReason> {
        if !self.config.enabled {
            return Ok(());
        }

        if !self.is_trading_allowed() {
            if let Some(reason) = self.trip_reason.read().clone() {
                return Err(reason);
            }
            return Err(TripReason::ManualHalt);
        }

        // Check position limits
        let positions = self.positions.read();

        // Check per-market limit
        let current_position = positions.get(market_id).map(|p| p.total()).unwrap_or(0);

        if current_position + contracts.abs() > self.config.max_position_per_market {
            let reason = TripReason::MaxPositionPerMarket {
                market_id: market_id.to_string(),
                current: current_position,
                limit: self.config.max_position_per_market,
            };
            return Err(reason);
        }

        // Check total position limit
        let total_position: i64 = positions.values().map(|p| p.total()).sum();
        if total_position + contracts.abs() > self.config.max_total_position {
            let reason = TripReason::MaxTotalPosition {
                current: total_position,
                limit: self.config.max_total_position,
            };
            return Err(reason);
        }

        Ok(())
    }

    /// Record a successful execution
    pub fn record_success(
        &self,
        market_id: &str,
        kalshi_contracts: i64,
        poly_contracts: i64,
        pnl_cents: i64,
    ) {
        // Reset consecutive errors
        self.consecutive_errors.store(0, Ordering::SeqCst);

        // Update position
        let mut positions = self.positions.write();
        let pos = positions.entry(market_id.to_string()).or_default();
        pos.kalshi_contracts += kalshi_contracts;
        pos.poly_contracts += poly_contracts;

        // Record P&L
        self.daily_pnl_cents.fetch_add(pnl_cents, Ordering::SeqCst);
    }

    /// Record an error
    pub fn record_error(&self) {
        let errors = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;

        if errors as u32 >= self.config.max_consecutive_errors {
            self.trip(TripReason::ConsecutiveErrors {
                count: errors as u32,
                limit: self.config.max_consecutive_errors,
            });
        }
    }

    /// Record P&L change
    pub fn record_pnl(&self, pnl_cents: i64) {
        let new_pnl = self.daily_pnl_cents.fetch_add(pnl_cents, Ordering::SeqCst) + pnl_cents;

        // Check if we've exceeded max loss (new_pnl is negative for losses)
        if new_pnl < -self.config.max_daily_loss_cents {
            self.trip(TripReason::MaxDailyLoss {
                current_cents: new_pnl,
                limit_cents: self.config.max_daily_loss_cents,
            });
        }
    }

    /// Manually trip the circuit breaker
    pub fn trip(&self, reason: TripReason) {
        self.halted.store(true, Ordering::SeqCst);
        *self.tripped_at.write() = Some(Instant::now());
        *self.trip_reason.write() = Some(reason);
    }

    /// Manually halt trading
    pub fn halt(&self) {
        self.trip(TripReason::ManualHalt);
    }

    /// Reset the circuit breaker (clear halt status)
    pub fn reset(&self) {
        self.halted.store(false, Ordering::SeqCst);
        self.consecutive_errors.store(0, Ordering::SeqCst);
        *self.tripped_at.write() = None;
        *self.trip_reason.write() = None;
    }

    /// Reset daily P&L (call at start of trading day)
    pub fn reset_daily_pnl(&self) {
        self.daily_pnl_cents.store(0, Ordering::SeqCst);
    }

    /// Clear all positions
    pub fn clear_positions(&self) {
        self.positions.write().clear();
    }

    /// Get current status
    pub fn status(&self) -> CircuitBreakerStatus {
        let positions = self.positions.read();
        let total_position: i64 = positions.values().map(|p| p.total()).sum();

        CircuitBreakerStatus {
            enabled: self.config.enabled,
            halted: self.halted.load(Ordering::SeqCst),
            consecutive_errors: self.consecutive_errors.load(Ordering::SeqCst) as u32,
            daily_pnl_cents: self.daily_pnl_cents.load(Ordering::SeqCst),
            total_position,
            market_count: positions.len(),
            trip_reason: self.trip_reason.read().as_ref().map(|r| r.to_string()),
            cooldown_remaining_secs: self.tripped_at.read().map(|t| {
                let elapsed = t.elapsed();
                if elapsed < self.config.cooldown_duration {
                    (self.config.cooldown_duration - elapsed).as_secs()
                } else {
                    0
                }
            }),
        }
    }

    /// Get daily P&L in cents
    pub fn get_daily_pnl_cents(&self) -> i64 {
        self.daily_pnl_cents.load(Ordering::SeqCst)
    }

    /// Get position for a market
    pub fn get_position(&self, market_id: &str) -> Option<MarketPosition> {
        self.positions.read().get(market_id).cloned()
    }
}

/// Circuit breaker status for reporting
#[derive(Debug, Clone)]
pub struct CircuitBreakerStatus {
    pub enabled: bool,
    pub halted: bool,
    pub consecutive_errors: u32,
    pub daily_pnl_cents: i64,
    pub total_position: i64,
    pub market_count: usize,
    pub trip_reason: Option<String>,
    pub cooldown_remaining_secs: Option<u64>,
}

// ============================================================================
// PyO3 Bindings
// ============================================================================

/// Python wrapper for circuit breaker configuration
#[cfg_attr(feature = "python", pyclass(name = "CircuitBreakerConfig"))]
#[derive(Clone)]
pub struct PyCircuitBreakerConfig {
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub max_position_per_market: i64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub max_total_position: i64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub max_daily_loss: f64, // Dollars (converted to cents internally)
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub max_consecutive_errors: u32,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub cooldown_secs: u64,
    #[cfg_attr(feature = "python", pyo3(get, set))]
    pub enabled: bool,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyCircuitBreakerConfig {
    #[cfg_attr(feature = "python", new)]
    #[cfg_attr(feature = "python", pyo3(signature = (
        max_position_per_market = 50000,
        max_total_position = 100000,
        max_daily_loss = 500.0,
        max_consecutive_errors = 5,
        cooldown_secs = 300,
        enabled = true
    )))]
    fn new(
        max_position_per_market: i64,
        max_total_position: i64,
        max_daily_loss: f64,
        max_consecutive_errors: u32,
        cooldown_secs: u64,
        enabled: bool,
    ) -> Self {
        Self {
            max_position_per_market,
            max_total_position,
            max_daily_loss,
            max_consecutive_errors,
            cooldown_secs,
            enabled,
        }
    }
}

impl From<PyCircuitBreakerConfig> for CircuitBreakerConfig {
    fn from(py_config: PyCircuitBreakerConfig) -> Self {
        Self {
            max_position_per_market: py_config.max_position_per_market,
            max_total_position: py_config.max_total_position,
            max_daily_loss_cents: (py_config.max_daily_loss * 100.0) as i64,
            max_consecutive_errors: py_config.max_consecutive_errors,
            cooldown_duration: Duration::from_secs(py_config.cooldown_secs),
            enabled: py_config.enabled,
        }
    }
}

/// Python wrapper for circuit breaker
#[cfg_attr(feature = "python", pyclass(name = "CircuitBreaker"))]
pub struct PyCircuitBreaker {
    inner: Arc<CircuitBreaker>,
}

#[cfg_attr(feature = "python", pymethods)]
impl PyCircuitBreaker {
    #[cfg_attr(feature = "python", new)]
    fn new(config: PyCircuitBreakerConfig) -> Self {
        Self {
            inner: Arc::new(CircuitBreaker::new(config.into())),
        }
    }

    /// Check if trading is currently allowed
    fn is_trading_allowed(&self) -> bool {
        self.inner.is_trading_allowed()
    }

    /// Check if a specific execution is allowed
    /// Raises RuntimeError with reason if not allowed
    #[cfg(feature = "python")]
    fn can_execute(&self, market_id: &str, contracts: i64) -> PyResult<()> {
        self.inner
            .can_execute(market_id, contracts)
            .map_err(|reason| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(reason.to_string()))
    }

    /// Record a successful execution
    fn record_success(
        &self,
        market_id: &str,
        kalshi_contracts: i64,
        poly_contracts: i64,
        pnl: f64,
    ) {
        self.inner.record_success(
            market_id,
            kalshi_contracts,
            poly_contracts,
            (pnl * 100.0) as i64,
        );
    }

    /// Record an error
    fn record_error(&self) {
        self.inner.record_error();
    }

    /// Record P&L change (in dollars)
    fn record_pnl(&self, pnl: f64) {
        self.inner.record_pnl((pnl * 100.0) as i64);
    }

    /// Manually trip with a reason
    fn trip(&self, _reason: &str) {
        self.inner.trip(TripReason::ManualHalt);
    }

    /// Manually halt trading
    fn halt(&self) {
        self.inner.halt();
    }

    /// Reset the circuit breaker
    fn reset(&self) {
        self.inner.reset();
    }

    /// Reset daily P&L
    fn reset_daily_pnl(&self) {
        self.inner.reset_daily_pnl();
    }

    /// Clear all positions
    fn clear_positions(&self) {
        self.inner.clear_positions();
    }

    /// Get current status as a dict
    #[cfg(feature = "python")]
    fn status(&self, py: Python) -> PyObject {
        let status = self.inner.status();
        let dict = PyDict::new(py);
        dict.set_item("enabled", status.enabled).unwrap();
        dict.set_item("halted", status.halted).unwrap();
        dict.set_item("consecutive_errors", status.consecutive_errors)
            .unwrap();
        dict.set_item("daily_pnl", status.daily_pnl_cents as f64 / 100.0)
            .unwrap();
        dict.set_item("total_position", status.total_position)
            .unwrap();
        dict.set_item("market_count", status.market_count).unwrap();
        dict.set_item("trip_reason", status.trip_reason).unwrap();
        dict.set_item("cooldown_remaining_secs", status.cooldown_remaining_secs)
            .unwrap();
        dict.into()
    }

    /// Get daily P&L in dollars
    fn daily_pnl(&self) -> f64 {
        self.inner.get_daily_pnl_cents() as f64 / 100.0
    }
}

// ============================================================================
// API Circuit Breaker for External Service Resilience
// ============================================================================

/// States for the API circuit breaker
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiCircuitState {
    /// Normal operation - requests are allowed
    Closed,
    /// Circuit is open - requests are blocked
    Open,
    /// Testing if service is recovered
    HalfOpen,
}

/// Configuration for API circuit breaker
#[derive(Debug, Clone)]
pub struct ApiCircuitBreakerConfig {
    /// Number of consecutive failures to trip the circuit
    pub failure_threshold: u32,
    /// Duration to wait before attempting recovery
    pub recovery_timeout: Duration,
    /// Number of successful calls in half-open state to close circuit
    pub success_threshold: u32,
}

impl Default for ApiCircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            success_threshold: 2,
        }
    }
}

/// Lightweight circuit breaker for external API calls.
///
/// Unlike the trading CircuitBreaker, this is designed for API resilience:
/// - Tracks consecutive failures
/// - Opens circuit after threshold failures
/// - Auto-transitions to half-open after timeout
/// - Closes after successful calls in half-open state
///
/// # Example
/// ```ignore
/// let breaker = ApiCircuitBreaker::new("espn", ApiCircuitBreakerConfig::default());
///
/// // Before making API call
/// if !breaker.is_available() {
///     return Err(anyhow!("ESPN API circuit breaker open"));
/// }
///
/// // After API call
/// match api_call().await {
///     Ok(result) => {
///         breaker.record_success();
///         Ok(result)
///     }
///     Err(e) => {
///         breaker.record_failure();
///         Err(e)
///     }
/// }
/// ```
pub struct ApiCircuitBreaker {
    name: String,
    config: ApiCircuitBreakerConfig,
    state: RwLock<ApiCircuitState>,
    failure_count: std::sync::atomic::AtomicU32,
    success_count: std::sync::atomic::AtomicU32,
    last_failure_time: RwLock<Option<Instant>>,
}

impl ApiCircuitBreaker {
    /// Create a new API circuit breaker
    pub fn new(name: &str, config: ApiCircuitBreakerConfig) -> Self {
        Self {
            name: name.to_string(),
            config,
            state: RwLock::new(ApiCircuitState::Closed),
            failure_count: std::sync::atomic::AtomicU32::new(0),
            success_count: std::sync::atomic::AtomicU32::new(0),
            last_failure_time: RwLock::new(None),
        }
    }

    /// Create with default configuration
    pub fn with_defaults(name: &str) -> Self {
        Self::new(name, ApiCircuitBreakerConfig::default())
    }

    /// Check if the circuit breaker allows requests
    pub fn is_available(&self) -> bool {
        let mut state = self.state.write();

        match *state {
            ApiCircuitState::Closed => true,
            ApiCircuitState::Open => {
                // Check if recovery timeout has passed
                let should_try = self
                    .last_failure_time
                    .read()
                    .map(|t| t.elapsed() >= self.config.recovery_timeout)
                    .unwrap_or(true);

                if should_try {
                    // Transition to half-open
                    *state = ApiCircuitState::HalfOpen;
                    self.success_count.store(0, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }
            ApiCircuitState::HalfOpen => true,
        }
    }

    /// Record a successful API call
    pub fn record_success(&self) {
        self.failure_count.store(0, Ordering::SeqCst);

        let mut state = self.state.write();
        match *state {
            ApiCircuitState::HalfOpen => {
                let successes = self.success_count.fetch_add(1, Ordering::SeqCst) + 1;
                if successes >= self.config.success_threshold {
                    *state = ApiCircuitState::Closed;
                    tracing::info!(
                        "API circuit breaker '{}' closed after {} successful calls",
                        self.name,
                        successes
                    );
                }
            }
            _ => {
                // Ensure we're in closed state on success
                *state = ApiCircuitState::Closed;
            }
        }
    }

    /// Record a failed API call
    pub fn record_failure(&self) {
        let failures = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_failure_time.write() = Some(Instant::now());

        let mut state = self.state.write();
        match *state {
            ApiCircuitState::Closed => {
                if failures >= self.config.failure_threshold {
                    *state = ApiCircuitState::Open;
                    tracing::warn!(
                        "API circuit breaker '{}' OPENED after {} consecutive failures",
                        self.name,
                        failures
                    );
                }
            }
            ApiCircuitState::HalfOpen => {
                // Any failure in half-open goes back to open
                *state = ApiCircuitState::Open;
                tracing::warn!(
                    "API circuit breaker '{}' re-OPENED during half-open test",
                    self.name
                );
            }
            ApiCircuitState::Open => {
                // Already open, nothing to do
            }
        }
    }

    /// Get current state
    pub fn state(&self) -> ApiCircuitState {
        *self.state.read()
    }

    /// Get the name of this circuit breaker
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get current failure count
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::SeqCst)
    }

    /// Reset the circuit breaker to closed state
    pub fn reset(&self) {
        let mut state = self.state.write();
        *state = ApiCircuitState::Closed;
        self.failure_count.store(0, Ordering::SeqCst);
        self.success_count.store(0, Ordering::SeqCst);
        *self.last_failure_time.write() = None;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_circuit_breaker_basic() {
        let cb = ApiCircuitBreaker::new(
            "test_api",
            ApiCircuitBreakerConfig {
                failure_threshold: 3,
                recovery_timeout: Duration::from_millis(100),
                success_threshold: 2,
            },
        );

        // Initially available
        assert!(cb.is_available());
        assert_eq!(cb.state(), ApiCircuitState::Closed);

        // Record failures
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_available()); // Still available (2 < 3)

        cb.record_failure();
        assert_eq!(cb.state(), ApiCircuitState::Open);
        assert!(!cb.is_available()); // Now blocked
    }

    #[test]
    fn test_api_circuit_breaker_recovery() {
        let cb = ApiCircuitBreaker::new(
            "test_api",
            ApiCircuitBreakerConfig {
                failure_threshold: 2,
                recovery_timeout: Duration::from_millis(10),
                success_threshold: 2,
            },
        );

        // Trip the breaker
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), ApiCircuitState::Open);

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(20));

        // Should transition to half-open
        assert!(cb.is_available());
        assert_eq!(cb.state(), ApiCircuitState::HalfOpen);

        // Success in half-open
        cb.record_success();
        assert_eq!(cb.state(), ApiCircuitState::HalfOpen); // Need 2 successes

        cb.record_success();
        assert_eq!(cb.state(), ApiCircuitState::Closed); // Now closed
    }

    #[test]
    fn test_api_circuit_breaker_half_open_failure() {
        let cb = ApiCircuitBreaker::new(
            "test_api",
            ApiCircuitBreakerConfig {
                failure_threshold: 1,
                recovery_timeout: Duration::from_millis(10),
                success_threshold: 1,
            },
        );

        // Trip the breaker
        cb.record_failure();
        assert_eq!(cb.state(), ApiCircuitState::Open);

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.is_available()); // Transitions to half-open

        // Failure in half-open should re-open
        cb.record_failure();
        assert_eq!(cb.state(), ApiCircuitState::Open);
    }

    #[test]
    fn test_max_daily_loss_trips() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_daily_loss_cents: 50000, // $500
            ..Default::default()
        });

        // Record some losses
        cb.record_pnl(-40000); // -$400
        assert!(cb.is_trading_allowed());

        cb.record_pnl(-15000); // -$150 more = -$550 total
        assert!(!cb.is_trading_allowed());

        // Check status
        let status = cb.status();
        assert!(status.halted);
        assert!(status.trip_reason.is_some());
    }

    #[test]
    fn test_consecutive_errors_trip() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_consecutive_errors: 3,
            ..Default::default()
        });

        cb.record_error();
        cb.record_error();
        assert!(cb.is_trading_allowed());

        cb.record_error(); // Third error
        assert!(!cb.is_trading_allowed());

        let status = cb.status();
        assert!(status.halted);
        assert_eq!(status.consecutive_errors, 3);
    }

    #[test]
    fn test_success_resets_error_counter() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_consecutive_errors: 3,
            ..Default::default()
        });

        cb.record_error();
        cb.record_error();
        cb.record_success("market1", 10, 10, 100); // Resets counter
        cb.record_error();
        cb.record_error();
        assert!(cb.is_trading_allowed()); // Still allowed (only 2 consecutive)

        let status = cb.status();
        assert_eq!(status.consecutive_errors, 2);
    }

    #[test]
    fn test_position_limit_per_market() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_position_per_market: 1000,
            ..Default::default()
        });

        // First execution should be allowed
        assert!(cb.can_execute("market1", 500).is_ok());
        cb.record_success("market1", 500, 0, 0);

        // Second execution exceeds limit
        let result = cb.can_execute("market1", 600);
        assert!(result.is_err());
    }

    #[test]
    fn test_total_position_limit() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            max_position_per_market: 1000,
            max_total_position: 1500,
            ..Default::default()
        });

        cb.record_success("market1", 500, 0, 0);
        cb.record_success("market2", 500, 0, 0);
        cb.record_success("market3", 400, 0, 0);

        // Next execution would exceed total limit
        let result = cb.can_execute("market4", 200);
        assert!(result.is_err());
    }

    #[test]
    fn test_manual_halt_and_reset() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());

        assert!(cb.is_trading_allowed());

        cb.halt();
        assert!(!cb.is_trading_allowed());

        cb.reset();
        assert!(cb.is_trading_allowed());
    }

    #[test]
    fn test_disabled_circuit_breaker() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            enabled: false,
            max_daily_loss_cents: 100, // Very low limit
            ..Default::default()
        });

        // Should always be allowed when disabled
        cb.record_pnl(-500); // -$5 exceeds $1 limit
        assert!(cb.is_trading_allowed());
    }

    #[test]
    fn test_pnl_tracking() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());

        cb.record_pnl(1000); // +$10
        cb.record_pnl(-500); // -$5
        cb.record_pnl(200); // +$2

        assert_eq!(cb.get_daily_pnl_cents(), 700); // +$7

        cb.reset_daily_pnl();
        assert_eq!(cb.get_daily_pnl_cents(), 0);
    }
}
