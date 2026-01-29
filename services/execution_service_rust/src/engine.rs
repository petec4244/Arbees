//! Execution engine with comprehensive safeguards
//!
//! Handles order execution with rate limiting, kill switch, idempotency,
//! balance validation, price sanity checks, and audit logging.

use anyhow::Result;
use arbees_rust_core::atomic_orderbook::kalshi_fee_cents;
use arbees_rust_core::clients::kalshi::KalshiClient;
use arbees_rust_core::clients::polymarket::PolymarketClient;
use arbees_rust_core::models::{ExecutionRequest, ExecutionResult, ExecutionSide, ExecutionStatus, Platform};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::Utc;
use log::{error, info, warn};
use std::sync::Arc;

use crate::audit::{AuditEventType, AuditLogger};
use crate::balance::BalanceCache;
use crate::config::SafeguardConfig;
use crate::idempotency::{IdempotencyResult, IdempotencyTracker};
use crate::kill_switch::KillSwitch;
use crate::polymarket_executor::PolymarketExecutor;
use crate::rate_limiter::RateLimiter;

/// Calculate trading fee for a given platform, price, and size.
///
/// For Kalshi and Paper trading:
/// - Uses Kalshi's fee schedule: fee = ceil(7 * price * (100 - price) / 10000) cents per contract
/// - Fee is per contract, so total fee = fee_per_contract * quantity
///
/// For Polymarket:
/// - Uses 2% fee on the position value
fn calculate_fee(platform: Platform, price: f64, size: f64) -> f64 {
    match platform {
        Platform::Kalshi | Platform::Paper => {
            // Convert price (0.0-1.0) to cents (0-100)
            let price_cents = (price * 100.0).round() as u16;
            // Get fee in cents per contract
            let fee_cents = kalshi_fee_cents(price_cents);
            // Convert back to dollars and multiply by quantity
            // Size represents number of contracts, each contract has face value of $1
            (fee_cents as f64 / 100.0) * size
        }
        Platform::Polymarket => {
            // Polymarket charges 2% fee on position value
            price * size * 0.02
        }
    }
}

/// Rejection reason for safeguard failures
#[derive(Debug, Clone)]
pub enum RejectionReason {
    KillSwitchActive,
    RateLimitExceeded(String),
    DuplicateRequest,
    InsufficientBalance(String),
    OrderSizeExceeded(String),
    PriceSanityFailed(String),
    CredentialsNotConfigured(String),
}

impl std::fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KillSwitchActive => write!(f, "Kill switch is active - trading halted"),
            Self::RateLimitExceeded(msg) => write!(f, "Rate limit exceeded: {}", msg),
            Self::DuplicateRequest => write!(f, "Duplicate request - already processed"),
            Self::InsufficientBalance(msg) => write!(f, "Insufficient balance: {}", msg),
            Self::OrderSizeExceeded(msg) => write!(f, "Order size limit exceeded: {}", msg),
            Self::PriceSanityFailed(msg) => write!(f, "Price sanity check failed: {}", msg),
            Self::CredentialsNotConfigured(msg) => write!(f, "{}", msg),
        }
    }
}

/// Execution engine with all safeguards
pub struct ExecutionEngine {
    kalshi: Arc<KalshiClient>,
    polymarket: PolymarketClient,
    polymarket_executor: Option<PolymarketExecutor>,
    paper_trading: bool,

    // Safeguards
    config: SafeguardConfig,
    rate_limiter: RateLimiter,
    kill_switch: Arc<KillSwitch>,
    idempotency: Arc<IdempotencyTracker>,
    balance_cache: Arc<BalanceCache>,
    audit_logger: AuditLogger,
}

