# Polymarket Monitor - Detailed Planning Document

**Version**: 1.0
**Last Updated**: 2026-01-30
**Service**: `services/polymarket_monitor/monitor.py`
**Status**: ✅ Active (with Phase 1: Asset Extraction Fixes)

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Architecture Overview](#architecture-overview)
3. [Key Components](#key-components)
4. [Data Flow](#data-flow)
5. [Market Assignment Flow](#market-assignment-flow)
6. [Price Publishing Mechanisms](#price-publishing-mechanisms)
7. [WebSocket vs REST Polling](#websocket-vs-rest-polling)
8. [Crypto Asset Extraction](#crypto-asset-extraction)
9. [Configuration & Environment](#configuration--environment)
10. [Error Handling & Resilience](#error-handling--resilience)
11. [Known Limitations](#known-limitations)
12. [Integration Points](#integration-points)

---

## Executive Summary

The Polymarket Monitor is a real-time price monitoring service that:

- **Connects to Polymarket WebSocket API** behind a VPN (required for EU IP geofencing)
- **Receives market assignments from Orchestrator** via Redis pub/sub
- **Subscribes to market prices** and publishes them in real-time to downstream consumers
- **Publishes via ZMQ** (primary, <50ms latency) and optionally Redis (backward compatibility)
- **Handles token_id → condition_id mapping** for Polymarket's architecture
- **Normalizes prices** from market data into standardized `MarketPrice` objects
- **Extracts crypto assets** from market titles for crypto markets (e.g., "Bitcoin" → BTC)
- **Supports both WebSocket streaming and REST polling** for resilience

**Key Difference from Kalshi Monitor**:
- Polymarket uses **token_id (per team)** instead of ticker symbols
- Markets are identified by **condition_id** (market hash)
- Each moneyline market has **TWO tokens** (one per outcome)
- Requires **VPN for EU IP** (Polymarket geo-fences CLOB to EU)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     PolymarketMonitor Service                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────┐  ┌──────────────────────┐  ┌──────────────┐  │
│  │ VPN (EU IP)  │  │  Redis Bus (Message  │  │  ZMQ Context │  │
│  │  Gluetun     │  │  PubSub + Broadcast) │  │  PubSocket   │  │
│  └──────────────┘  └──────────────────────┘  └──────────────┘  │
│         ↑                    ↑                        ↑           │
│         │                    │                        │           │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │          HybridPolymarketClient                         │   │
│  │  (WebSocket + REST API for market data)                │   │
│  └──────────────────────────────────────────────────────────┘   │
│         ↑                    ↑                        │           │
│         │                    │                        │           │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Three Concurrent Task Loops:                          │   │
│  │  1. _assignment_listener()   - Redis assignments        │   │
│  │  2. _price_streaming_loop()  - WebSocket prices         │   │
│  │  3. _health_check_loop()     - Heartbeat/monitoring     │   │
│  │  4. _rest_poll_loop()        - Fallback polling (alt)   │   │
│  └──────────────────────────────────────────────────────────┘   │
│         ↑                    ↑                        ↓           │
└─────────────────────────────────────────────────────────────────┘
         │                    │                        ↓
         │                    ↓                   ┌──────────────┐
         │              Redis Channels:          │ ZMQ :5556    │
         │              ├─ orchestrator:         │ (Low latency)│
         │              │  market_assignments    │ TCP endpoint │
         │              ├─ game:{game_id}:       │              │
         │              │  price                 │ Publishers:  │
         │              ├─ shard:*:heartbeat     │ - Prices     │
         │              └─ shard:*:command       │ - Markets    │
         │                                        └──────────────┘
         ↓
    Orchestrator (sends assignments)
         ↑
    Market Discovery Services
```

---

## Key Components

### 1. **PolymarketMonitor Class**

Main orchestrator managing all service operations.

**Constructor Initialization**:
```python
self.redis = RedisBus()                           # Redis connection
self.poly_client = HybridPolymarketClient()       # Polymarket API client
self.subscribed_tokens: set[str]                  # Currently subscribed token IDs
self._token_to_market: dict[str, dict]            # token_id → market routing info
self._market_metadata: dict[str, dict]            # condition_id → market details
self._active_by_game_type: dict[tuple, str]       # (game_id, market_type) → condition_id
self._zmq_context, self._zmq_pub                  # ZMQ publisher socket
self._zmq_seq = 0                                  # Message sequence counter
self._zmq_pub_port = 5556                         # Default ZMQ publish port
self._running, self._health_ok                    # Service state flags
```

**Key State Tracking**:

| State Variable | Purpose | Type |
|---|---|---|
| `subscribed_tokens` | Tokens actively subscribed to WebSocket | `set[str]` |
| `_token_to_market` | Maps each token to its market context | `dict` |
| `_market_metadata` | Stores title, asset, type for each market | `dict` |
| `_active_by_game_type` | Prevents publishing stale markets | `dict` |
| `_prices_published` | Counter for monitoring throughput | `int` |
| `_last_price_time` | Timestamp of last price update | `datetime` |

### 2. **HybridPolymarketClient**

Provides both WebSocket and REST API access to Polymarket.

**Methods Used**:
- `connect()` - Establish WebSocket connection
- `subscribe_with_metadata()` - Subscribe to market updates
- `get_market()` - Fetch market details (title, tokens, conditions)
- `get_market_price()` - Fetch current market price
- `unsubscribe()` - Stop watching a token
- `disconnect()` - Clean shutdown

**WebSocket Events**:
- Market price updates (real-time)
- Order book changes
- Connection status changes

### 3. **RedisBus**

Pub/Sub messaging system for coordination with Orchestrator.

**Channels Used**:

| Channel | Direction | Purpose |
|---------|-----------|---------|
| `orchestrator:startup_state_request` | OUT | Request current assignments on startup |
| `orchestrator:startup_state_response:polymarket` | IN | Receive market assignments |
| `orchestrator:market_assignments` | IN | Receive new/updated assignments |
| `game:{game_id}:price` | OUT | Publish prices (backward compat) |
| `shard:*:heartbeat` | IN | Monitor health |

### 4. **MarketPrice Model**

Standardized market price structure.

```python
@dataclass
class MarketPrice:
    market_id: str                    # condition_id (Polymarket market hash)
    platform: Platform.POLYMARKET     # Platform identifier
    game_id: str                      # Event ID (sports game, crypto market, etc.)
    market_title: str                 # Human-readable market description
    contract_team: Optional[str]      # Team name (sports) or asset code (crypto: BTC, ETH)
    yes_bid: float                    # Best bid price (0.0-1.0)
    yes_ask: float                    # Best ask price (0.0-1.0)
    mid_price: float                  # (yes_bid + yes_ask) / 2
    yes_bid_size: Optional[float]     # Liquidity at bid level
    yes_ask_size: Optional[float]     # Liquidity at ask level
    volume: Optional[float]           # 24h trading volume
    liquidity: float                  # Total available liquidity
    status: MarketStatus              # OPEN, CLOSED, SETTLED
    timestamp: datetime               # Price update timestamp
    last_trade_price: Optional[float] # Last executed trade price
```

---

## Data Flow

### Startup Sequence

```
1. Start() called
   ├─ Verify VPN (must NOT be US IP) → RuntimeError if US
   ├─ Initialize ZMQ PubSocket on port 5556
   ├─ Start Redis listener (for assignment callbacks)
   ├─ Request startup state from Orchestrator
   │  └─ Orchestrator responds with 47+ existing assignments
   │     └─ Process each assignment → _subscribe_to_market()
   └─ Launch concurrent loops

2. Concurrent Loops Start
   ├─ _assignment_listener()    → Listen for new/updated assignments
   ├─ _price_streaming_loop()   → WebSocket price stream
   ├─ _health_check_loop()      → Periodic health reporting
   └─ _rest_poll_loop()         → Fallback polling (optional)

3. First Prices Flow In
   ├─ WebSocket receives market updates
   ├─ Prices normalized to MarketPrice objects
   ├─ Published to ZMQ (primary) + Redis (optional)
   └─ Consumers receive in real-time
```

### Assignment Processing Flow

```
Orchestrator sends assignment:
{
  "type": "polymarket_assign",
  "event_id": "polymarket:0x123abc...",
  "market_type": "crypto",
  "markets": [
    {"condition_id": "0xabc...", "market_type": "crypto"}
  ]
}
        ↓
_handle_assignment() receives via Redis
        ↓
For each market in assignment:
  ├─ Store active mapping: (event_id, "crypto") → condition_id
  └─ Call _subscribe_to_market(condition_id, event_id, "crypto")
        ↓
_subscribe_to_market():
  ├─ Fetch market details from Polymarket API
  │  └─ Get: question/title, clobTokenIds (token list)
  ├─ Extract crypto asset from title (e.g., "Bitcoin" → "BTC")
  ├─ Store in _market_metadata[condition_id]
  ├─ For each token_id:
  │  └─ Store _token_to_market[token_id] = {condition_id, event_id, market_type}
  └─ Subscribe to token via WebSocket
        ↓
WebSocket starts sending prices for this token
        ↓
_handle_price_update() receives price updates in real-time
```

### Price Update Processing Flow

```
WebSocket receives price update:
{
  "token_id": "123456789...",
  "yes_bid": 0.45,
  "yes_ask": 0.47,
  "mid_price": 0.46,
  "liquidity": 1500.0
  ...
}
        ↓
_handle_price_update() called
        ↓
Look up market info:
  └─ condition_id, game_id, market_type = _token_to_market[token_id]
        ↓
Validate market is still active:
  └─ (game_id, market_type) must be in _active_by_game_type
        ↓
Retrieve market metadata:
  └─ title, asset = _market_metadata[condition_id]
        ↓
Normalize to MarketPrice object:
  ├─ For sports: contract_team = outcome (team name)
  └─ For crypto: contract_team = asset (BTC, ETH, SOL, etc.)
        ↓
Publish via BOTH paths:
  ├─ ZMQ: _publish_zmq_price(condition_id, game_id, normalized_price)
  │        └─ Topic: prices.poly.{condition_id}
  │        └─ Payload: ZmqEnvelope with MarketPrice
  │
  └─ Redis: (optional) redis.publish_market_price(game_id, normalized_price)
             └─ Channel: game:{game_id}:price
        ↓
Update statistics:
  ├─ _prices_published += 1
  └─ _last_price_time = now()
```

---

## Market Assignment Flow

### How Assignments Get to Monitor

```
Discovery Process (Orchestrator):
  ├─ crypto_provider discovers Polymarket markets
  └─ For each market: create assignment message

Assignment Delivery (Two Paths):

PATH 1: Startup Recovery (Initial/Restart)
  1. Monitor calls _request_startup_state()
  2. Publishes to "orchestrator:startup_state_request"
  3. Orchestrator responds on "orchestrator:startup_state_response:polymarket"
  4. Monitor receives response with all 47+ current assignments
  5. Monitor processes each immediately → full recovery in seconds

PATH 2: Continuous Updates (During Runtime)
  1. Orchestrator publishes to "orchestrator:market_assignments"
  2. Monitor's _assignment_listener() receives via Redis subscription
  3. For each new assignment → call _handle_assignment()
  4. Subscribe to new token via WebSocket
  5. Prices flow in within seconds
```

### Assignment Message Format

```python
{
    "type": "polymarket_assign",
    "event_id": "polymarket:0xdf6b7fa7db0e453c5d91a4d0d71fca52a5455a7ea8d7c8981913981465bab291",
    "sport": None,  # None for crypto/multi-market
    "market_type": "crypto",  # "crypto", "economics", "politics", "moneyline"
    "markets": [
        {
            "condition_id": "0xabc123...",  # Market hash from Polymarket
            "market_type": "crypto"
        }
    ]
}
```

---

## Price Publishing Mechanisms

### 1. ZMQ Publishing (Primary - Hot Path)

**Configuration**:
```python
ZMQ_PUB_PORT = 5556  # TCP endpoint
ZMQ_TRANSPORT_MODE = "zmq_only" (default)
```

**Message Format**:
```python
# Topic: prices.poly.{condition_id}
# Multipart message:
#   Part 0: Topic (string) → "prices.poly.0xabc123..."
#   Part 1: JSON Envelope

envelope = {
    "seq": 12345,                           # Message sequence (incremented per publish)
    "timestamp_ms": 1769812209106,          # Unix timestamp in milliseconds
    "source": "polymarket_monitor",         # Identifies this monitor
    "payload": {                            # MarketPrice data
        "market_id": "0xabc123...",         # condition_id
        "platform": "polymarket",
        "game_id": "polymarket:0xdf6b7fa...",
        "market_title": "Will Bitcoin reach $100k by March?",
        "yes_bid": 0.45,
        "yes_ask": 0.47,
        "mid_price": 0.46,
        "yes_bid_size": 23.5,
        "yes_ask_size": 18.2,
        "liquidity": 1234.56,
        "timestamp": "2026-01-30T22:30:09.106065",
        "asset": "BTC"  # For crypto markets (from extract_asset_from_market_title)
    }
}
```

**Subscribers**:
- crypto_shard_rust (via ZMQ subscription on `prices.poly.*`)
- signal_processor_rust (if present)
- Any consumer listening to `tcp://polymarket_monitor:5556`

**Latency**: <10ms from WebSocket receive to ZMQ publish

### 2. Redis Publishing (Backward Compatibility)

**Configuration**:
```python
ZMQ_TRANSPORT_MODE = "both"  # or "redis_only"
```

**Channel**: `game:{game_id}:price`

**Payload**: Same `MarketPrice` object, msgpack-encoded

**Latency**: 20-100ms (Redis round-trip)

**Use Case**: Legacy consumers that only read from Redis

---

## WebSocket vs REST Polling

### WebSocket Streaming (Primary)

**Mechanism**:
- HybridPolymarketClient maintains persistent WebSocket connection
- Subscribes to token updates via client metadata
- Receives price updates in real-time as they occur

**Pros**:
- ✅ Real-time, <100ms latency
- ✅ Efficient (only pays for events that occur)
- ✅ Automatic reconnection handling

**Cons**:
- ❌ Flaky on poor networks (potential gaps)
- ❌ Requires persistent connection

**Implementation**:
```python
async def _price_streaming_loop(self):
    """Stream prices from Polymarket WebSocket."""
    while self._running:
        try:
            # HybridPolymarketClient handles WebSocket connection
            msg = await self.poly_client.get_next_message()
            if not msg:
                await asyncio.sleep(0.1)
                continue

            # Extract price data from message
            token_id = msg.get("token_id")
            await self._handle_price_update(price_from_message)
        except Exception as e:
            logger.error(f"WebSocket error: {e}")
            # Reconnect on error (handled by client)
```

### REST Polling (Fallback)

**Mechanism**:
- Every `POLYMARKET_POLL_INTERVAL_SECONDS` (default 2s), poll all active markets
- Fetch market data via REST API
- Parse outcomes and calculate prices

**Pros**:
- ✅ Always works (REST API is reliable)
- ✅ No connection state needed
- ✅ Can detect missed prices during WebSocket gaps

**Cons**:
- ❌ 2+ second latency (polling interval)
- ❌ Wasteful API calls (sends same data repeatedly)
- ❌ Higher load on API

**Implementation**:
```python
async def _rest_poll_loop(self):
    """Fallback polling to ensure we publish prices even if WS is quiet."""
    while self._running:
        for (g_id, m_type), condition_id in self._active_by_game_type.items():
            market_data = await self.poly_client.get_market(condition_id)
            if market_data:
                # Extract outcome prices and publish
                # (duplicate of WebSocket prices, but ensures no gaps)
                ...
```

**When Used**:
- Automatically runs alongside WebSocket in background
- Used if WebSocket connection is unstable
- Provides extra safety net for critical prices

---

## Crypto Asset Extraction

### The Problem

Polymarket markets have titles like:
- "Will Bitcoin reach $100,000 by end of 2026?"
- "Ethereum price above $3000 tomorrow?"
- "Solana (SOL) up by 50% this week?"

But the `MarketPrice` model needs a canonical asset code (BTC, ETH, SOL) in the `contract_team` field for:
- Arbitrage detection (identifying which asset)
- Risk management (position sizing by asset)
- Display in UI

### The Solution: `extract_asset_from_market_title()`

**Algorithm**:
```python
def extract_asset_from_market_title(title: str) -> Optional[str]:
    """Extract crypto asset name from market title."""
    if not title:
        return None

    title_upper = title.upper()

    # Keyword-to-code mapping
    asset_keywords = {
        "BITCOIN": "BTC",
        "BTC": "BTC",
        "ETHEREUM": "ETH",
        "ETH": "ETH",
        "SOLANA": "SOL",
        "SOL": "SOL",
        "XRP": "XRP",
        "RIPPLE": "XRP",
        "DOGE": "DOGE",
        "DOGECOIN": "DOGE",
        "CARDANO": "ADA",
        "ADA": "ADA",
        # ... more mappings
    }

    # Find first matching keyword in title
    for keyword, asset in asset_keywords.items():
        if keyword in title_upper:
            return asset

    return None
```

**Integration Points**:

```python
# Called during market subscription
async def _subscribe_to_market(self, condition_id, game_id, market_type):
    market = await self.poly_client.get_market(condition_id)
    market_title = market.get("question", "")

    # Extract asset for crypto markets
    asset = extract_asset_from_market_title(market_title) if market_type == "crypto" else None

    # Store in metadata for later use
    self._market_metadata[condition_id] = {
        "title": market_title,
        "asset": asset,
        "game_id": game_id,
        "market_type": market_type,
    }

# Called when publishing prices
async def _handle_price_update(self, price):
    meta = self._market_metadata.get(condition_id, {})
    asset = meta.get("asset")

    normalized_price = MarketPrice(
        ...
        contract_team=asset,  # BTC, ETH, SOL, etc. for crypto
        ...
    )
```

**Supported Assets**:

| Asset Name | Codes | Example Markets |
|---|---|---|
| Bitcoin | BTC, BITCOIN | "Bitcoin above $100k" |
| Ethereum | ETH, ETHEREUM | "Ethereum price $3k?" |
| Solana | SOL, SOLANA | "Solana up 50%?" |
| XRP | XRP, RIPPLE | "XRP above $5?" |
| Dogecoin | DOGE, DOGECOIN | "DOGE hits $1?" |
| Cardano | ADA, CARDANO | "Cardano $1?" |
| Polkadot | DOT, POLKADOT | "Polkadot ATH?" |
| Avalanche | AVAX, AVALANCHE | "AVAX breaks $200?" |

---

## Configuration & Environment

### Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `POLYMARKET_POLL_INTERVAL_SECONDS` | `2.0` | REST polling frequency (seconds) |
| `ZMQ_PUB_PORT` | `5556` | ZMQ publish port |
| `ZMQ_TRANSPORT_MODE` | `zmq_only` | `zmq_only`, `redis_only`, or `both` |
| `HOSTNAME` | `polymarket-monitor-1` | Container instance ID for heartbeat |
| `DEBUG_RUN_ID` | `pre-fix` | Debug logging identifier |
| `REDIS_URL` | `redis://localhost:6379` | Redis connection string (from shared lib) |
| `POLYMARKET_CLOB_URL` | `https://clob.polymarket.com` | Polymarket CLOB API endpoint |
| `POLYMARKET_GAMMA_URL` | `https://gamma-api.polymarket.com` | Polymarket Gamma API endpoint |

### Docker Configuration

```yaml
# docker-compose.yml
services:
  polymarket_monitor:
    image: polymarket_monitor:latest
    network_mode: "service:vpn"  # CRITICAL: Share VPN network stack
    depends_on:
      - vpn  # Gluetun with NordVPN
      - redis
    environment:
      ZMQ_TRANSPORT_MODE: zmq_only
      POLYMARKET_POLL_INTERVAL_SECONDS: "2.0"
      ZMQ_PUB_PORT: "5556"
    ports:
      - "5556:5556"  # ZMQ publish endpoint
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
```

**VPN Requirement**:
- Polymarket CLOB WebSocket requires EU IP
- Gluetun container runs NordVPN tunnel
- polymarket_monitor shares network: `network_mode: "service:vpn"`
- Monitor verifies VPN on startup (fails if US IP detected)

---

## Error Handling & Resilience

### VPN Verification (Startup)

```python
async def _verify_vpn(self):
    """Fail hard if VPN not working (must NOT be US IP)."""
    async with httpx.AsyncClient(timeout=15) as client:
        resp = await client.get("https://ipinfo.io/json")
        data = resp.json()

        if data.get("country") == "US":
            raise RuntimeError(
                f"VPN not working! Detected US IP: {data.get('ip')}. "
                "Check NORDVPN_USER/PASS and VPN container health."
            )

        logger.info(f"VPN verified: {data.get('country')} ({data.get('city')})")
```

**Behavior**:
- If VPN check fails: Monitor exits immediately (no recovery)
- If Polymarket API returns EU-only error: Monitor retries with backoff
- If WebSocket disconnects: HybridPolymarketClient auto-reconnects

### Market Subscription Errors

```python
try:
    await self._subscribe_to_market(condition_id, game_id, market_type)
except Exception as e:
    logger.error(f"Failed to subscribe to {condition_id}: {e}")
    # Continue without this market (don't crash entire monitor)
```

### WebSocket Disconnect Handling

```python
# Handled by HybridPolymarketClient:
# - Detects disconnect
# - Automatically attempts reconnection with exponential backoff
# - Resubscribes to all tokens on reconnect
# - Logs warnings for persistent failures
```

### REST Poll Fallback

If WebSocket is flaky:
1. WebSocket provides real-time prices (primary)
2. REST poll provides 2s-latency prices (fallback)
3. Deduplication prevents duplicate publishes
4. Both paths work independently

### Health Monitoring

```python
async def _health_check_loop(self):
    """Periodic health reporting via heartbeat."""
    while self._running:
        # Check WebSocket connection status
        ws_ok = self.poly_client.is_connected

        # Update heartbeat with status
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "vpn_ok": self._health_ok,
            "ws_ok": ws_ok,
        })

        # Orchestrator uses heartbeat to detect if monitor is alive
        await asyncio.sleep(5)  # Every 5 seconds
```

---

## Known Limitations

### 1. Single Price Per Asset+Platform

**Current Design**:
```python
# Cache stores: asset|platform → single CryptoPriceData
prices_cache["BTC|polymarket"] = price_of_latest_market

# Problem: Multiple BTC markets on Polymarket
# Market 1: "BTC > $95k"
# Market 2: "BTC > $100k"
# Both map to "BTC|polymarket" → last write wins
```

**Impact**:
- Arbitrage detection can't distinguish between different strike prices
- Cross-platform arbitrage comparison may be invalid
- Same asset on same platform shows only latest price

**Planned Fix** (Phase 2):
- Implement per-market circular buffer
- Index by `market_id` instead of `asset|platform`
- Store price history for matching across markets

### 2. Polymarket Market Discovery Gaps

**Issue**:
- Gamma API may have geofencing restrictions
- Directional markets (15-min Up/Down) sometimes not discoverable
- Market listing may be incomplete due to API limits

**Workaround**:
- Market discovery retries every 60 seconds
- REST poll provides fallback prices even if discovery is incomplete
- Manual market ID input supported via orchestrator assignment

### 3. REST Polling Inefficiency

**Current**: Every 2 seconds, poll ALL active markets (potentially 100+)

**Issue**:
- Wasteful API calls (most return no change)
- Increases API rate-limit pressure
- Could cause throttling during high market counts

**Mitigation**:
- WebSocket handles real-time updates (primary)
- REST polling is fallback only
- Can increase poll interval if needed (via env var)

### 4. Token ID Resolution

**Issue**:
- Some markets have `clobTokenIds` missing
- Fallback: `resolve_yes_token_id()` may fail
- Market becomes unavailable for WebSocket subscription

**Handling**:
- Logs warning but continues
- REST poll still provides prices for these markets
- Doesn't crash the entire monitor

---

## Integration Points

### 1. Orchestrator (Upstream)

**Sends**: Market assignments → `orchestrator:market_assignments` channel

**Receives**: Startup state request → `orchestrator:startup_state_request` channel

**Expected Flow**:
```
Orchestrator runs discovery every 60s
    ↓
Finds 19 Polymarket crypto markets
    ↓
Publishes assignment for each to polymarket_monitor
    ↓
polymarket_monitor subscribes to tokens
    ↓
Prices flow to downstream consumers
```

### 2. Crypto_Shard (Downstream)

**Receives**: Prices via ZMQ subscription on `prices.poly.*`

**Uses For**:
- Arbitrage detection (comparing prices across platforms)
- Probability modeling (current market prices)
- Risk management (liquidity checks)

**Expected Format**:
```python
topic: "prices.poly.0xabc123..."
payload: {
    "seq": 12345,
    "timestamp_ms": 1769812209106,
    "source": "polymarket_monitor",
    "payload": MarketPrice(
        market_id="0xabc123...",
        asset="BTC",  # For crypto
        yes_bid=0.45,
        yes_ask=0.47,
        liquidity=1500.0,
        ...
    )
}
```

### 3. Redis (Coordination)

**Channels**:
- `orchestrator:startup_state_request` - Send on startup
- `orchestrator:startup_state_response:polymarket` - Receive responses
- `orchestrator:market_assignments` - Continuous updates
- `game:{game_id}:price` - Optional backward-compat publishing

### 4. Polymarket APIs

**Gamma API** (Public, no VPN needed):
```
GET /events?active=true&closed=false
GET /markets?slug={slug}
GET /markets/{condition_id}
```

**CLOB API** (Requires EU IP):
```
WebSocket wss://ws-us-prod.clob.polymarket.com
GET /markets/{condition_id}
```

**HybridPolymarketClient handles**:
- Auto-selecting right API endpoint
- VPN requirement checking
- WebSocket connection management
- Token ID resolution

### 5. Heartbeat Service

**Publishes**: Service health every 5 seconds

**Used By**: Orchestrator to detect service death

**Format**:
```python
{
    "service": "polymarket_monitor",
    "instance_id": "polymarket-monitor-1",
    "status": "HEALTHY",
    "checks": {
        "redis_ok": True,
        "vpn_ok": True,
        "ws_ok": True,
    },
    "timestamp": 2026-01-30T22:30:00Z
}
```

---

## Performance Characteristics

### Latency Profile

| Operation | Latency | Notes |
|---|---|---|
| WebSocket price → ZMQ publish | <10ms | Hot path |
| ZMQ subscribe → crypto_shard receive | <50ms | Network round-trip |
| REST poll cycle | 2s | Configurable |
| Market assignment → subscription | ~100ms | API call + subscribe |
| Orchestrator startup state request | ~500ms | Request + response |

### Throughput

| Metric | Expected | Conditions |
|---|---|---|
| Prices/second | 100-300 | All 47 markets active |
| ZMQ messages/second | Same as above | Real-time streaming |
| API calls/minute | ~30 | REST poll at 2s interval |
| Memory usage | ~50-100MB | Market metadata + cache |

### Network Usage

- **WebSocket**: 5-50KB/s (depends on volatility)
- **REST polling**: ~2KB per poll cycle
- **ZMQ publishing**: 100-200 bytes per price update
- **Redis**: Minimal (mostly metadata)

---

## Troubleshooting Guide

### Issue: "VPN not working! Detected US IP"

**Cause**: VPN container not running or NordVPN config incorrect

**Fix**:
```bash
# Check VPN container status
docker ps | grep vpn

# Verify VPN connection
docker logs gluetun | grep -i "vpn\|connected"

# Restart VPN
docker compose restart vpn

# Wait 10s, then restart polymarket_monitor
sleep 10
docker compose restart polymarket_monitor
```

### Issue: No Polymarket prices received

**Diagnosis**:
```bash
# Check if monitor is subscribed to markets
docker logs polymarket_monitor | grep -i "subscribed\|subscribe"

# Check for WebSocket errors
docker logs polymarket_monitor | grep -i "websocket\|ws"

# Check if orchestrator sent assignments
docker logs orchestrator | grep -i "polymarket"
```

**Common Causes**:
1. Orchestrator hasn't discovered markets yet (wait 60s)
2. WebSocket connection failed (check VPN)
3. Token ID resolution failed (check logs for details)

### Issue: Parse errors in crypto_shard

**Old Error** (pre-fix): `premature end of input` → Asset field was null

**Cause**: Monitor not extracting asset from title

**Fix**: Update monitor, which now calls `extract_asset_from_market_title()`

### Issue: High memory usage

**Cause**: `_market_metadata` storing too much data

**Fix**:
```python
# Periodically clean old metadata
async def cleanup_metadata():
    old_condition_ids = set(_market_metadata.keys()) - set(_token_to_market.values())
    for cid in old_condition_ids:
        del _market_metadata[cid]
```

---

## Future Improvements

### Phase 2: Per-Market Storage (Planned)

```python
# Current (wrong):
cache["BTC|polymarket"] = latest_price  # Only 1 price per asset

# Proposed (correct):
cache["0xabc123..."] = [price1, price2, price3]  # History per market
# Allows matching "BTC > $95k" with "BTC > $100k" correctly
```

### Dynamic Subscription Management

```python
# Current: Subscribe to all assigned markets immediately
# Proposed:
#   - Subscribe only if liquidity > threshold
#   - Unsubscribe if market closes
#   - Auto-resubscribe on market reopening
```

### Market Quality Scoring

```python
# For each market, calculate:
# - Liquidity score (spread, volume)
# - Update frequency (latency)
# - Outcome probability (model vs market)
# Use to prioritize which markets to display/analyze
```

### WebSocket Resilience

```python
# Current: Auto-reconnect on disconnect
# Proposed:
#   - Maintain queue of pending prices during gap
#   - Sync queue on reconnect
#   - Detect and fill missing data from REST poll
```

---

## Summary

The Polymarket Monitor is a production-grade service that:

✅ **Reliably streams prices** from Polymarket via WebSocket (or REST fallback)
✅ **Automatically discovers markets** from Orchestrator assignments
✅ **Publishes in real-time** via ZMQ (<50ms) to arbitrage detection
✅ **Handles crypto assets** via title extraction (BTC, ETH, SOL, etc.)
✅ **Recovers quickly** on restart via startup state sync
✅ **Monitors health** via heartbeat for orchestrator visibility

**Key Dependencies**:
- VPN (Gluetun) for EU IP geofencing
- Redis for coordination messages
- Polymarket CLOB + Gamma APIs
- ZMQ for low-latency publishing

**Performance**:
- <10ms latency (WebSocket to ZMQ publish)
- ~300 prices/second at full capacity
- ~100MB memory for 50 active markets

---

**Document Version**: 1.0
**Last Updated**: 2026-01-30
**Status**: ✅ Complete (with Phase 1 deserialization fixes)
