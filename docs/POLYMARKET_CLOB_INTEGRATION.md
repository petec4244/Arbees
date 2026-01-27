# Polymarket CLOB Integration Plan

**Status:** Deferred - Requires separate focused project
**Priority:** Critical for dual-platform trading
**Estimated Effort:** 2-3 weeks

## Overview

The Polymarket CLOB (Central Limit Order Book) execution is entirely unimplemented. The current system can only:
- Monitor Polymarket prices via WebSocket (through `polymarket_monitor`)
- Detect arbitrage opportunities between Kalshi and Polymarket

It **cannot** execute trades on Polymarket. This document outlines the work required to implement full Polymarket trading capability.

## Current State

### What Works
- Price monitoring via Polymarket WebSocket API
- Publishing prices to Redis for signal generation
- Cross-platform arbitrage detection (Kalshi YES + Poly NO scenarios)

### What's Missing
- Order placement on Polymarket CLOB
- Position management on Polymarket
- Wallet/key management for signing
- Nonce tracking per market

## Technical Requirements

### 1. Ethereum Wallet Integration

Polymarket orders require EIP-712 typed data signatures. We need:

```rust
// Dependencies to add to Cargo.toml
ethers = { version = "2.0", features = ["rustls", "ws"] }
ethers-signers = "2.0"
```

**Implementation:**
- Load private key from environment variable or secure storage
- Create wallet signer for order signing
- Never expose private key in logs or error messages

### 2. EIP-712 Order Signing

Polymarket uses typed structured data signing (EIP-712) for orders:

```rust
use ethers::types::transaction::eip712::{EIP712Domain, Eip712};

#[derive(Debug, Clone, Eip712)]
#[eip712(
    name = "Polymarket",
    version = "1",
    chain_id = 137,  // Polygon mainnet
    verifying_contract = "0x..."
)]
struct PolymarketOrder {
    maker: Address,
    taker: Address,
    token_id: U256,
    maker_amount: U256,
    taker_amount: U256,
    side: u8,  // 0 = BUY, 1 = SELL
    expiration: U256,
    nonce: U256,
    fee_rate_bps: U256,
    signature_type: u8,
}
```

### 3. CLOB API Client

Create a new client in `rust_core/src/clients/polymarket_clob.rs`:

```rust
pub struct PolymarketClobClient {
    http_client: reqwest::Client,
    base_url: String,
    wallet: LocalWallet,
    nonce_tracker: Arc<RwLock<HashMap<String, U256>>>,  // market_id -> nonce
}

impl PolymarketClobClient {
    /// Place a limit order
    pub async fn place_order(
        &self,
        market_id: &str,
        side: OrderSide,
        price: f64,
        size: f64,
    ) -> Result<OrderResponse>;

    /// Cancel an order
    pub async fn cancel_order(&self, order_id: &str) -> Result<()>;

    /// Cancel all orders for a market
    pub async fn cancel_all(&self, market_id: &str) -> Result<()>;

    /// Get current positions
    pub async fn get_positions(&self) -> Result<Vec<Position>>;

    /// Get open orders
    pub async fn get_open_orders(&self) -> Result<Vec<Order>>;

    /// Get order status
    pub async fn get_order(&self, order_id: &str) -> Result<Order>;
}
```

### 4. Nonce Management

Each market has its own nonce that must be tracked:

```rust
pub struct NonceTracker {
    nonces: RwLock<HashMap<String, U256>>,
}

impl NonceTracker {
    /// Get next nonce for a market, incrementing the counter
    pub fn next_nonce(&self, market_id: &str) -> U256;

    /// Reset nonce from chain state (on startup or after errors)
    pub async fn sync_from_chain(&self, market_id: &str) -> Result<()>;
}
```

### 5. Execution Service Integration

Modify `execution_service_rust` to support Polymarket:

```rust
// In execution_service_rust/src/main.rs

async fn execute_trade(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    match request.platform {
        Platform::Kalshi => self.execute_kalshi(request).await,
        Platform::Polymarket => self.execute_polymarket(request).await,
        Platform::Paper => self.execute_paper(request).await,
    }
}

async fn execute_polymarket(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // 1. Get current market state
    // 2. Build order with correct nonce
    // 3. Sign order with EIP-712
    // 4. Submit to CLOB API
    // 5. Wait for fill confirmation
    // 6. Return execution result
}
```

