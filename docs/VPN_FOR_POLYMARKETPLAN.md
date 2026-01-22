# Polymarket VPN Architecture Implementation Plan

## Overview

Implement a dedicated Polymarket Monitor shard running inside a Docker container with Gluetun VPN, enabling Polymarket API access from US while keeping Kalshi connections direct and low-latency.

**Architecture:**
```
┌─────────────────────────────────────────────────────────────┐
│                    Docker Host                               │
│                                                              │
│  ┌────────────────┐  ┌────────────────┐                     │
│  │ GameShard 1    │  │ GameShard 2    │  Direct → Kalshi    │
│  │ (NBA game)     │  │ (NHL game)     │  (10-30ms)          │
│  └───────┬────────┘  └───────┬────────┘                     │
│          │                   │                               │
│          └─────────┬─────────┘                               │
│                    ↓                                         │
│          ┌────────────────────┐                              │
│          │      Redis         │  (< 5ms pub/sub)            │
│          └────────────────────┘                              │
│                    ↑                                         │
│          ┌────────────────────┐                              │
│          │ Polymarket Monitor │                              │
│          │ (network_mode:vpn) │  VPN → Polymarket           │
│          └────────────────────┘  (150-300ms)                │
│                    ↑                                         │
│          ┌────────────────────┐                              │
│          │   Gluetun (VPN)    │  UK/EU Server               │
│          └────────────────────┘                              │
└─────────────────────────────────────────────────────────────┘
```

**Key Benefit:** Kalshi latency unaffected (direct), Polymarket prices distributed via Redis (< 5ms).

---

## Phase 1: Gluetun VPN Container

### Task 1.1: Add VPN Service to docker-compose.yml
**File:** `docker-compose.yml`

Add Gluetun VPN service:
```yaml
services:
  vpn:
    image: qmcgaw/gluetun
    container_name: arbees_vpn
    cap_add:
      - NET_ADMIN
    devices:
      - /dev/net/tun:/dev/net/tun
    environment:
      - VPN_SERVICE_PROVIDER=nordvpn
      - OPENVPN_USER=${NORDVPN_USER}
      - OPENVPN_PASSWORD=${NORDVPN_PASS}
      - SERVER_COUNTRIES=United Kingdom
      - FIREWALL_OUTBOUND_SUBNETS=192.168.0.0/16,172.16.0.0/12
      - HEALTH_VPN_DURATION_INITIAL=30s
    networks:
      - arbees-network
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "https://ipinfo.io"]
      interval: 60s
      timeout: 10s
      retries: 3
```

### Task 1.2: Add VPN Credentials to .env
**File:** `.env`

```bash
# VPN Configuration (NordVPN recommended)
NORDVPN_USER=your_email@example.com
NORDVPN_PASS=your_password
```

**Verification:**
```bash
docker-compose up -d vpn
docker exec arbees_vpn wget -qO- https://ipinfo.io/json
# Should show: {"country": "GB", ...}
```

---

## Phase 2: Polymarket Monitor Service

### Task 2.1: Create Service Directory
**Files to create:**
- `services/polymarket_monitor/__init__.py`
- `services/polymarket_monitor/monitor.py`
- `services/polymarket_monitor/Dockerfile`

### Task 2.2: Implement PolymarketMonitor
**File:** `services/polymarket_monitor/monitor.py`

