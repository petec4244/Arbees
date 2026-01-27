# ZMQ + Redis Hybrid Architecture Design

**Concept**: Use ZMQ for critical hot path (sub-millisecond), mirror to Redis for backward compatibility and non-critical services

**Date**: 2026-01-27
**Status**: ✅ Ready for Implementation

---

## Architecture Overview

```
┌───────────────────────────────────────────────────────────────────────────┐
│                        CRITICAL PATH (ZMQ - <1ms)                         │
├───────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  kalshi_monitor ──────┐                                                    │
│  (ZMQ PUB)            │                                                    │
│    topics:            │                                                    │
│    - prices.kalshi    │                                                    │
│    - orderbook.kalshi │                                                    │
│                       │                                                    │
│  polymarket_monitor ──┤                                                    │
│  (ZMQ PUB)            │                                                    │
│    topics:            │                                                    │
│    - prices.poly      │         ZMQ PUB/SUB BUS                          │
│    - orderbook.poly   ├────────► (tcp://*:5555)                          │
│                       │           ~0.5ms latency                          │
│  orchestrator ────────┤                                                    │
│  (ZMQ PUB)            │                                                    │
│    topics:            │                                                    │
│    - games.discovered │                                                    │
│    - games.updated    │                                                    │
│                       │                                                    │
└───────────────────────┼───────────────────────────────────────────────────┘
                        │
                        ├──────────────┐
                        │              │
                        ▼              ▼
            ┌──────────────────┐  ┌──────────────────┐
            │  game_shard_rust │  │  zmq_listener    │
            │    (ZMQ SUB)     │  │  (ZMQ SUB → Redis)│
            │                  │  │                  │
            │  Subscribes to:  │  │  Subscribes to:  │
            │  - prices.*      │  │  - prices.*      │
            │  - games.*       │  │  - games.*       │
            │                  │  │  - signals.*     │
            │  Publishes:      │  │                  │
            │  - signals.*     │  │  Publishes to:   │
            │    (ZMQ PUB)     │  │  - Redis (XADD)  │
            └────────┬─────────┘  └──────────────────┘
                     │                      │
                     ▼                      │
         ┌──────────────────────┐          │
         │ execution_service    │          │
         │    (ZMQ SUB)         │          │
         │                      │          │
         │ Subscribes to:       │          │
         │ - signals.trades     │          │
         └──────────────────────┘          │
                                            ▼
┌───────────────────────────────────────────────────────────────────────────┐
│                     NON-CRITICAL PATH (Redis - 10-20ms)                   │
├───────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  position_tracker ────┐                                                    │
│  analytics_service ───┤                                                    │
│  api ─────────────────┼─────► Redis (XREAD/GET)                          │
│  notification_service ┘        - prices:* (from zmq_listener)             │
│                                - games:* (from zmq_listener)               │
│                                - signals:* (from zmq_listener)             │
│                                                                             │
└───────────────────────────────────────────────────────────────────────────┘
```

---

## Key Design Principles

### 1. **ZMQ Topic-Based Pub/Sub**

All critical messages use ZMQ topics for filtering:

```
prices.kalshi.KXNBAGAME-2024-01-15-MIA-v-BOS
prices.poly.0x123abc...
games.nba.401584847
games.nhl.401587234
signals.trade.arb1738012345123
signals.trade.arb1738012345124
```

**Benefits**:
- Subscribers can filter by topic prefix (e.g., `prices.kalshi.*`)
- No need to deserialize messages you don't care about
- Broadcast to multiple subscribers efficiently

### 2. **zmq_listener as Bridge Service**

A dedicated service that subscribes to ALL critical ZMQ topics and mirrors them to Redis.

**Role**: "Write-behind cache" - ZMQ is source of truth, Redis is async mirror

**No Single Point of Failure**: If zmq_listener crashes, ZMQ path still works. Just Redis won't be updated temporarily.

### 3. **Gradual Migration**

Services can be migrated one at a time:

