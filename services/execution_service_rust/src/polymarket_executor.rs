//! Polymarket CLOB Executor
//!
//! Wraps the polymarket_clob client for use in the execution engine.

use anyhow::Result;
use arbees_rust_core::clients::polymarket_clob::{
    PolymarketAsyncClient, SharedAsyncClient, PreparedCreds,
};
use arbees_rust_core::models::{
    ExecutionRequest, ExecutionResult, ExecutionStatus, ExecutionSide, Platform,
};
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Polymarket CLOB executor for live trading
pub struct PolymarketExecutor {
    client: Arc<SharedAsyncClient>,
}

impl PolymarketExecutor {
    /// Create a new PolymarketExecutor from environment variables.
    ///
    /// Required environment variables:
    /// - `POLYMARKET_PRIVATE_KEY`: L1 wallet private key (hex, with 0x prefix)
    /// - `POLYMARKET_FUNDER_ADDRESS`: Funder/proxy wallet address
    ///
    /// Optional environment variables:
    /// - `POLYMARKET_CHAIN_ID`: 137 for mainnet, 80002 for Amoy testnet (default: 137)
    /// - `POLYMARKET_CLOB_HOST`: CLOB API host (default: https://clob.polymarket.com)
    /// - `POLYMARKET_API_NONCE`: Nonce for API key derivation (default: 0)
    pub async fn from_env() -> Result<Self> {
        let host = std::env::var("POLYMARKET_CLOB_HOST")
            .unwrap_or_else(|_| "https://clob.polymarket.com".to_string());
        let chain_id: u64 = std::env::var("POLYMARKET_CHAIN_ID")
            .unwrap_or_else(|_| "137".to_string())
            .parse()?;
        let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
            .map_err(|_| anyhow::anyhow!("POLYMARKET_PRIVATE_KEY not set"))?;
        let funder = std::env::var("POLYMARKET_FUNDER_ADDRESS")
            .map_err(|_| anyhow::anyhow!("POLYMARKET_FUNDER_ADDRESS not set"))?;

        info!(
            "Initializing Polymarket CLOB executor: host={}, chain_id={}",
            host, chain_id
        );

        let async_client = PolymarketAsyncClient::new(&host, chain_id, &private_key, &funder)?;

        // Derive API credentials
        let nonce: u64 = std::env::var("POLYMARKET_API_NONCE")
            .unwrap_or_else(|_| "0".to_string())
            .parse()?;

        info!("Deriving API credentials with nonce={}", nonce);
        let api_creds = async_client.derive_api_key(nonce).await?;
        let prepared_creds = PreparedCreds::from_api_creds(&api_creds)?;

        let shared = SharedAsyncClient::new(async_client, prepared_creds, chain_id);

        // Try to load neg_risk cache if available
        if let Ok(cache_path) = std::env::var("POLYMARKET_NEG_RISK_CACHE") {
            match shared.load_cache(&cache_path) {
                Ok(count) => info!("Loaded {} neg_risk entries from cache", count),
                Err(e) => warn!("Failed to load neg_risk cache: {}", e),
            }
        }

        info!("Polymarket CLOB executor initialized successfully");

        Ok(Self {
            client: Arc::new(shared),
        })
    }

    /// Execute an order on Polymarket CLOB
    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        let token_id = request
            .token_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Polymarket order requires token_id"))?;

        let start_time = Utc::now();

        debug!(
            "Executing Polymarket FAK order: token_id={}, side={:?}, price={}, size={}",
            token_id, request.side, request.limit_price, request.size
        );

        // Execute FAK order based on side
        let fill = match request.side {
            ExecutionSide::Yes => {
                // Buying YES token
                self.client
                    .buy_fak(token_id, request.limit_price, request.size)
                    .await?
            }
            ExecutionSide::No => {
                // For NO side, we sell YES token (equivalent to buying NO)
                // Price needs to be inverted: NO price = 1 - YES price
                let no_price = 1.0 - request.limit_price;
                self.client
                    .sell_fak(token_id, no_price, request.size)
                    .await?
            }
        };

        let executed_at = Utc::now();
        let latency_ms = (executed_at - start_time).num_milliseconds() as f64;

        // Calculate fees (2% Polymarket fee)
        let fees = fill.fill_cost * 0.02;
        let avg_price = if fill.filled_size > 0.0 {
            fill.fill_cost / fill.filled_size
        } else {
            0.0
        };

        let status = if fill.filled_size >= request.size {
            ExecutionStatus::Filled
        } else if fill.filled_size > 0.0 {
            ExecutionStatus::Partial
        } else {
            ExecutionStatus::Rejected
        };

        info!(
            "Polymarket order {}: status={:?}, filled={:.2}/{:.2}, avg_price={:.4}, fees={:.4}, latency={}ms",
            fill.order_id, status, fill.filled_size, request.size, avg_price, fees, latency_ms
        );

        Ok(ExecutionResult {
            request_id: request.request_id,
            idempotency_key: request.idempotency_key,
            status,
            rejection_reason: if fill.filled_size == 0.0 {
                Some("No fill".to_string())
            } else {
                None
            },
            order_id: Some(fill.order_id),
            filled_qty: fill.filled_size,
            avg_price,
            fees,
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
