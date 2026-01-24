# Real Trading API Checklist

This document outlines the API requirements for moving from paper trading to real-money execution on Kalshi and Polymarket.

---

## Overview

| Platform | Current Status | Real Trading Ready |
|----------|----------------|-------------------|
| Kalshi | Paper trading with simulated fills | ⚠️ Partial (order placement exists, needs validation) |
| Polymarket | Paper trading with simulated fills | ❌ No (order placement not implemented) |

---

## Kalshi API Requirements

### Authentication

| Requirement | Status | Notes |
|-------------|--------|-------|
| API Key ID | ✅ Implemented | `KALSHI_API_KEY` env var |
| RSA Private Key | ✅ Implemented | `KALSHI_PRIVATE_KEY` or `KALSHI_PRIVATE_KEY_PATH` |
| RSA-PSS Signature | ✅ Implemented | `timestamp + method + path` format |
| Demo Environment Keys | ✅ Implemented | `KALSHI_DEMO_API_KEY`, `KALSHI_DEMO_PRIVATE_KEY` |

### Market Data (Read-Only)

| Endpoint | Status | Location |
|----------|--------|----------|
| GET /markets | ✅ Implemented | `markets/kalshi/client.py` |
| GET /markets/{ticker} | ✅ Implemented | `markets/kalshi/client.py` |
| GET /markets/{ticker}/orderbook | ✅ Implemented | `markets/kalshi/client.py` |
| GET /events | ✅ Implemented | `markets/kalshi/client.py` |
| WebSocket orderbook_delta | ✅ Implemented | `markets/kalshi/websocket/ws_client.py` |

### Trading (Write Operations)

| Endpoint | Status | Location | Notes |
|----------|--------|----------|-------|
| POST /portfolio/orders | ✅ Implemented | `markets/kalshi/client.py:place_order()` | Needs validation with real API |
| DELETE /portfolio/orders/{id} | ✅ Implemented | `markets/kalshi/client.py:cancel_order()` | Needs validation |
| GET /portfolio/orders | ⚠️ Missing | - | Needed for order status tracking |
| GET /portfolio/orders/{id} | ⚠️ Missing | - | Needed for individual order status |
| GET /portfolio/fills | ⚠️ Missing | - | Needed for fill reconciliation |

### Portfolio / Balance

| Endpoint | Status | Location | Notes |
|----------|--------|----------|-------|
| GET /portfolio/positions | ✅ Implemented | `markets/kalshi/client.py:get_positions()` | Needs validation |
| GET /portfolio/balance | ⚠️ Missing | - | Needed for balance checks |
| GET /portfolio/settlements | ⚠️ Missing | - | Needed for settlement tracking |

### Order Placement Requirements

```python
# Current implementation (markets/kalshi/client.py)
async def place_order(
    self,
    market_id: str,
    side: str,       # "yes" or "no"
    price: float,    # 0.0 to 1.0
    quantity: float, # number of contracts
) -> dict:
    data = {
        "ticker": market_id,
        "action": "buy",
        "side": side,  # "yes" or "no"
        "type": "limit",
        "count": int(quantity),
        "yes_price": int(price * 100) if side == "yes" else None,
        "no_price": int(price * 100) if side == "no" else None,
    }
```

**Missing for Production:**
1. `client_order_id` - Idempotency key to prevent duplicate orders
2. `expiration_ts` - Order expiration timestamp (optional)
3. `time_in_force` - IOC, GTC, etc. (optional but recommended)
4. Better error handling for partial fills, rejections

### Implementation Gaps

1. **Order Status Tracking**
   - Need `GET /portfolio/orders` to check order status
   - Need `GET /portfolio/fills` for fill reconciliation
   
2. **Balance Management**
   - Need `GET /portfolio/balance` before placing orders
   - Should validate sufficient funds before order placement
   
3. **Idempotency**
   - Must implement `client_order_id` generation
   - Track sent orders to detect duplicates
   
4. **Partial Fill Handling**
   - Current code assumes full fills
   - Need to handle partial fills and remaining quantities

---

## Polymarket API Requirements

### Authentication

Polymarket CLOB uses a wallet-based authentication model:

| Requirement | Status | Notes |
|-------------|--------|-------|
| Ethereum Wallet | ❌ Missing | Private key for signing |
| API Credentials | ❌ Missing | API Key, API Secret, API Passphrase |
| Order Signing | ❌ Missing | EIP-712 typed data signing |
| Nonce Management | ❌ Missing | Track nonces for order signing |

### Market Data (Read-Only)

| Endpoint | Status | Location |
|----------|--------|----------|
| GET /markets (Gamma) | ✅ Implemented | `markets/polymarket/client.py` |
| GET /markets/{id} (Gamma) | ✅ Implemented | `markets/polymarket/client.py` |
| GET /book (CLOB) | ✅ Implemented | `markets/polymarket/client.py` |
| WebSocket orderbook | ✅ Implemented | `markets/polymarket/websocket/ws_client.py` |