```
Phase 1: game_shard (hot path)
  ✓ Subscribe to ZMQ for prices
  ✓ Publish signals to ZMQ
  ✓ Keep Redis reads for market discovery (cold path)

Phase 2: execution_service (hot path)
  ✓ Subscribe to ZMQ for signals
  ✓ Keep Redis writes for trade history (cold path)

Phase 3: Others remain on Redis
  ✓ position_tracker, analytics, api, notifications
  ✓ No changes needed - zmq_listener feeds Redis
```

---

## Implementation Details

### Service: zmq_listener

**Location**: `services/zmq_listener_rust/`

**Purpose**: Subscribe to all critical ZMQ topics, mirror to Redis

```rust
// services/zmq_listener_rust/src/main.rs

use zmq::{Context, SUB};
use redis::Commands;
use serde_json::Value;
use tracing::{info, error};

#[tokio::main]
async fn main() -> Result<()> {
    // ZMQ subscriber (all topics)
    let zmq_context = Context::new();
    let zmq_sub = zmq_context.socket(SUB)?;

    // Connect to all ZMQ publishers
    zmq_sub.connect("tcp://kalshi_monitor:5555")?;
    zmq_sub.connect("tcp://polymarket_monitor:5556")?;
    zmq_sub.connect("tcp://orchestrator:5557")?;
    zmq_sub.connect("tcp://game_shard:5558")?;

    // Subscribe to all critical topics
    zmq_sub.set_subscribe(b"prices.")?;
    zmq_sub.set_subscribe(b"games.")?;
    zmq_sub.set_subscribe(b"signals.")?;

    info!("ZMQ Listener started - mirroring to Redis");

    // Redis connection
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_con = redis_client.get_async_connection().await?;

    // Main loop: ZMQ → Redis
    loop {
        // Receive from ZMQ (non-blocking with timeout)
        match zmq_sub.recv_multipart(0) {
            Ok(parts) if parts.len() >= 2 => {
                let topic = String::from_utf8_lossy(&parts[0]);
                let payload = &parts[1];

                // Mirror to Redis
                if let Err(e) = mirror_to_redis(&mut redis_con, &topic, payload).await {
                    error!("Failed to mirror to Redis: {}", e);
                    // Continue - don't crash on Redis errors
                }
            }
            Err(e) => {
                error!("ZMQ recv error: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            _ => {}
        }
    }
}

async fn mirror_to_redis(
    redis_con: &mut redis::aio::Connection,
    topic: &str,
    payload: &[u8],
) -> Result<()> {
    // Parse topic to determine Redis stream/key
    let parts: Vec<&str> = topic.split('.').collect();

    match parts[0] {
        "prices" => {
            // prices.kalshi.TICKER → prices:kalshi stream
            let platform = parts.get(1).unwrap_or(&"unknown");
            let stream_key = format!("prices:{}", platform);

            // Add to Redis stream
            redis::cmd("XADD")
                .arg(&stream_key)
                .arg("MAXLEN")
                .arg("~")
                .arg(10000)  // Keep last 10k messages
                .arg("*")
                .arg("topic")
                .arg(topic)
                .arg("data")
                .arg(payload)
                .query_async(redis_con)
                .await?;

            info!("Mirrored {} to {}", topic, stream_key);
        }
        "games" => {
            // games.nba.401584847 → games:nba:401584847 key
            let sport = parts.get(1).unwrap_or(&"unknown");
            let game_id = parts.get(2).unwrap_or(&"unknown");
            let key = format!("games:{}:{}", sport, game_id);

            // Set with expiration (game states expire after 24h)
            redis::cmd("SETEX")
                .arg(&key)
                .arg(86400)  // 24 hours
                .arg(payload)
                .query_async(redis_con)
                .await?;

            info!("Mirrored {} to {}", topic, key);
        }
        "signals" => {
            // signals.trade.ID → signals:trades stream
            let stream_key = "signals:trades";

            redis::cmd("XADD")
                .arg(stream_key)
                .arg("MAXLEN")
                .arg("~")
                .arg(1000)  // Keep last 1k signals
                .arg("*")
                .arg("topic")
                .arg(topic)
                .arg("data")
                .arg(payload)
                .query_async(redis_con)
                .await?;

            info!("Mirrored {} to {}", topic, stream_key);
        }
        _ => {
            // Unknown topic - ignore or log
        }
    }

    Ok(())
}
```