```python
"""
Dedicated Polymarket price monitor running behind VPN.
Publishes prices to Redis for GameShards to consume.
"""

import asyncio
import os
import json
from datetime import datetime
from typing import Optional
import httpx
from loguru import logger

from markets.polymarket.hybrid_client import HybridPolymarketClient
from shared.arbees_shared.messaging import RedisBus, Channel


class PolymarketMonitor:
    """
    Monitors Polymarket markets via VPN and publishes prices to Redis.

    Designed to run in Docker container with network_mode: "service:vpn"
    """

    def __init__(self):
        self.redis = RedisBus()
        self.poly_client = HybridPolymarketClient()
        self.subscribed_tokens: set[str] = set()
        self._running = False
        self._health_ok = False

    async def start(self):
        """Initialize connections and start monitoring."""
        # Verify VPN before anything else
        await self._verify_vpn()
        self._health_ok = True

        # Connect to Redis and Polymarket
        await self.redis.connect()
        await self.poly_client.connect()
        self._running = True

        logger.info("PolymarketMonitor started successfully")

        # Run concurrent tasks
        await asyncio.gather(
            self._price_streaming_loop(),
            self._health_check_loop(),
            self._market_discovery_loop(),
        )

    async def _verify_vpn(self):
        """Verify VPN connection (must NOT be US IP)."""
        async with httpx.AsyncClient(timeout=10) as client:
            resp = await client.get("https://ipinfo.io/json")
            data = resp.json()

            country = data.get("country", "UNKNOWN")
            ip = data.get("ip", "UNKNOWN")

            if country == "US":
                raise RuntimeError(
                    f"VPN not working! Using US IP: {ip}. "
                    "Check NORDVPN_USER/PASS and VPN container health."
                )

            logger.info(f"VPN verified: {country} ({ip})")

    async def _price_streaming_loop(self):
        """Stream prices from Polymarket WebSocket and publish to Redis."""
        try:
            async for price_update in self.poly_client.stream_prices():
                # Publish to pub/sub for real-time consumers
                await self.redis.publish(
                    Channel.POLYMARKET_PRICES,
                    {
                        "market_id": price_update.market_id,
                        "token_id": price_update.token_id,
                        "yes_bid": price_update.yes_bid,
                        "yes_ask": price_update.yes_ask,
                        "no_bid": price_update.no_bid,
                        "no_ask": price_update.no_ask,
                        "liquidity": price_update.liquidity,
                        "timestamp_ms": int(datetime.utcnow().timestamp() * 1000),
                    }
                )

                # Store latest in hash for on-demand lookup
                await self.redis.client.hset(
                    "polymarket:latest_prices",
                    price_update.market_id,
                    json.dumps(price_update.to_dict())
                )
        except Exception as e:
            logger.error(f"Price streaming error: {e}")
            self._health_ok = False
            raise

    async def _health_check_loop(self):
        """Periodic health checks."""
        while self._running:
            try:
                await self._verify_vpn()

                if not self.poly_client.is_connected:
                    logger.warning("Polymarket WS disconnected, reconnecting...")
                    await self.poly_client.reconnect()

                self._health_ok = True

            except Exception as e:
                logger.error(f"Health check failed: {e}")
                self._health_ok = False

                # Publish alert
                await self.redis.publish(Channel.SYSTEM_ALERTS, {
                    "type": "POLYMARKET_MONITOR_UNHEALTHY",
                    "service": "polymarket_monitor",
                    "error": str(e),
                    "timestamp": datetime.utcnow().isoformat(),
                })

            await asyncio.sleep(60)

    async def _market_discovery_loop(self):
        """Subscribe to market assignment messages from orchestrator."""
        async for msg in self.redis.subscribe(Channel.MARKET_ASSIGNMENTS):
            if msg.get("platform") == "polymarket":
                token_id = msg.get("token_id")
                market_id = msg.get("market_id")

                if token_id and token_id not in self.subscribed_tokens:
                    await self.poly_client.subscribe(token_id)
                    self.subscribed_tokens.add(token_id)
                    logger.info(f"Subscribed to Polymarket: {market_id} ({token_id})")


async def main():
    """Entry point."""
    monitor = PolymarketMonitor()

    try:
        await monitor.start()
    except KeyboardInterrupt:
        logger.info("Shutting down...")
    except Exception as e:
        logger.error(f"Fatal error: {e}")
        raise


if __name__ == "__main__":
    asyncio.run(main())
```

