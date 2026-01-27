# Service Latency Analysis: ZMQ vs Redis

**Purpose**: Determine which services need direct ZMQ access vs can stay on Redis slow track
**Date**: 2026-01-27
**Status**: ✅ Analysis Complete

---

## Service Categories

### Critical Path (MUST be on ZMQ)
**Requirement**: Sub-10ms latency, every millisecond matters
**Services**: game_shard_rust, execution_service_rust

### Real-Time Path (SHOULD be on ZMQ)
**Requirement**: 10-50ms latency, noticeable user impact
**Services**: api (WebSocket), notification_service_rust

### Background Path (FINE on Redis)
**Requirement**: 50-500ms latency, batch processing acceptable
**Services**: position_tracker_rust, analytics_service, orchestrator

---

## Detailed Service Analysis

### 1. position_tracker_rust 🟡 **BORDERLINE**

**Current Purpose**: Track P&L, positions, generate exit signals

**Latency Sensitivity**:
```
Trade Execution → position_tracker → Exit Signal
     (100ms)    →     (???ms)      →   (100ms)

If position_tracker is slow, exit signals delayed.
```

**Analysis**:

| Aspect | Redis (Current) | Direct ZMQ | Verdict |
|--------|----------------|------------|---------|
| **Input latency** | 10-20ms (zmq_listener lag) | 0.5ms (direct) | ⚠️ Matters for exits |
| **Processing time** | 5-10ms (P&L calc) | 5-10ms (same) | - |
| **Output urgency** | Exit signals time-sensitive | Exit signals time-sensitive | ⚠️ Matters |
| **Throughput** | 50-100 trades/sec | 50-100 trades/sec | - |

**Recommendation**: **Direct ZMQ for trade updates, Redis for position queries**

**Reasoning**:
- Exit signals ARE time-sensitive (want to exit fast when game swings)
- P&L calculations not compute-intensive
- 10-20ms Redis lag could miss optimal exit window

**Implementation**:
```rust
// services/position_tracker_rust/src/main.rs

// Subscribe to trade completions via ZMQ (hot path)
zmq_sub.connect("tcp://execution_service:5559")?;
zmq_sub.set_subscribe(b"trades.completed")?;

// Also read Redis for position queries (cold path)
let redis_client = redis::Client::open("redis://redis:6379")?;

loop {
    // ZMQ: Real-time trade updates
    match zmq_sub.recv_multipart(zmq::DONTWAIT) {
        Ok(parts) => {
            let trade: Trade = serde_json::from_slice(&parts[1])?;

            // Update position immediately
            update_position(&trade)?;

            // Check for exit conditions (time-sensitive!)
            if let Some(exit_signal) = check_exit_conditions(&trade)? {
                // Publish exit signal to ZMQ
                zmq_pub.send_multipart([
                    b"signals.exit",
                    &serde_json::to_vec(&exit_signal)?
                ], 0)?;
            }
        }
        Err(zmq::Error::EAGAIN) => {
            // No ZMQ message, continue
        }
        Err(e) => error!("ZMQ error: {}", e),
    }

    // Redis: Respond to position queries from API
    if let Some(query) = redis_sub.try_recv()? {
        let position = get_position(&query.game_id)?;
        redis.set(format!("position:{}:response", query.game_id), position)?;
    }
}
```

**Expected Benefit**: 10-20ms faster exit signals → better P&L on reversals

**Complexity**: Low (similar to game_shard pattern)

---

### 2. api (FastAPI) 🟢 **SHOULD USE ZMQ for WebSocket**

**Current Purpose**: REST API + WebSocket for frontend dashboard

**Latency Sensitivity**:
```
Frontend WebSocket → api → game state/prices
      (real-time)     (???ms)

User expects <100ms updates on dashboard
```

**Analysis**:

| Component | Redis (Current) | Direct ZMQ | Verdict |
|-----------|----------------|------------|---------|
| **REST API** | Fine (queries cached data) | No benefit | ✅ Keep Redis |
| **WebSocket** | 10-20ms stale data | Real-time (<1ms) | ⚡ Use ZMQ |

**Recommendation**: **Hybrid - REST on Redis, WebSocket on ZMQ**

**Reasoning**:
- REST queries: Latency doesn't matter (user clicking, 100ms is fine)
- WebSocket streaming: User watching live prices/signals, wants instant updates
- Redis lag = stale dashboard = bad UX