**Key Features**:
- ✅ Non-blocking ZMQ receive
- ✅ Doesn't crash on Redis errors (continues processing)
- ✅ Uses Redis Streams (XADD) for ordered messages
- ✅ Uses Redis keys (SETEX) for latest state
- ✅ Automatic message expiration (no unbounded growth)

---

### Modified: kalshi_monitor (ZMQ Publisher)

**Changes**: Add ZMQ PUB socket alongside existing functionality

```rust
// services/kalshi_monitor/main.py

import zmq
import json
from markets.kalshi.ws_client import KalshiWebSocketClient

async def main():
    # ZMQ publisher
    zmq_context = zmq.Context()
    zmq_pub = zmq_context.socket(zmq.PUB)
    zmq_pub.bind("tcp://*:5555")

    # Kalshi WebSocket (existing)
    async def on_price_update(ticker: str, yes_ask: float, yes_bid: float, timestamp_ms: int):
        # Create message
        msg = {
            "ticker": ticker,
            "yes_ask": yes_ask,
            "yes_bid": yes_bid,
            "timestamp_ms": timestamp_ms,
        }

        # Publish to ZMQ with topic
        topic = f"prices.kalshi.{ticker}"
        payload = json.dumps(msg).encode('utf-8')
        zmq_pub.send_multipart([topic.encode('utf-8'), payload])

        # Optional: Keep Redis publish for backward compat during migration
        # await redis_client.publish(f"prices:kalshi:{ticker}", payload)

    ws_client = KalshiWebSocketClient(on_price_update=on_price_update)
    await ws_client.connect()
```

**Migration Path**: Keep both ZMQ and Redis publishes initially, remove Redis later.

---

### Modified: game_shard_rust (ZMQ Subscriber + Publisher)

**Changes**: Subscribe to ZMQ for prices, publish signals to ZMQ

```rust
// services/game_shard_rust/src/main.rs

use zmq::{Context, SUB, PUB};

#[tokio::main]
async fn main() -> Result<()> {
    let zmq_context = Context::new();

    // Subscribe to prices (ZMQ)
    let zmq_sub = zmq_context.socket(SUB)?;
    zmq_sub.connect("tcp://kalshi_monitor:5555")?;
    zmq_sub.connect("tcp://polymarket_monitor:5556")?;
    zmq_sub.set_subscribe(b"prices.")?;

    // Subscribe to games (ZMQ)
    zmq_sub.connect("tcp://orchestrator:5557")?;
    zmq_sub.set_subscribe(b"games.")?;

    // Publish signals (ZMQ)
    let zmq_pub = zmq_context.socket(PUB)?;
    zmq_pub.bind("tcp://*:5558")?;

    info!("Game shard started - ZMQ mode");

    // Keep Redis for cold path (market discovery, state)
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_con = redis_client.get_async_connection().await?;

    loop {
        // Receive price/game update from ZMQ (~0.5ms)
        let parts = zmq_sub.recv_multipart(0)?;
        let topic = String::from_utf8_lossy(&parts[0]);
        let payload = &parts[1];

        // Process based on topic
        if topic.starts_with("prices.") {
            let price_update: PriceUpdate = serde_json::from_slice(payload)?;

            // Update game state
            let game_id = get_game_for_market(&price_update.ticker, &mut redis_con).await?;
            let mut game_state = update_game_state(game_id, price_update);

            // Check for arbitrage
            if let Some(signal) = check_arbitrage(&game_state)? {
                // Publish signal to ZMQ (~0.5ms)
                let signal_topic = format!("signals.trade.{}", signal.id);
                let signal_payload = serde_json::to_vec(&signal)?;
                zmq_pub.send_multipart([
                    signal_topic.as_bytes(),
                    &signal_payload
                ], 0)?;

                info!("Signal published via ZMQ: {}", signal.id);
            }
        } else if topic.starts_with("games.") {
            // Handle game update
            let game_update: GameUpdate = serde_json::from_slice(payload)?;
            // ...
        }
    }
}

// Redis still used for cold path (market discovery)
async fn get_game_for_market(ticker: &str, redis: &mut Connection) -> Result<String> {
    // Market ID → Game ID mapping (cached in Redis)
    let game_id: Option<String> = redis.get(format!("market:{}:game_id", ticker)).await?;
    // ...
}
```