### Task 2.3: Create Dockerfile
**File:** `services/polymarket_monitor/Dockerfile`

```dockerfile
FROM python:3.11-slim

WORKDIR /app

# Install system deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy requirements first for caching
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy shared packages
COPY shared/ /app/shared/
COPY markets/ /app/markets/

# Install shared package
RUN pip install -e /app/shared

# Copy service code
COPY services/polymarket_monitor/ /app/services/polymarket_monitor/

ENV PYTHONPATH=/app
ENV PYTHONUNBUFFERED=1

CMD ["python", "-m", "services.polymarket_monitor.monitor"]
```

### Task 2.4: Add Monitor to docker-compose.yml
**File:** `docker-compose.yml`

```yaml
  polymarket_monitor:
    build:
      context: .
      dockerfile: services/polymarket_monitor/Dockerfile
    container_name: arbees_polymarket_monitor
    network_mode: "service:vpn"  # All traffic through VPN
    depends_on:
      vpn:
        condition: service_healthy
    environment:
      - REDIS_URL=redis://redis:6379
      - LOG_LEVEL=INFO
    restart: unless-stopped
```

**Verification:**
```bash
docker-compose up polymarket_monitor
# Watch logs for "VPN verified: GB" and price updates
```

---

## Phase 3: Redis Channel Integration

### Task 3.1: Add Polymarket Channels
**File:** `shared/arbees_shared/messaging/channels.py`

Add to Channel enum:
```python
class Channel(str, Enum):
    # ... existing channels ...
    POLYMARKET_PRICES = "polymarket:prices"
    MARKET_ASSIGNMENTS = "orchestrator:market_assignments"
    SYSTEM_ALERTS = "system:alerts"
```

### Task 3.2: Add PolymarketPriceUpdate Type
**File:** `shared/arbees_shared/messaging/redis_bus.py`

```python
from dataclasses import dataclass, asdict

@dataclass
class PolymarketPriceUpdate:
    market_id: str
    token_id: str
    yes_bid: float
    yes_ask: float
    no_bid: float
    no_ask: float
    liquidity: float
    timestamp_ms: int

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> "PolymarketPriceUpdate":
        return cls(**data)
```

---

## Phase 4: GameShard Integration

### Task 4.1: Add Polymarket Price Consumer
**File:** `services/game_shard/shard.py`

Add to GameShard class:

```python
class GameShard:
    def __init__(self, ...):
        # ... existing init ...
        self._polymarket_prices: dict[str, PolymarketPriceUpdate] = {}
        self._use_polymarket_redis = os.getenv("POLYMARKET_VIA_REDIS", "true").lower() == "true"

    async def _start_background_tasks(self):
        """Start all background tasks."""
        tasks = [
            # ... existing tasks ...
        ]

        if self._use_polymarket_redis:
            tasks.append(self._polymarket_price_consumer())

        await asyncio.gather(*tasks)

    async def _polymarket_price_consumer(self):
        """Subscribe to Polymarket prices from monitor shard."""
        logger.info("Starting Polymarket price consumer (via Redis)")

        async for msg in self.redis.subscribe(Channel.POLYMARKET_PRICES):
            price = PolymarketPriceUpdate.from_dict(msg)
            self._polymarket_prices[price.market_id] = price

            # Trigger arb detection if we have matching Kalshi price
            kalshi_price = self._kalshi_prices.get(price.market_id)
            if kalshi_price:
                await self._check_arbitrage(price.market_id, kalshi_price, price)

    async def _check_arbitrage(self, market_id: str, kalshi: KalshiPrice, poly: PolymarketPriceUpdate):
        """Check for arbitrage using Rust SIMD detection."""
        import arbees_core

        mask = arbees_core.simd_check_arbs(
            int(kalshi.yes_ask * 100),
            int(kalshi.no_ask * 100),
            int(poly.yes_ask * 100),
            int(poly.no_ask * 100),
            threshold_cents=100,
        )

        if mask != 0:
            arb_types = arbees_core.simd_decode_mask(mask)
            logger.info(f"Arb detected on {market_id}: {arb_types}")
            await self._handle_arbitrage(market_id, arb_types, kalshi, poly)
```