### Trading (Write Operations)

| Endpoint | Status | Notes |
|----------|--------|-------|
| POST /order (CLOB) | ❌ Missing | Create signed order |
| DELETE /order/{id} (CLOB) | ❌ Missing | Cancel order |
| GET /orders (CLOB) | ❌ Missing | List open orders |
| GET /order/{id} (CLOB) | ❌ Missing | Get order status |
| GET /trades (CLOB) | ❌ Missing | Get fills/trades |

### Balance / Positions

| Endpoint | Status | Notes |
|----------|--------|-------|
| Wallet USDC Balance | ❌ Missing | Check wallet balance |
| Conditional Token Balances | ❌ Missing | Check token holdings |
| Token Approvals | ❌ Missing | Approve CLOB to spend tokens |

### Order Signing (EIP-712)

Polymarket CLOB orders require EIP-712 typed data signing:

```python
# Required for order placement (NOT currently implemented)
class PolymarketOrderSigner:
    def __init__(self, private_key: str, chain_id: int = 137):
        self.private_key = private_key
        self.chain_id = chain_id
    
    def sign_order(
        self,
        maker: str,         # Wallet address
        token_id: str,      # Market token ID
        side: str,          # "buy" or "sell"
        price: float,       # Price (0-1)
        size: float,        # Number of shares
        nonce: int,         # Unique nonce
        expiration: int,    # Unix timestamp
    ) -> dict:
        # Create EIP-712 typed data structure
        # Sign with private key
        # Return signed order payload
        pass
```

### Recommended Approach

**Option A: Use Official py-clob-client**
- GitHub: https://github.com/Polymarket/py-clob-client
- Handles signing, nonce management, order lifecycle
- Requires: pip install py-clob-client

**Option B: Implement Signing Manually**
- More control but more complex
- Need to implement EIP-712 signing
- Handle nonce management ourselves

### Implementation Gaps

1. **Wallet Setup**
   - Need Ethereum private key for signing
   - Wallet must be funded with USDC on Polygon
   - Wallet must have approved CLOB contract
   
2. **Order Signing**
   - Implement EIP-712 typed data signing
   - Or integrate py-clob-client library
   
3. **Nonce Management**
   - Track used nonces to prevent replay
   - Handle concurrent order placement
   
4. **Balance Tracking**
   - Query wallet USDC balance
   - Query conditional token balances
   - Handle token approvals

---

## Implementation Priority

### Phase 1: Kalshi Production (Lower Risk)
1. Add `GET /portfolio/balance` endpoint
2. Add `GET /portfolio/orders` for status tracking
3. Add `GET /portfolio/fills` for reconciliation
4. Implement idempotency with `client_order_id`
5. Test on Kalshi demo environment

### Phase 2: Polymarket Production (Higher Complexity)
1. Evaluate py-clob-client integration vs manual signing
2. Implement wallet management (secure key storage)
3. Implement order signing (EIP-712)
4. Implement balance/approval checks
5. Test with small amounts on mainnet (no testnet)

---

## Safety Requirements

### Pre-Trade Checks
- [ ] Verify sufficient balance before order
- [ ] Validate order parameters (price bounds, size limits)
- [ ] Check for duplicate orders (idempotency)
- [ ] Verify market is open/tradeable

### Post-Trade Reconciliation
- [ ] Confirm order acceptance
- [ ] Track partial fills
- [ ] Update local position state
- [ ] Log all order lifecycle events

### Risk Limits
- [ ] Maximum position size per market
- [ ] Maximum total exposure
- [ ] Rate limiting to prevent runaway trading
- [ ] Circuit breaker on consecutive losses

### Monitoring
- [ ] Real-time position tracking
- [ ] P&L monitoring with alerts
- [ ] Order rejection monitoring
- [ ] API error rate tracking

---

## Environment Configuration

### Kalshi
```bash
# Production
KALSHI_ENV=prod
KALSHI_API_KEY=your_prod_key
KALSHI_PRIVATE_KEY=your_prod_private_key

# Demo/Testnet (for testing)
KALSHI_ENV=demo
KALSHI_DEMO_API_KEY=your_demo_key
KALSHI_DEMO_PRIVATE_KEY=your_demo_private_key
```

### Polymarket
```bash
# Production (no testnet available)
POLYMARKET_API_KEY=your_api_key
POLYMARKET_API_SECRET=your_api_secret
POLYMARKET_API_PASSPHRASE=your_passphrase
POLYMARKET_PRIVATE_KEY=your_ethereum_private_key  # For signing
```

---

## Testing Strategy

1. **Unit Tests**: Mock API responses, test order building logic
2. **Integration Tests (Kalshi Demo)**: Real API calls on testnet
3. **Integration Tests (Polymarket)**: Read-only calls (no testnet)
4. **Paper Trading**: Continue current simulation with real market data
5. **Small Size Live**: Start with minimum order sizes on production

---

*Last updated: 2026-01-23*
*See also: [API_INVENTORY.md](API_INVENTORY.md)*