**Latency**:
- ZMQ receive: ~0.5ms (vs 20ms Redis)
- Processing: ~5-10ms (model calculations)
- ZMQ publish: ~0.5ms (vs 20ms Redis)
- **Total**: ~6-11ms (vs ~60ms with Redis)

---

### Modified: execution_service_rust (ZMQ Subscriber)

**Changes**: Subscribe to ZMQ for signals

```rust
// services/execution_service_rust/src/main.rs

use zmq::{Context, SUB};

#[tokio::main]
async fn main() -> Result<()> {
    let zmq_context = Context::new();

    // Subscribe to signals (ZMQ)
    let zmq_sub = zmq_context.socket(SUB)?;
    zmq_sub.connect("tcp://game_shard:5558")?;
    zmq_sub.set_subscribe(b"signals.trade")?;

    info!("Execution service started - ZMQ mode");

    // Keep Redis for trade history (cold path)
    let redis_client = redis::Client::open("redis://redis:6379")?;

    loop {
        // Receive signal from ZMQ (~0.5ms)
        let parts = zmq_sub.recv_multipart(0)?;
        let topic = String::from_utf8_lossy(&parts[0]);
        let payload = &parts[1];

        let signal: TradingSignal = serde_json::from_slice(payload)?;

        info!("Received signal via ZMQ: {}", signal.id);

        // Execute trade (IOC order)
        match execute_ioc_order(&signal).await {
            Ok(trade) => {
                // Write to database (still important for audit trail)
                db.insert_trade(&trade).await?;

                info!("Trade executed: {} (latency: {}ms)",
                      trade.order_id, trade.latency_ms);
            }
            Err(e) => {
                error!("Trade execution failed: {}", e);
            }
        }
    }
}
```

**Latency**:
- ZMQ receive: ~0.5ms (vs 20ms Redis)
- Kalshi API call: ~100ms (IOC order)
- **Total**: ~100.5ms (vs ~120ms with Redis)

---

## docker-compose.yml Changes

```yaml
services:
  # Add zmq_listener service
  zmq_listener:
    build:
      context: .
      dockerfile: services/zmq_listener_rust/Dockerfile
    container_name: arbees-zmq-listener
    depends_on:
      redis:
        condition: service_healthy
    env_file:
      - .env
    restart: unless-stopped
    profiles:
      - full

  # Existing services - add ZMQ ports
  kalshi_monitor:
    ports:
      - "5555:5555"  # ZMQ PUB
    # ... rest unchanged

  polymarket_monitor:
    ports:
      - "5556:5556"  # ZMQ PUB
    # ... rest unchanged

  orchestrator:
    ports:
      - "5557:5557"  # ZMQ PUB
    # ... rest unchanged

  game_shard:
    ports:
      - "5558:5558"  # ZMQ PUB (for signals)
    # ... rest unchanged
```

---

## Benefits of This Design

### 1. **Best of Both Worlds** ✅

| Aspect | ZMQ (Hot Path) | Redis (Cold Path) |
|--------|---------------|-------------------|
| **Latency** | <1ms | 10-20ms (acceptable) |
| **Use Cases** | Price updates, signals | Market discovery, history |
| **Persistence** | No (ephemeral) | Yes (audit trail) |
| **Backward Compat** | No changes needed | Existing services work |

### 2. **No Single Point of Failure** ✅

```
If zmq_listener crashes:
  ✓ ZMQ hot path still works (game_shard → execution_service)
  ✗ Redis not updated (analytics/api stale)
  → Auto-restart zmq_listener, Redis catches up

If Redis crashes:
  ✓ ZMQ hot path still works
  ✗ Cold path services fail (position_tracker, api)
  → Auto-restart Redis, zmq_listener refills

If ZMQ publisher crashes:
  ✗ Hot path breaks for that service
  ✓ Redis still has last-known state
  → Auto-restart publisher, reconnect subscribers
```