### Task 4.2: Update GameShard docker-compose Entry
**File:** `docker-compose.yml`

```yaml
  game_shard:
    build:
      context: .
      dockerfile: services/game_shard/Dockerfile
    environment:
      - REDIS_URL=redis://redis:6379
      - KALSHI_API_KEY=${KALSHI_API_KEY}
      - POLYMARKET_VIA_REDIS=true  # Use Redis for Polymarket prices
    depends_on:
      - redis
      - polymarket_monitor
    # No VPN - direct connection for Kalshi
```

---

## Phase 5: Orchestrator Integration

### Task 5.1: Publish Market Assignments
**File:** `services/orchestrator/orchestrator.py`

When assigning games to shards, notify Polymarket monitor:

```python
async def _assign_game_to_shard(self, game: Game, shard_id: str):
    # ... existing assignment logic ...

    # Notify Polymarket monitor to subscribe to this market
    if game.polymarket_token_id:
        await self.redis.publish(Channel.MARKET_ASSIGNMENTS, {
            "platform": "polymarket",
            "market_id": game.market_id,
            "token_id": game.polymarket_token_id,
            "shard_id": shard_id,
            "sport": game.sport,
        })
        logger.debug(f"Published Polymarket assignment: {game.market_id}")
```

---

## Files Summary

| File | Action | Description |
|------|--------|-------------|
| `docker-compose.yml` | Modify | Add vpn + polymarket_monitor services |
| `.env` | Modify | Add NORDVPN_USER, NORDVPN_PASS |
| `services/polymarket_monitor/__init__.py` | Create | Package init |
| `services/polymarket_monitor/monitor.py` | Create | Main monitor service |
| `services/polymarket_monitor/Dockerfile` | Create | Container build |
| `shared/arbees_shared/messaging/channels.py` | Modify | Add new channels |
| `shared/arbees_shared/messaging/redis_bus.py` | Modify | Add PolymarketPriceUpdate |
| `services/game_shard/shard.py` | Modify | Add Redis price consumer |
| `services/orchestrator/orchestrator.py` | Modify | Publish market assignments |

---

## Verification Steps

### 1. Test VPN Container
```bash
docker-compose up -d vpn
sleep 30
docker exec arbees_vpn wget -qO- https://ipinfo.io/json
# Expected: {"country": "GB", "ip": "185.x.x.x", ...}
```

### 2. Test Polymarket Access Through VPN
```bash
docker exec arbees_vpn wget -qO- "https://clob.polymarket.com/markets?limit=1" | head -100
# Expected: JSON response with market data (not 403/451)
```

### 3. Test Full Pipeline
```bash
# Terminal 1: Watch Redis
docker exec arbees_redis redis-cli SUBSCRIBE polymarket:prices

# Terminal 2: Start services
docker-compose up polymarket_monitor game_shard

# Should see price messages in Terminal 1
```

### 4. Verify Latency Isolation
```bash
# Kalshi should be direct (10-30ms)
# Polymarket via Redis should be < 5ms after VPN
docker logs arbees_polymarket_monitor | grep "VPN verified"
docker logs arbees_shard_001 | grep "Polymarket price consumer"
```

---

## Rollback Strategy

If VPN approach fails:
1. Set `POLYMARKET_VIA_REDIS=false` in GameShard
2. Use existing `POLYMARKET_PROXY_URL` for EU proxy fallback
3. Polymarket monitor can be disabled without affecting Kalshi trading

---

## Cost

- NordVPN: ~$3-8/month (2-year plan recommended)
- Docker overhead: Minimal (~100MB RAM, 1% CPU)
- No additional hardware required
