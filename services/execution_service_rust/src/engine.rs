use anyhow::Result;
use arbees_rust_core::atomic_orderbook::kalshi_fee_cents;
use arbees_rust_core::clients::{kalshi::KalshiClient, polymarket::PolymarketClient};
use arbees_rust_core::models::{ExecutionRequest, ExecutionResult, ExecutionSide, ExecutionStatus, Platform};
use chrono::Utc;
use log::{error, info, warn};

use crate::polymarket_executor::PolymarketExecutor;

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

pub struct ExecutionEngine {
    kalshi: KalshiClient,
    polymarket: PolymarketClient,
    polymarket_executor: Option<PolymarketExecutor>,
    paper_trading: bool,
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
        // Try to load Kalshi credentials from environment
        let kalshi = match KalshiClient::from_env() {
            Ok(client) => {
                if client.has_credentials() {
                    info!("Kalshi client initialized with trading credentials");
                } else {
                    warn!("Kalshi client initialized without credentials (will reject live trades)");
                }
                client
            }
            Err(e) => {
                warn!("Failed to initialize Kalshi client from env: {}. Using read-only client.", e);
                KalshiClient::new().expect("Failed to create Kalshi client")
            }
        };

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

        Self {
            kalshi,
            polymarket: PolymarketClient::new(),
            polymarket_executor,
            paper_trading,
        }
    }

    /// Check if Kalshi live trading is available
    pub fn kalshi_live_enabled(&self) -> bool {
        !self.paper_trading && self.kalshi.has_credentials()
    }

    /// Check if Polymarket live trading is available
    pub fn polymarket_live_enabled(&self) -> bool {
        !self.paper_trading && self.polymarket_executor.is_some()
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        info!("Executing request: {:?}", request);

        if self.paper_trading {
            // Paper trading logic with realistic fee calculation
            info!("Paper trade simulation for {}", request.request_id);
            let executed_at = Utc::now();
            let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;

            // Calculate realistic fees based on the platform
            let fees = calculate_fee(request.platform, request.limit_price, request.size);
            info!(
                "Paper trade fees: ${:.4} (platform={:?}, price={:.3}, size={:.2})",
                fees, request.platform, request.limit_price, request.size
            );

            return Ok(ExecutionResult {
                request_id: request.request_id,
                idempotency_key: request.idempotency_key,
                status: ExecutionStatus::Filled,
                rejection_reason: None,
                order_id: Some(format!("paper-{}", uuid::Uuid::new_v4())),
                filled_qty: request.size,
                avg_price: request.limit_price,
                fees,
                platform: request.platform,
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
            });
        }

        // Real execution
        match request.platform {
            Platform::Kalshi => {
                self.execute_kalshi(request).await
            }
            Platform::Polymarket => {
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
            Platform::Paper => {
                let executed_at = Utc::now();
                let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
                // Calculate realistic fees using Kalshi fee schedule for Paper trades
                let fees = calculate_fee(Platform::Paper, request.limit_price, request.size);
                Ok(ExecutionResult {
                    request_id: request.request_id,
                    idempotency_key: request.idempotency_key,
                    status: ExecutionStatus::Filled,
                    rejection_reason: None,
                    order_id: Some(format!("paper-{}", uuid::Uuid::new_v4())),
                    filled_qty: request.size,
                    avg_price: request.limit_price,
                    fees,
                    platform: Platform::Paper,
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

    /// Execute a Kalshi order using the live API
    async fn execute_kalshi(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        let start_time = Utc::now();

        // Check if we have credentials
        if !self.kalshi.has_credentials() {
            let executed_at = Utc::now();
            let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
            return Ok(ExecutionResult {
                request_id: request.request_id,
                idempotency_key: request.idempotency_key,
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(
                    "Kalshi credentials not configured. Set KALSHI_API_KEY and KALSHI_PRIVATE_KEY"
                        .to_string(),
                ),
                order_id: None,
                filled_qty: 0.0,
                avg_price: 0.0,
                fees: 0.0,
                platform: Platform::Kalshi,
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
            });
        }

        // Map ExecutionSide to Kalshi side string
        let side_str = match request.side {
            ExecutionSide::Yes => "yes",
            ExecutionSide::No => "no",
        };

        // Convert size to contracts (assuming size is in dollars and contracts are $1 each)
        let quantity = request.size.round() as i32;
        if quantity < 1 {
            let executed_at = Utc::now();
            let latency_ms = (executed_at - request.created_at).num_milliseconds() as f64;
            return Ok(ExecutionResult {
                request_id: request.request_id,
                idempotency_key: request.idempotency_key,
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(format!(
                    "Order size too small: {} contracts (minimum 1)",
                    quantity
                )),
                order_id: None,
                filled_qty: 0.0,
                avg_price: 0.0,
                fees: 0.0,
                platform: Platform::Kalshi,
                market_id: request.market_id.clone(),
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
            });
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
                    warn!("IOC order {} partially filled: {}/{}", order.order_id, filled_qty, quantity);
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
                    request_id: request.request_id,
                    idempotency_key: request.idempotency_key,
                    status,
                    rejection_reason: None,
                    order_id: Some(order.order_id),
                    filled_qty,
                    avg_price: request.limit_price, // TODO: Get actual fill price from order
                    fees,
                    platform: Platform::Kalshi,
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
            Err(e) => {
                let executed_at = Utc::now();
                let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

                error!("Kalshi order failed: {}", e);

                Ok(ExecutionResult {
                    request_id: request.request_id,
                    idempotency_key: request.idempotency_key,
                    status: ExecutionStatus::Failed,
                    rejection_reason: Some(format!("Kalshi API error: {}", e)),
                    order_id: None,
                    filled_qty: 0.0,
                    avg_price: 0.0,
                    fees: 0.0,
                    platform: Platform::Kalshi,
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
}