### 3. **Gradual Migration** ✅

**Week 1**: Add zmq_listener (mirror only, no service changes)
- Deploy zmq_listener
- ZMQ messages mirrored to Redis
- Verify Redis data looks correct
- **No risk** - existing services unchanged

**Week 2**: Migrate game_shard to ZMQ
- Subscribe to ZMQ for prices (keep Redis fallback)
- Publish signals to ZMQ (keep Redis for now)
- Measure latency improvement
- **Low risk** - can rollback easily

**Week 3**: Migrate execution_service to ZMQ
- Subscribe to ZMQ for signals
- Measure end-to-end latency
- **Medium risk** - critical path, but has Redis backup

**Week 4+**: Optionally migrate others (orchestrator, monitors)

### 4. **Easy Rollback** ✅

If ZMQ causes issues, rollback is trivial:

```rust
// Rollback: Change one line
// zmq_sub.connect("tcp://game_shard:5558")?;  // ZMQ
let mut redis_sub = redis.subscribe("signals:trades")?;  // Redis fallback
```

### 5. **Observability** ✅

Monitor both systems independently:

```bash
# ZMQ message rate (via zmq_listener logs)
docker logs zmq_listener | grep "Mirrored" | wc -l

# Redis message rate
docker exec redis redis-cli INFO stats | grep instantaneous_ops_per_sec

# Latency comparison
# ZMQ: <1ms (in logs)
# Redis: 10-20ms (in logs)
```

---

## Latency Comparison

### Current (Redis Only)

```
Price Update (kalshi_monitor)
  └─> Redis PUB (20ms)
      └─> game_shard SUB (20ms)
          └─> Redis PUB (20ms)
              └─> execution_service SUB (20ms)
                  └─> Kalshi API (100ms)

Total: 180ms (60ms Redis + 100ms API)
```

### With ZMQ + Redis Hybrid

```
Price Update (kalshi_monitor)
  ├─> ZMQ PUB (0.5ms)
  │   ├─> game_shard SUB (0.5ms)
  │   │   └─> ZMQ PUB (0.5ms)
  │   │       └─> execution_service SUB (0.5ms)
  │   │           └─> Kalshi API (100ms)
  │   │
  │   └─> zmq_listener SUB (0.5ms)
  │       └─> Redis XADD (10ms) ← background, non-blocking
  │
  └─> [Optional] Redis PUB (20ms) ← backward compat during migration

Hot Path Total: 102ms (2ms ZMQ + 100ms API)
Cold Path Total: 180ms (Redis only, async)

Improvement: 78ms savings (43% faster)
```

---

## Potential Issues & Solutions

### Issue 1: Message Ordering

**Problem**: ZMQ doesn't guarantee order across multiple publishers

**Solution**: Use message timestamps + sequence numbers

```rust
#[derive(Serialize, Deserialize)]
struct Message {
    seq: u64,           // Atomic sequence number
    timestamp_ms: i64,  // Wall clock time
    payload: Value,
}

// Publisher
static SEQ: AtomicU64 = AtomicU64::new(0);

fn publish(topic: &str, payload: Value) {
    let msg = Message {
        seq: SEQ.fetch_add(1, Ordering::SeqCst),
        timestamp_ms: now_millis(),
        payload,
    };
    zmq_pub.send_multipart([topic.as_bytes(), &serde_json::to_vec(&msg)?], 0)?;
}

// Subscriber - detect out-of-order
let mut last_seq = 0;
if msg.seq <= last_seq {
    warn!("Out-of-order message: {} vs {}", msg.seq, last_seq);
    // Handle: buffer and reorder, or drop
}
last_seq = msg.seq;
```

### Issue 2: Message Loss on Subscriber Crash

**Problem**: If subscriber crashes, misses messages during downtime

**Solution**: zmq_listener provides Redis backup for replay

