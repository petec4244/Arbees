# API Inventory: Kalshi + Polymarket

This document inventories all REST and WebSocket API calls made in the codebase, mapping them to official documentation and identifying potential mismatches.

---

## Kalshi

### REST API

**Base URL (hard-coded)**: `https://api.elections.kalshi.com/trade-api/v2`  
**Location**: [`markets/kalshi/client.py`](../markets/kalshi/client.py) line 72

**Authentication**: RSA-PSS signature over `{timestamp}{method}{path}` with headers:
- `KALSHI-ACCESS-KEY`: API key ID
- `KALSHI-ACCESS-TIMESTAMP`: Unix timestamp in milliseconds
- `KALSHI-ACCESS-SIGNATURE`: Base64-encoded RSA-PSS signature

| Endpoint | Method | Purpose | Location | Notes |
|----------|--------|---------|----------|-------|
| `/markets` | GET | List markets | client.py:267 | Params: `limit`, `status`, `series_ticker` |
| `/markets/{ticker}` | GET | Get market details | client.py:276 | Returns `market` object |
| `/markets/{ticker}/orderbook` | GET | Get orderbook snapshot | client.py:286 | Returns `yes`/`no` bid arrays as `[price_cents, qty]` |
| `/portfolio/orders` | POST | Place limit order | client.py:428 | Payload: `ticker`, `action`, `side`, `type`, `count`, `yes_price`/`no_price` |
| `/portfolio/orders/{order_id}` | DELETE | Cancel order | client.py:434 | |
| `/portfolio/positions` | GET | Get open positions | client.py:443 | |
| `/events` | GET | Get events | client.py:470 | Optional `series_ticker` filter |

**Potential Issues**:
1. Base URL is hard-coded to "elections" subdomain - may differ for sports or demo environment
2. No environment toggle for demo/testnet

### WebSocket API

**WS URL (configurable)**: Default `wss://api.elections.kalshi.com/trade-api/ws/v2`  
**Location**: [`markets/kalshi/websocket/ws_client.py`](../markets/kalshi/websocket/ws_client.py)  
**Config**: `KALSHI_WS_URL` env var or `KALSHI_ENV=demo` for testnet

**Authentication**: RSA-PSS signed headers (same as REST API):
- `KALSHI-ACCESS-KEY`: API key ID
- `KALSHI-ACCESS-TIMESTAMP`: Unix timestamp in milliseconds
- `KALSHI-ACCESS-SIGNATURE`: Base64-encoded RSA-PSS signature over `{timestamp}GET/trade-api/ws/v2`

| Message Type | Direction | Purpose | Notes |
|--------------|-----------|---------|-------|
| `subscribe` | Client→Server | Subscribe to orderbook deltas | Payload: `{"id":1,"cmd":"subscribe","params":{"channels":["orderbook_delta"],"market_tickers":[...]}}` |
| `orderbook_snapshot` | Server→Client | Full orderbook state | Contains `msg.yes`/`msg.no` bid arrays |
| `orderbook_delta` | Server→Client | Incremental updates | Contains `msg.yes`/`msg.no` bid arrays |

**Orderbook Schema** (per Kalshi docs + Rust reference):
- `msg.yes`: YES side bids - `[[price_cents, qty], ...]`
- `msg.no`: NO side bids - `[[price_cents, qty], ...]`
- To get YES ask: `100 - best_NO_bid` (buying YES = selling NO)
- To get YES bid: `best_YES_bid`

**Status**: FIXED - Auth now uses RSA-PSS, schema correctly parses yes/no arrays with inversion.

---

## Polymarket

### Gamma API (Market Discovery)

**Base URL (hard-coded)**: `https://gamma-api.polymarket.com`  
**Location**: [`markets/polymarket/client.py`](../markets/polymarket/client.py) line 36

**Authentication**: Optional Bearer token (rarely needed for public endpoints)

| Endpoint | Method | Purpose | Location | Notes |
|----------|--------|---------|----------|-------|
| `/markets` | GET | List markets | client.py:344 | Params: `limit`, `offset`, `tag_id`, `active`, `closed` |
| `/markets/{condition_id}` | GET | Get market details | client.py:353 | Returns market with `clobTokenIds`, `outcomes`, `outcomePrices` |
| `/tags` | GET | List available tags | (referenced in comment) | Used to map sport→tag_id |

### CLOB API (Orderbook + Trading)

**Base URL (hard-coded)**: `https://clob.polymarket.com`  
**Location**: [`markets/polymarket/client.py`](../markets/polymarket/client.py) line 35

| Endpoint | Method | Purpose | Location | Notes |
|----------|--------|---------|----------|-------|
| `/book` | GET | Get orderbook | client.py:376 | Params: `token_id`. Returns `bids`/`asks` as `[{price, size}]` |
| `/markets/{condition_id}` | GET | Get market (fallback) | client.py:244 | Used when Gamma lookup fails |

**Status**: 
1. **ID detection**: FIXED - Now uses `_is_condition_id()` / `_is_token_id()` helpers instead of length heuristic:
   - Token IDs: Numeric strings (digits only)
   - Condition IDs: Hex strings (with or without 0x prefix, contains a-f chars)
2. **No trading implementation**: `place_order()` is not implemented for Polymarket CLOB

### WebSocket API

**WS URL (hard-coded)**: `wss://ws-subscriptions-clob.polymarket.com/ws/market`  
**Location**: [`markets/polymarket/websocket/ws_client.py`](../markets/polymarket/websocket/ws_client.py) line 33

**Authentication**: None (public feed)

| Message Type | Direction | Purpose | Notes |
|--------------|-----------|---------|-------|
| `{"type":"market","assets_ids":[...]}` | Client→Server | Subscribe to token orderbooks | Matches Rust reference bot |
| Orderbook updates | Server→Client | Price/depth changes | Parsed in `_parse_price_update()` |

**Potential Issues**:
1. URLs hard-coded - no override for alternative endpoints
2. No unsubscribe support in WS protocol (local state only)

---

## Cross-Cutting Issues

### ExecutionEngine (`services/execution_engine.py`)

- Uses `opportunity.event_id` as `market_id` (line 152-153)
- Comment notes this may be wrong - events contain multiple markets
- For real trading, must resolve to specific market ticker

### Environment Configuration

Currently **no centralized environment config** exists for:
- Kalshi prod vs demo vs testnet
- Polymarket endpoint overrides
- Separate API keys per environment

### Missing for Real Trading

| Platform | Gap | Priority |
|----------|-----|----------|
| Kalshi | Testnet URL config | High |
| Kalshi | WS auth validation | High |
| Kalshi | Fill/order status polling | Medium |
| Kalshi | Balance/settlement queries | Medium |
| Polymarket | Order placement (CLOB signing) | High |
| Polymarket | Wallet/balance management | High |
| Polymarket | Nonce tracking | High |
| Both | Idempotency keys | Medium |
| Both | Reconciliation loop | Medium |

---

## Official Documentation References

- **Kalshi**: https://docs.kalshi.com/
  - REST API: `/api-reference/`
  - WebSocket: `/websockets/`
  - Demo environment: mentioned in docs
- **Polymarket**: 
  - Gamma API: https://gamma-api.polymarket.com (self-documenting)
  - CLOB API: https://docs.polymarket.com/ (if available)
  - py-clob-client: https://github.com/Polymarket/py-clob-client

---

*Generated as part of API audit. See [`api_audit_+_live_trading_4e624c19.plan.md`](../.cursor/plans/api_audit_+_live_trading_4e624c19.plan.md) for full plan.*