**Implementation**:
```python
# services/api/websocket.py

import zmq
import asyncio
from fastapi import WebSocket

class GameWebSocket:
    def __init__(self):
        # ZMQ subscriber for real-time updates
        self.zmq_context = zmq.Context()
        self.zmq_sub = self.zmq_context.socket(zmq.SUB)
        self.zmq_sub.connect("tcp://game_shard:5558")
        self.zmq_sub.subscribe(b"signals")
        self.zmq_sub.subscribe(b"games")

        # Redis for historical queries
        self.redis = redis.Redis()

    async def stream_updates(self, websocket: WebSocket, game_id: str):
        # Subscribe to specific game
        topic_filter = f"games.{game_id}".encode()
        self.zmq_sub.subscribe(topic_filter)

        while True:
            try:
                # Non-blocking ZMQ receive
                topic, data = self.zmq_sub.recv_multipart(zmq.NOBLOCK)

                # Send to WebSocket client immediately
                await websocket.send_json({
                    "topic": topic.decode(),
                    "data": json.loads(data)
                })
            except zmq.Again:
                await asyncio.sleep(0.01)  # 10ms poll
            except Exception as e:
                logger.error(f"WebSocket error: {e}")
                break
```

**Expected Benefit**:
- Dashboard updates 10-20ms faster (real-time vs slightly stale)
- Better UX for live trading monitoring

**Complexity**: Low (Python ZMQ binding is simple)

---

### 3. notification_service_rust 🟢 **SHOULD USE ZMQ**

**Current Purpose**: Send Signal alerts (SMS, webhooks) when high-edge opportunities

**Latency Sensitivity**:
```
Signal Generated → notification_service → SMS/Webhook
     (1ms)      →       (???ms)        →    (500ms)

Want notifications as fast as possible for manual trading
```

**Analysis**:

| Aspect | Redis (Current) | Direct ZMQ | Verdict |
|--------|----------------|------------|---------|
| **Input latency** | 10-20ms (zmq_listener) | 0.5ms (direct) | ⚡ Matters |
| **Processing** | 1-2ms (format message) | 1-2ms (same) | - |
| **Output** | SMS (500ms), Webhook (100ms) | Same | - |
| **Urgency** | High (manual trader needs alert ASAP) | High | ⚡ Matters |

**Recommendation**: **Direct ZMQ**

**Reasoning**:
- If you're sending SMS/webhook for manual trading, every millisecond counts
- User wants to see alert before price moves
- 10-20ms Redis lag could mean missed opportunity

**Implementation**:
```rust
// services/notification_service_rust/src/main.rs

// Subscribe to high-priority signals via ZMQ
zmq_sub.connect("tcp://game_shard:5558")?;
zmq_sub.set_subscribe(b"signals.high_edge")?;

loop {
    let parts = zmq_sub.recv_multipart(0)?;
    let signal: TradingSignal = serde_json::from_slice(&parts[1])?;

    // Filter: Only notify if edge > threshold
    if signal.edge_pct >= 10.0 {
        // Send SMS via Twilio (async, non-blocking)
        tokio::spawn(async move {
            send_sms(&signal).await;
        });

        // Send webhook (async, non-blocking)
        tokio::spawn(async move {
            send_webhook(&signal).await;
        });
    }
}
```

**Expected Benefit**: 10-20ms faster alerts → manual trader has more time to act

**Complexity**: Very Low (already Rust, just change subscriber)

---

### 4. analytics_service ⚪ **FINE ON REDIS**

**Current Purpose**: Historical analysis, generate reports, ML features

**Latency Sensitivity**:
```
Scheduled Job → analytics_service → Generate Report
   (cron)     →      (???ms)       →   (minutes)

Reports are batch jobs, no real-time requirement
```

**Analysis**:

| Aspect | Redis (Current) | Direct ZMQ | Verdict |
|--------|----------------|------------|---------|
| **Input latency** | 10-20ms (zmq_listener) | 0.5ms (direct) | ✅ Doesn't matter |
| **Processing** | Minutes (SQL queries, ML) | Minutes (same) | - |
| **Output urgency** | Low (scheduled reports) | Low | ✅ Doesn't matter |
| **Data source** | Database (historical) | Database (same) | - |