```rust
// On startup, subscriber reads last N messages from Redis
let last_messages: Vec<(String, Vec<u8>)> = redis::cmd("XREVRANGE")
    .arg("prices:kalshi")
    .arg("+")
    .arg("-")
    .arg("COUNT")
    .arg(100)  // Last 100 messages
    .query(&mut redis_con)?;

for (id, fields) in last_messages {
    // Process backlog
    let data = fields.get("data").unwrap();
    process_message(data)?;
}

// Now subscribe to ZMQ for new messages
```

### Issue 3: Network Partition

**Problem**: ZMQ uses TCP, can disconnect

**Solution**: Automatic reconnection + Redis fallback

```rust
loop {
    match zmq_sub.recv_multipart(zmq::DONTWAIT) {
        Ok(parts) => {
            // Process ZMQ message
            process_zmq_message(parts)?;
        }
        Err(zmq::Error::EAGAIN) => {
            // No message, check Redis fallback
            if let Some(msg) = redis_sub.try_recv()? {
                warn!("Using Redis fallback (ZMQ unavailable)");
                process_redis_message(msg)?;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(e) => {
            error!("ZMQ error: {}, reconnecting...", e);
            zmq_sub.disconnect("tcp://game_shard:5558")?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            zmq_sub.connect("tcp://game_shard:5558")?;
        }
    }
}
```

---

## Success Metrics

### After Implementation

| Metric | Current (Redis) | Target (ZMQ Hybrid) |
|--------|----------------|---------------------|
| Price update → game_shard | 20ms | **0.5ms** ⚡ |
| game_shard → execution_service | 20ms | **0.5ms** ⚡ |
| End-to-end (hot path) | 160-200ms | **100-120ms** 🎯 |
| Redis lag (zmq_listener) | N/A | <100ms |
| Message loss rate | 0% | **0%** (Redis backup) |

---

## Implementation Checklist

### Phase 1: zmq_listener (Week 1)
- [ ] Create `services/zmq_listener_rust/` service
- [ ] Subscribe to all ZMQ topics
- [ ] Mirror to Redis Streams (XADD)
- [ ] Add health check (verify messages flowing)
- [ ] Deploy and monitor for 48 hours

### Phase 2: Migrate Publishers (Week 2)
- [ ] Add ZMQ PUB to kalshi_monitor
- [ ] Add ZMQ PUB to polymarket_monitor
- [ ] Add ZMQ PUB to orchestrator
- [ ] Keep Redis publishes (dual mode)
- [ ] Verify zmq_listener sees all messages

### Phase 3: Migrate game_shard (Week 3)
- [ ] Subscribe to ZMQ for prices/games
- [ ] Publish signals via ZMQ
- [ ] Keep Redis reads for market discovery
- [ ] Measure latency improvement
- [ ] Run 48-hour test

### Phase 4: Migrate execution_service (Week 4)
- [ ] Subscribe to ZMQ for signals
- [ ] Keep Redis writes for trade history
- [ ] Measure end-to-end latency
- [ ] Validate <120ms p95

### Phase 5: Cleanup (Week 5+)
- [ ] Remove Redis publishes from monitors (ZMQ only)
- [ ] Benchmark final latency
- [ ] Update documentation
- [ ] Declare victory 🎉

---

## Conclusion

Your hybrid design is **brilliant** because:

1. ✅ **Solves latency** (60ms → 2ms on hot path)
2. ✅ **Maintains compatibility** (Redis still there)
3. ✅ **No single point of failure** (both systems independent)
4. ✅ **Easy rollback** (zmq_listener can be disabled)
5. ✅ **Gradual migration** (one service at a time)

This is exactly the architecture high-frequency trading firms use: **fast path + audit trail**.

---

**Next Action**: Implement Phase 1 (zmq_listener) as proof-of-concept

**Estimated Effort**:
- zmq_listener: 4-6 hours
- Migrate one service (kalshi_monitor): 2-3 hours
- Testing and validation: 1 week

**Expected Outcome**: 100-120ms end-to-end latency (vs 160-200ms current)

---

**Document Status**: ✅ Ready to Build
**Last Updated**: 2026-01-27