## API Endpoints

### CLOB REST API
- Base URL: `https://clob.polymarket.com`
- Authentication: API key + signed requests

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/orders` | POST | Place new order |
| `/orders/{id}` | DELETE | Cancel order |
| `/orders` | GET | List open orders |
| `/positions` | GET | Get current positions |
| `/markets/{id}` | GET | Get market info |

### WebSocket API
- URL: `wss://ws-subscriptions-clob.polymarket.com/ws/`
- Already implemented in `polymarket_monitor`

## Security Considerations

1. **Private Key Storage**
   - Never log private keys
   - Use environment variables or secure vault
   - Consider hardware wallet integration for production

2. **Order Signing**
   - Validate all order parameters before signing
   - Implement order size limits
   - Add signature verification before submission

3. **Network Security**
   - All API calls over HTTPS
   - VPN required for CLOB API (geo-restricted)
   - Rate limiting to avoid API bans

## VPN Architecture

Polymarket CLOB requires EU IP address (same as WebSocket):

```
┌─────────────────────────────────────────────────────────────┐
│                      Docker Network                          │
│                                                              │
│  ┌──────────────┐     ┌──────────────┐     ┌─────────────┐ │
│  │   gluetun    │────▶│ polymarket   │────▶│    Redis    │ │
│  │   (VPN)      │     │   monitor    │     │             │ │
│  └──────────────┘     └──────────────┘     └─────────────┘ │
│         │                                         │         │
│         │             ┌──────────────┐            │         │
│         └────────────▶│ polymarket   │◀───────────┘         │
│                       │   executor   │                      │
│                       │   (NEW)      │                      │
│                       └──────────────┘                      │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Phases

### Phase 1: Foundation (Week 1)
- [ ] Add ethers-rs dependencies
- [ ] Implement wallet loading and signing
- [ ] Create basic CLOB client structure
- [ ] Add nonce tracking

### Phase 2: Order Management (Week 1-2)
- [ ] Implement place_order with EIP-712 signing
- [ ] Implement cancel_order
- [ ] Add order status polling
- [ ] Handle partial fills

### Phase 3: Integration (Week 2-3)
- [ ] Integrate with execution_service_rust
- [ ] Add Polymarket position tracking
- [ ] Implement cross-platform arbitrage execution
- [ ] Add comprehensive error handling

### Phase 4: Testing & Hardening (Week 3)
- [ ] Unit tests for signing logic
- [ ] Integration tests with testnet
- [ ] Paper trading mode for Polymarket
- [ ] Production deployment

## Dependencies

```toml
# Add to services/Cargo.toml or execution_service_rust/Cargo.toml
ethers = { version = "2.0", features = ["rustls", "ws"] }
ethers-signers = "2.0"
ethers-contract = "2.0"
```

## Environment Variables

```bash
# Polymarket CLOB Configuration
POLYMARKET_CLOB_API_KEY=<api_key>
POLYMARKET_PRIVATE_KEY=<ethereum_private_key>  # KEEP SECRET
POLYMARKET_CLOB_URL=https://clob.polymarket.com
POLYMARKET_CHAIN_ID=137  # Polygon mainnet

# VPN (already configured)
VPN_SERVICE_PROVIDER=nordvpn
VPN_TYPE=openvpn
```

## Risk Considerations

1. **Smart Contract Risk**: Polymarket operates on Polygon; smart contract bugs could affect funds
2. **Nonce Desync**: If nonces get out of sync, orders will fail until reset
3. **API Reliability**: CLOB API may have outages; need graceful degradation
4. **Geo-restrictions**: Must maintain VPN connectivity for all CLOB operations

## Decision: Why Deferred?

This work is deferred because:
1. **Kalshi-only is viable**: The system can operate profitably on Kalshi alone
2. **Complexity**: EIP-712 signing and nonce management add significant complexity
3. **Risk**: Errors in signing or execution could result in financial loss
4. **Testing**: Requires extensive testing before production use

The current system is ready for Kalshi live trading. Polymarket integration should be a separate, focused project with dedicated testing time.