**Recommendation**: **Keep on Redis**

**Reasoning**:
- Batch processing (runs every hour/day)
- Processing time >> messaging latency (minutes vs milliseconds)
- Mostly queries database, not real-time streams
- 10-20ms lag is 0.001% of total processing time

**No Changes Needed**: ✅

---

### 5. orchestrator_rust 🟡 **BORDERLINE**

**Current Purpose**: ESPN game discovery, shard assignment, health monitoring

**Latency Sensitivity**:
```
ESPN API → orchestrator → Discover Game → Publish
  (1-5s)  →   (???ms)   →    (???ms)    →  (???ms)

Game discovery is periodic (every 30-60s), not latency-sensitive
```

**Analysis**:

| Component | Redis (Current) | Direct ZMQ | Verdict |
|-----------|----------------|------------|---------|
| **Game discovery** | Batch (30-60s) | Batch (same) | ✅ Doesn't matter |
| **Publishing discovered games** | 10-20ms (Redis PUB) | 0.5ms (ZMQ PUB) | ⚠️ Slight benefit |
| **Health checks** | 10s interval | 10s interval | ✅ Doesn't matter |

**Recommendation**: **Migrate orchestrator as ZMQ PUBLISHER (low effort)**

**Reasoning**:
- Publishing is easy (just change PUB socket)
- Subscribing not needed (orchestrator is pure publisher)
- Minimal effort, eliminates one Redis hop for game_shard

**Implementation**:
```rust
// services/orchestrator_rust/src/main.rs

// Add ZMQ publisher (in addition to Redis for now)
let zmq_pub = zmq_context.socket(zmq::PUB)?;
zmq_pub.bind("tcp://*:5557")?;

// When game discovered
async fn publish_game_discovered(&self, game: GameInfo) -> Result<()> {
    // ZMQ publish (hot path)
    let topic = format!("games.{}.{}", game.sport, game.game_id);
    let payload = serde_json::to_vec(&game)?;
    self.zmq_pub.send_multipart([
        topic.as_bytes(),
        &payload
    ], 0)?;

    // Optional: Keep Redis for backward compat during migration
    // self.redis.publish("games:discovered", payload)?;

    Ok(())
}
```

**Expected Benefit**: 10-20ms faster game discovery propagation

**Complexity**: Very Low (just add ZMQ publisher)

---

### 6. market_discovery_rust ⚪ **FINE ON REDIS**

**Current Purpose**: Market ID lookup (Polymarket/Kalshi), team matching RPC

**Latency Sensitivity**:
```
RPC Request → market_discovery → Lookup Market ID → Response
   (sync)   →      (???ms)      →    (50-200ms)    →  (???ms)

Team matching is RPC (request-response), not pub/sub
```

**Analysis**:

| Aspect | Redis (Current) | Direct ZMQ | Verdict |
|--------|----------------|------------|---------|
| **Request type** | RPC (request-response) | RPC (same) | - |
| **Lookup time** | 50-200ms (API calls) | 50-200ms (same) | - |
| **Cache hit** | <1ms (Redis GET) | <1ms (same) | - |
| **Pattern** | REQ-REP (Redis BRPOP) | REQ-REP (ZMQ REQ-REP) | ⚠️ Comparable |

**Recommendation**: **Keep on Redis**