impl ExecutionEngine {
    /// Create a new execution engine
    ///
    /// If paper_trading is true, all orders will be simulated.
    /// Otherwise, the engine will attempt live trading on supported platforms.
    ///
    /// For Kalshi live trading, set environment variables:
    /// - KALSHI_API_KEY: Your API key
    /// - KALSHI_PRIVATE_KEY or KALSHI_PRIVATE_KEY_PATH: RSA private key
    /// - KALSHI_ENV: "prod" or "demo" (defaults to prod)
    ///
    /// For Polymarket live trading, set environment variables:
    /// - POLYMARKET_PRIVATE_KEY: L1 wallet private key (hex, with 0x prefix)
    /// - POLYMARKET_FUNDER_ADDRESS: Funder/proxy wallet address
    /// - POLYMARKET_CHAIN_ID: 137 for mainnet, 80002 for testnet (default: 137)
    /// - POLYMARKET_CLOB_HOST: CLOB API host (default: https://clob.polymarket.com)
    /// - POLYMARKET_API_NONCE: Nonce for API key derivation (default: 0)
    pub async fn new(paper_trading: bool) -> Self {
        Self::with_safeguards(paper_trading, None, None, None).await
    }

    /// Create a new execution engine with explicit safeguard components
    pub async fn with_safeguards(
        paper_trading: bool,
        redis: Option<Arc<RedisBus>>,
        kill_switch: Option<Arc<KillSwitch>>,
        idempotency: Option<Arc<IdempotencyTracker>>,
    ) -> Self {
        let config = SafeguardConfig::from_env();
        config.log_config();

        // Try to load Kalshi credentials from environment
        let kalshi = Arc::new(match KalshiClient::from_env() {
            Ok(client) => {
                if client.has_credentials() {
                    info!("Kalshi client initialized with trading credentials");
                } else {
                    warn!("Kalshi client initialized without credentials (will reject live trades)");
                }
                client
            }
            Err(e) => {
                warn!(
                    "Failed to initialize Kalshi client from env: {}. Using read-only client.",
                    e
                );
                KalshiClient::new().expect("Failed to create Kalshi client")
            }
        });

        // Try to initialize Polymarket CLOB executor (only for live trading)
        let polymarket_executor = if !paper_trading {
            match PolymarketExecutor::from_env().await {
                Ok(exec) => {
                    info!("Polymarket CLOB executor initialized");
                    Some(exec)
                }
                Err(e) => {
                    warn!("Polymarket CLOB executor not available: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Initialize safeguards
        let rate_limiter = RateLimiter::new(config.max_orders_per_minute, config.max_orders_per_hour);
        let kill_switch = kill_switch.unwrap_or_else(|| Arc::new(KillSwitch::new()));
        let idempotency = idempotency.unwrap_or_else(|| Arc::new(IdempotencyTracker::new()));
        let balance_cache = Arc::new(BalanceCache::new(config.balance_refresh_secs));
        let audit_logger = AuditLogger::new(redis, config.audit_log_enabled);

        Self {
            kalshi,
            polymarket: PolymarketClient::new(),
            polymarket_executor,
            paper_trading,
            config,
            rate_limiter,
            kill_switch,
            idempotency,
            balance_cache,
            audit_logger,
        }
    }

    /// Get the kill switch (for external control)
    pub fn kill_switch(&self) -> Arc<KillSwitch> {
        Arc::clone(&self.kill_switch)
    }

    /// Get the idempotency tracker (for cleanup task)
    pub fn idempotency_tracker(&self) -> Arc<IdempotencyTracker> {
        Arc::clone(&self.idempotency)
    }

    /// Get the balance cache (for refresh task)
    pub fn balance_cache(&self) -> Arc<BalanceCache> {
        Arc::clone(&self.balance_cache)
    }

    /// Get the Kalshi client (for balance refresh)
    pub fn kalshi_client(&self) -> Arc<KalshiClient> {
        Arc::clone(&self.kalshi)
    }

    /// Get the config
    pub fn config(&self) -> &SafeguardConfig {
        &self.config
    }

    /// Check if Kalshi live trading is available
    pub fn kalshi_live_enabled(&self) -> bool {
        !self.paper_trading && self.kalshi.has_credentials()
    }

    /// Check if Polymarket live trading is available
    pub fn polymarket_live_enabled(&self) -> bool {
        !self.paper_trading && self.polymarket_executor.is_some()
    }

    /// Execute a request with all safeguard checks
    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        let start_time = Utc::now();

        // Log request received
        self.audit_logger.log_execution_requested(&request).await;

        // ============================================================
        // SAFEGUARD 1: Kill Switch Check
        // ============================================================
        if self.kill_switch.is_enabled() {
            warn!("Order rejected: Kill switch is active");
            self.audit_logger
                .log_rejection(AuditEventType::OrderRejected, &request, "Kill switch active")
                .await;
            return Ok(self.create_rejection(&request, start_time, RejectionReason::KillSwitchActive));
        }

        // ============================================================
        // SAFEGUARD 2: Rate Limiting
        // ============================================================
        if let Err(rate_err) = self.rate_limiter.check_and_record() {
            warn!("Order rejected: {}", rate_err);
            self.audit_logger
                .log_rejection(
                    AuditEventType::RateLimitExceeded,
                    &request,
                    &rate_err.to_string(),
                )
                .await;
            return Ok(self.create_rejection(
                &request,
                start_time,
                RejectionReason::RateLimitExceeded(rate_err.to_string()),
            ));
        }

        // ============================================================
        // SAFEGUARD 3: Idempotency Check
        // ============================================================
        match self.idempotency.check_and_record(&request.idempotency_key, &request.request_id) {
            IdempotencyResult::Duplicate { original_timestamp } => {
                warn!(
                    "Duplicate request detected: {} (original: {})",
                    request.idempotency_key, original_timestamp
                );
                self.audit_logger
                    .log_rejection(AuditEventType::DuplicateDetected, &request, "Duplicate idempotency key")
                    .await;
                return Ok(self.create_rejection(&request, start_time, RejectionReason::DuplicateRequest));
            }
            IdempotencyResult::New => {}
        }

        // ============================================================
        // SAFEGUARD 4: Order Size Limits
        // ============================================================
        let order_value = request.limit_price * request.size;
        let quantity = request.size.round() as i32;

        if order_value > self.config.max_order_size {
            let reason = format!(
                "Order value ${:.2} exceeds max ${:.2}",
                order_value, self.config.max_order_size
            );
            warn!("Order rejected: {}", reason);
            self.audit_logger
                .log_rejection(AuditEventType::OrderRejected, &request, &reason)
                .await;
            return Ok(self.create_rejection(
                &request,
                start_time,
                RejectionReason::OrderSizeExceeded(reason),
            ));
        }

        if quantity > self.config.max_order_contracts {
            let reason = format!(
                "Contract count {} exceeds max {}",
                quantity, self.config.max_order_contracts
            );
            warn!("Order rejected: {}", reason);
            self.audit_logger
                .log_rejection(AuditEventType::OrderRejected, &request, &reason)
                .await;
            return Ok(self.create_rejection(
                &request,
                start_time,
                RejectionReason::OrderSizeExceeded(reason),
            ));
        }

        // ============================================================
        // SAFEGUARD 5: Price Sanity Check
        // ============================================================
        if request.limit_price < self.config.min_safe_price
            || request.limit_price > self.config.max_safe_price
        {
            let reason = format!(
                "Price {:.3} outside safe range [{:.2}, {:.2}]",
                request.limit_price, self.config.min_safe_price, self.config.max_safe_price
            );
            warn!("Order rejected: {}", reason);
            self.audit_logger
                .log_rejection(AuditEventType::PriceSanityFailed, &request, &reason)
                .await;
            return Ok(self.create_rejection(
                &request,
                start_time,
                RejectionReason::PriceSanityFailed(reason),
            ));
        }

        // ============================================================
        // SAFEGUARD 6: Balance Validation (live trading only)
        // ============================================================
        if !self.paper_trading && request.platform != Platform::Paper {
            if let Err(balance_err) = self.balance_cache.validate_order(&request.platform, order_value).await {
                warn!("Order rejected: {}", balance_err);
                self.audit_logger
                    .log_rejection(AuditEventType::InsufficientBalance, &request, &balance_err)
                    .await;
                return Ok(self.create_rejection(
                    &request,
                    start_time,
                    RejectionReason::InsufficientBalance(balance_err),
                ));
            }
        }

        // ============================================================
        // EXECUTE ORDER
        // ============================================================
        info!(
            "Executing order: {:?} {} x{} @ {:.3} on {:?}",
            request.side, request.market_id, request.size, request.limit_price, request.platform
        );

        let result = if self.paper_trading {
            self.execute_paper(&request, start_time).await
        } else {
            match request.platform {
                Platform::Kalshi => self.execute_kalshi(&request, start_time).await,
                Platform::Polymarket => self.execute_polymarket(request.clone()).await,
                Platform::Paper => self.execute_paper(&request, start_time).await,
            }
        };

        // Log result
        if let Ok(ref res) = result {
            self.audit_logger.log_execution_result(res).await;

            // Mark balance as stale after order (live trading only)
            if !self.paper_trading && res.status == ExecutionStatus::Filled {
                match res.platform {
                    Platform::Kalshi => self.balance_cache.mark_kalshi_stale().await,
                    Platform::Polymarket => self.balance_cache.mark_polymarket_stale().await,
                    Platform::Paper => {}
                }
            }
        }

        result
    }

    /// Create a rejection result
    fn create_rejection(
        &self,
        request: &ExecutionRequest,
        start_time: chrono::DateTime<Utc>,
        reason: RejectionReason,
    ) -> ExecutionResult {
        let executed_at = Utc::now();
        let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

        ExecutionResult {
            request_id: request.request_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(reason.to_string()),
            order_id: None,
            filled_qty: 0.0,
            avg_price: 0.0,
            fees: 0.0,
            platform: request.platform,
            market_id: request.market_id.clone(),
            contract_team: request.contract_team.clone(),
            game_id: request.game_id.clone(),
            sport: request.sport,
            signal_id: request.signal_id.clone(),
            signal_type: request.signal_type.clone(),
            edge_pct: request.edge_pct,
            side: request.side,
            requested_at: request.created_at,
            executed_at,
            latency_ms,
        }
    }

    /// Execute a paper trade (simulation)
    async fn execute_paper(
        &self,
        request: &ExecutionRequest,
        start_time: chrono::DateTime<Utc>,
    ) -> Result<ExecutionResult> {
        info!("Paper trade simulation for {}", request.request_id);
        let executed_at = Utc::now();
        let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

        // Calculate realistic fees based on the platform
        let fees = calculate_fee(request.platform, request.limit_price, request.size);
        info!(
            "Paper trade fees: ${:.4} (platform={:?}, price={:.3}, size={:.2})",
            fees, request.platform, request.limit_price, request.size
        );

        Ok(ExecutionResult {
            request_id: request.request_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            status: ExecutionStatus::Filled,
            rejection_reason: None,
            order_id: Some(format!("paper-{}", uuid::Uuid::new_v4())),
            filled_qty: request.size,
            avg_price: request.limit_price,
            fees,
            platform: request.platform,
            market_id: request.market_id.clone(),
            contract_team: request.contract_team.clone(),
            game_id: request.game_id.clone(),
            sport: request.sport,
            signal_id: request.signal_id.clone(),
            signal_type: request.signal_type.clone(),
            edge_pct: request.edge_pct,
            side: request.side,
            requested_at: request.created_at,
            executed_at,
            latency_ms,
        })
    }

    /// Execute a Kalshi order using the live API
    async fn execute_kalshi(
        &self,
        request: &ExecutionRequest,
        start_time: chrono::DateTime<Utc>,
    ) -> Result<ExecutionResult> {
        // Check if we have credentials
        if !self.kalshi.has_credentials() {
            return Ok(self.create_rejection(
                request,
                start_time,
                RejectionReason::CredentialsNotConfigured(
                    "Kalshi credentials not configured. Set KALSHI_API_KEY and KALSHI_PRIVATE_KEY"
                        .to_string(),
                ),
            ));
        }

        // Map ExecutionSide to Kalshi side string
        let side_str = match request.side {
            ExecutionSide::Yes => "yes",
            ExecutionSide::No => "no",
        };

        // Convert size to contracts (assuming size is in dollars and contracts are $1 each)
        let quantity = request.size.round() as i32;
        if quantity < 1 {
            let reason = format!("Order size too small: {} contracts (minimum 1)", quantity);
            return Ok(self.create_rejection(
                request,
                start_time,
                RejectionReason::OrderSizeExceeded(reason),
            ));
        }

        info!(
            "Placing Kalshi IOC order: {} {} x{} @ {:.2} on {}",
            "buy", side_str, quantity, request.limit_price, request.market_id
        );

        // Place IOC (Immediate-or-Cancel) order
        // IOC orders fill immediately or cancel - they never rest on the book
        // This eliminates one-sided fill risk in arbitrage trading
        match self
            .kalshi
            .place_ioc_order(&request.market_id, side_str, request.limit_price, quantity)
            .await
        {
            Ok(order) => {
                let executed_at = Utc::now();
                let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

                // For IOC orders, determine fill status
                let filled_qty = order.filled_count() as f64;
                let status = if order.is_filled() {
                    ExecutionStatus::Filled
                } else if order.is_partial() {
                    // IOC partial fills are rare but possible
                    warn!(
                        "IOC order {} partially filled: {}/{}",
                        order.order_id, filled_qty, quantity
                    );
                    ExecutionStatus::Partial
                } else {
                    // IOC order didn't fill at all - this is normal (no liquidity)
                    info!("IOC order {} did not fill (no liquidity)", order.order_id);
                    ExecutionStatus::Cancelled
                };

                // Calculate fees based on filled quantity
                let fees = calculate_fee(Platform::Kalshi, request.limit_price, filled_qty);

                info!(
                    "Kalshi IOC order {} status: {:?}, filled: {}/{}, fees: ${:.4}",
                    order.order_id, status, filled_qty, quantity, fees
                );

                Ok(ExecutionResult {
                    request_id: request.request_id.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    status,
                    rejection_reason: None,
                    order_id: Some(order.order_id),
                    filled_qty,
                    avg_price: request.limit_price, // TODO: Get actual fill price from order
                    fees,
                    platform: Platform::Kalshi,
                    market_id: request.market_id.clone(),
                    contract_team: request.contract_team.clone(),
                    game_id: request.game_id.clone(),
                    sport: request.sport,
                    signal_id: request.signal_id.clone(),
                    signal_type: request.signal_type.clone(),
                    edge_pct: request.edge_pct,
                    side: request.side,
                    requested_at: request.created_at,
                    executed_at,
                    latency_ms,
                })
            }
            Err(e) => {
                let executed_at = Utc::now();
                let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

                error!("Kalshi order failed: {}", e);

                Ok(ExecutionResult {
                    request_id: request.request_id.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    status: ExecutionStatus::Failed,
                    rejection_reason: Some(format!("Kalshi API error: {}", e)),
                    order_id: None,
                    filled_qty: 0.0,
                    avg_price: 0.0,
                    fees: 0.0,
                    platform: Platform::Kalshi,
                    market_id: request.market_id.clone(),
                    contract_team: request.contract_team.clone(),
                    game_id: request.game_id.clone(),
                    sport: request.sport,
                    signal_id: request.signal_id.clone(),
                    signal_type: request.signal_type.clone(),
                    edge_pct: request.edge_pct,
                    side: request.side,
                    requested_at: request.created_at,
                    executed_at,
                    latency_ms,
                })
            }
        }
    }

    /// Execute a Polymarket order
    async fn execute_polymarket(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        if let Some(executor) = &self.polymarket_executor {
            executor.execute(request).await
        } else {
            let executed_at = Utc::now();
            let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
            Ok(ExecutionResult {
                request_id: request.request_id,
                idempotency_key: request.idempotency_key,
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(
                    "Polymarket CLOB not configured. Set POLYMARKET_PRIVATE_KEY and POLYMARKET_FUNDER_ADDRESS"
                        .to_string(),
                ),
                order_id: None,
                filled_qty: 0.0,
                avg_price: 0.0,
                fees: 0.0,
                platform: Platform::Polymarket,
                market_id: request.market_id,
                contract_team: request.contract_team,
                game_id: request.game_id,
                sport: request.sport,
                signal_id: request.signal_id,
                signal_type: request.signal_type,
                edge_pct: request.edge_pct,
                side: request.side,
                requested_at: request.created_at,
                executed_at,
                latency_ms,
            })
        }
    }
}