**Reasoning**:
- RPC pattern, not pub/sub (different use case)
- Bottleneck is external API calls (50-200ms), not messaging
- ZMQ REQ-REP not significantly faster than Redis BRPOP for RPC
- Redis provides persistent queue (if market_discovery crashes, requests aren't lost)

**No Changes Needed**: ✅

---

## Summary Table

| Service | Current Latency | With ZMQ | Benefit | Priority | Recommendation |
|---------|----------------|----------|---------|----------|----------------|
| **game_shard_rust** | 20ms | **0.5ms** | 19.5ms | 🔴 P0 | ✅ **MIGRATE** (critical path) |
| **execution_service_rust** | 20ms | **0.5ms** | 19.5ms | 🔴 P0 | ✅ **MIGRATE** (critical path) |
| **position_tracker_rust** | 10-20ms | **0.5ms** | 10-20ms | 🟡 P1 | ✅ **MIGRATE** (exit signals) |
| **notification_service_rust** | 10-20ms | **0.5ms** | 10-20ms | 🟡 P1 | ✅ **MIGRATE** (user alerts) |
| **api (WebSocket)** | 10-20ms | **0.5ms** | 10-20ms | 🟡 P1 | ✅ **MIGRATE** (real-time UX) |
| **orchestrator_rust** | 10-20ms | **0.5ms** | 10-20ms | 🟢 P2 | ⚠️ **OPTIONAL** (easy win) |
| **analytics_service** | 10-20ms | 0.5ms | ~0% | ⚪ P3 | ❌ **SKIP** (batch processing) |
| **market_discovery_rust** | 50-200ms | 50-200ms | ~0% | ⚪ P3 | ❌ **SKIP** (RPC, not pub/sub) |
| **api (REST)** | 100ms+ | 100ms+ | ~0% | ⚪ P3 | ❌ **SKIP** (user clicks) |

---

## Recommended Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                    ZMQ HOT PATH (<1ms)                       │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  PUBLISHERS:                                                 │
│  ├─ kalshi_monitor (prices.kalshi.*)                        │
│  ├─ polymarket_monitor (prices.poly.*)                      │
│  ├─ orchestrator (games.*.*)                                │
│  ├─ game_shard (signals.trade.*)                            │
│  └─ execution_service (trades.completed.*)                  │
│                                                               │
│  SUBSCRIBERS:                                                │
│  ├─ game_shard (prices.*, games.*)          ← 0.5ms        │
│  ├─ execution_service (signals.trade.*)     ← 0.5ms        │
│  ├─ position_tracker (trades.completed.*)   ← 0.5ms        │
│  ├─ notification_service (signals.*)        ← 0.5ms        │
│  ├─ api (WebSocket: games.*, signals.*)     ← 0.5ms        │
│  └─ zmq_listener (ALL: *.*)                 ← 0.5ms        │
│                                                               │
└───────────────────────┬──────────────────────────────────────┘
                        │
                        │ zmq_listener (async mirror)
                        ▼
┌──────────────────────────────────────────────────────────────┐
│                   REDIS SLOW TRACK (10-20ms)                 │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  READERS:                                                    │
│  ├─ analytics_service (batch reports)       ← 10-20ms       │
│  ├─ api (REST queries)                      ← 10-20ms       │
│  ├─ market_discovery (RPC)                  ← 10-20ms       │
│  └─ position_tracker (position queries)     ← 10-20ms       │
│                                                               │
└──────────────────────────────────────────────────────────────┘
```

---

## Throughput Analysis

### Current System (All Redis)

```
Redis pub/sub throughput: ~50K msg/sec
  - kalshi_monitor: 1K msg/sec (price updates)
  - polymarket_monitor: 1K msg/sec (price updates)
  - game_shard: 5K msg/sec (signals)
  - orchestrator: 0.1K msg/sec (game discovery)
  - Total: ~7K msg/sec

Current usage: 14% of Redis capacity → NO BOTTLENECK
```

**Verdict**: Redis throughput is **not a bottleneck** currently.

### With ZMQ Hybrid

```
ZMQ throughput: ~1M+ msg/sec
  - Hot path: 7K msg/sec → 0.7% of ZMQ capacity
  - zmq_listener: 7K msg/sec → Redis (same load)

Total Redis load: Same (7K msg/sec from zmq_listener)
```

**Verdict**: Throughput is **not improved** (Redis still writes 7K msg/sec). But **latency is improved** (hot path bypasses Redis reads).

---

## When Throughput Becomes an Issue

### Redis Bottleneck Symptoms

```
Redis CPU: >80%
Message latency: >50ms (vs 10-20ms normally)
Pub/sub lag: >1s (messages queue up)

At that point: ~350K+ msg/sec (50x current load)
```

**When will this happen?**
- 50x more games tracked (500+ concurrent games vs 10 current)
- 10x faster market updates (100ms → 10ms intervals)
- More markets per game (spreads, totals, props)

**Solution if it happens**: ZMQ already in place, no Redis bottleneck on hot path.

---

## Additional Optimizations Beyond ZMQ

### 1. Shared Memory for Same-Host Services

If all services on same host, use ZMQ IPC (Unix domain sockets):

```rust
// Instead of TCP
zmq_pub.bind("tcp://*:5555")?;

// Use IPC (0.1ms vs 0.5ms)
zmq_pub.bind("ipc:///tmp/arbees_prices.ipc")?;
```

**Latency improvement**: 0.5ms → 0.1ms (5x faster)

**Complexity**: Very low (just change bind address)

### 2. Binary Serialization

Instead of JSON, use MessagePack or Protobuf:

```rust
// JSON: ~1-2ms to serialize
let payload = serde_json::to_vec(&price_update)?;

// MessagePack: ~0.1-0.2ms to serialize (10x faster)
let payload = rmp_serde::to_vec(&price_update)?;
```

**Latency improvement**: ~1-2ms per message (both encode and decode)

**Complexity**: Medium (change serialization format)

### 3. Batching (for non-critical messages)

Batch multiple messages into single ZMQ send:

```rust
// Instead of sending 100 price updates individually (100 × 0.5ms = 50ms)
for price in prices {
    zmq_pub.send(&price, 0)?;  // 50ms total
}

// Batch into single message (1 × 0.5ms = 0.5ms)
let batch: Vec<PriceUpdate> = prices.collect();
zmq_pub.send(&serde_json::to_vec(&batch)?, 0)?;  // 0.5ms total
```

**Throughput improvement**: 100x (100 msg/sec → 10K msg/sec)

**Latency trade-off**: Adds batching delay (e.g., batch every 10ms)

**Complexity**: Medium (change subscriber to handle batches)

---

## Implementation Priority

### Phase 1: Critical Path (Week 1-2) 🔴
```
1. zmq_listener (bridge service)
2. game_shard → ZMQ subscriber
3. execution_service → ZMQ subscriber

Expected: 60ms savings on hot path
```

### Phase 2: Real-Time Services (Week 3-4) 🟡
```
4. position_tracker → ZMQ subscriber
5. notification_service → ZMQ subscriber
6. api (WebSocket) → ZMQ subscriber

Expected: Better UX, faster exit signals
```

### Phase 3: Optional Publishers (Week 5+) 🟢
```
7. orchestrator → ZMQ publisher

Expected: Cleaner architecture, marginal latency improvement
```

### Phase 4: Skip ⚪
```
✗ analytics_service (batch processing, no benefit)
✗ market_discovery (RPC pattern, not pub/sub)
✗ api (REST) (user latency tolerance, no benefit)
```

---

## Cost-Benefit Analysis

| Service | Implementation Effort | Latency Benefit | Worth It? |
|---------|----------------------|-----------------|-----------|
| game_shard | 4 hours | 19.5ms | ✅ **YES** (critical path) |
| execution_service | 2 hours | 19.5ms | ✅ **YES** (critical path) |
| position_tracker | 3 hours | 10-20ms | ✅ **YES** (exit signals) |
| notification_service | 1 hour | 10-20ms | ✅ **YES** (easy win) |
| api (WebSocket) | 2 hours | 10-20ms | ✅ **YES** (UX improvement) |
| orchestrator | 1 hour | 10-20ms | ⚠️ **MAYBE** (marginal) |
| analytics_service | 2 hours | ~0ms | ❌ **NO** (wasted effort) |
| market_discovery | 4 hours | ~0ms | ❌ **NO** (wrong pattern) |

**Total Effort**: 13-17 hours for all "YES" services

**Total Benefit**: ~60ms on critical path, better UX, faster alerts

---

## Conclusion

### Services That SHOULD Use ZMQ Directly:
1. ✅ **game_shard_rust** (critical path)
2. ✅ **execution_service_rust** (critical path)
3. ✅ **position_tracker_rust** (exit signals)
4. ✅ **notification_service_rust** (user alerts)
5. ✅ **api (WebSocket only)** (real-time dashboard)

### Services That Are FINE on Redis:
6. ⚪ **analytics_service** (batch processing)
7. ⚪ **market_discovery_rust** (RPC pattern)
8. ⚪ **api (REST)** (user tolerance)

### Summary:
- **zmq_listener handles the rest** → Redis stays populated for backward compat
- **No throughput issues** → Current load (7K msg/sec) is only 14% of Redis capacity
- **Focus on latency** → ZMQ for time-sensitive services, Redis for everything else

---

**Next Action**: Implement Phase 1 (critical path) first, measure results, then decide on Phase 2.

**Document Status**: ✅ Analysis Complete
**Last Updated**: 2026-01-27
