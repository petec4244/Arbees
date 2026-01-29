# ZMQ Primary Architecture Plan

## Goal

Transition from Redis-first to **ZMQ-first** internal communication while using `zmq_listener` as an observer that logs all traffic for debugging, analytics, and historical replay.

## Current vs Target Architecture

### Current (Redis-First)
```
Monitor → Redis pub/sub → Game Shard → Redis → Signal Processor → Redis → Execution
                ↑
          zmq_listener (bridges ZMQ→Redis when mode=both)
```

### Target (ZMQ-First)
```
┌─────────────────────────────────────────────────────────────────────────┐
│                        HOT PATH (ZMQ Direct)                            │
│                                                                         │
│  Kalshi Monitor ──┐                                                     │
│  (PUB :5555)      │                                                     │
│                   ├──> Game Shard ──> Signal Processor ──> Execution   │
│  Polymarket Monitor                                                     │
│  (PUB :5556)      │    (SUB+PUB     (SUB+PUB            (SUB)          │
│                   │     :5558)       :5559)                            │
└───────────────────┼─────────────────────────────────────────────────────┘
                    │
┌───────────────────▼─────────────────────────────────────────────────────┐
│                    SLOW PATH (Observer/Logger)                          │
│                                                                         │
│  zmq_listener (SUB to all endpoints)                                   │
│       │                                                                │
│       ├──> Console logs (structured JSON)                              │
│       ├──> Redis streams (for historical queries)                      │
│       ├──> File logging (optional, for replay)                         │
│       └──> Metrics/Prometheus (optional)                               │
│                                                                         │
│  Frontend/Dashboard (reads from Redis streams)                         │
│  Analytics (reads from Redis streams)                                  │
│  Debugging tools (tail logs)                                           │
└─────────────────────────────────────────────────────────────────────────┘
```

## Changes Required

### 1. Environment Configuration

```bash
# .env changes
ZMQ_TRANSPORT_MODE=zmq_only   # Change default from redis_only

# zmq_listener specific
ZMQ_LISTENER_MODE=observer    # New: "observer" | "bridge" | "disabled"
ZMQ_LISTENER_LOG_LEVEL=info   # Log verbosity for message logging
ZMQ_LISTENER_LOG_FILE=/var/log/arbees/zmq_traffic.jsonl  # Optional file output
ZMQ_LISTENER_REDIS_STREAMS=true   # Write to Redis streams for queries
ZMQ_LISTENER_CONSOLE_LOG=true     # Log to stdout
```

### 2. Monitor Changes (Python)

**Files:**
- `services/kalshi_monitor/monitor.py`
- `services/polymarket_monitor/monitor.py`

Changes:
- Remove Redis publishing from hot path
- ZMQ becomes mandatory (not optional)
- Keep Redis only for non-critical data (market metadata, assignment acks)

```python
# Before: Dual publish
async def publish_price(self, price: MarketPrice):
    # Redis (current primary)
    await self.redis.publish(f"game:{price.game_id}:price", price.json())
    # ZMQ (optional)
    if self._zmq_enabled:
        await self._zmq_pub.send_multipart([topic, envelope])

# After: ZMQ only for hot path
async def publish_price(self, price: MarketPrice):
    # ZMQ is the hot path (always enabled)
    await self._zmq_pub.send_multipart([topic, envelope])
    # Redis only for slow-path consumers (via zmq_listener observer)
```

### 3. Game Shard Changes (Rust)

**File:** `services/game_shard_rust/src/shard.rs`

Changes:
- Subscribe to ZMQ endpoints directly (already implemented)
- Remove Redis subscription for price data
- Keep Redis for non-latency-critical operations (game assignments, metadata)

```rust
// Current: Checks transport mode
if transport_mode.use_zmq() { /* zmq path */ }
if transport_mode.use_redis() { /* redis path */ }

// Target: ZMQ is always primary
// Remove redis subscription for prices entirely
// zmq_listener handles Redis persistence
```

### 4. Signal Processor Changes (Rust)

**File:** `services/signal_processor_rust/src/main.rs`

Changes:
- Remove Redis pub/sub for signal consumption
- ZMQ SUB from game_shard:5558 becomes mandatory
- ZMQ PUB to :5559 becomes mandatory

### 5. Execution Service Changes (Rust)

**File:** `services/execution_service_rust/src/main.rs`

Changes:
- ZMQ SUB from signal_processor:5559 becomes mandatory
- Remove Redis subscription fallback

### 6. ZMQ Listener Transformation

**File:** `services/zmq_listener_rust/src/main.rs`

Transform from "bridge" to "observer" role:

```rust
// NEW: Observer mode configuration
enum ListenerMode {
    Observer,   // Subscribe and log/persist only (NEW default)
    Bridge,     // Subscribe and forward to Redis pub/sub (legacy)
    Disabled,   // Exit immediately
}

// NEW: Multi-output logging
struct ObserverConfig {
    console_log: bool,        // Log to stdout
    redis_streams: bool,      // Write to Redis streams (for queries)
    file_log: Option<PathBuf>, // Optional file output
    log_level: LogLevel,      // verbose | info | errors_only
}
```

## Detailed zmq_listener Observer Implementation

### New Capabilities

1. **Structured Console Logging**
   ```
   [2024-01-15T10:30:45.123Z] PRICE kalshi KXNBA-DET-NYK seq=12345 bid=0.45 ask=0.47 liq=$5000
   [2024-01-15T10:30:45.125Z] PRICE poly 0x123... seq=8901 bid=0.44 ask=0.46 liq=$8000
   [2024-01-15T10:30:45.200Z] SIGNAL game=12345 edge=2.3% kalshi=0.45 poly=0.44
   [2024-01-15T10:30:45.205Z] EXEC signal=abc123 status=submitted
   ```

2. **Redis Streams for Historical Queries**
   - `stream:prices:kalshi` - All Kalshi price updates
   - `stream:prices:polymarket` - All Polymarket price updates
   - `stream:signals` - All generated signals
   - `stream:executions` - All execution events
   - Configurable retention (MAXLEN)

3. **Optional File Logging**
   - JSONL format for replay capability
   - Rotated by size or time
   - Useful for backtesting

4. **Latency Tracking**
   - Measure zmq_listener receipt time vs message timestamp
   - Track pipeline latency end-to-end

### Message Types to Observe

| Topic Prefix | Source | Content |
|--------------|--------|---------|
| `prices.kalshi.*` | kalshi_monitor:5555 | Market prices |
| `prices.poly.*` | polymarket_monitor:5556 | Market prices |
| `signals.trade.*` | game_shard:5558 | Arbitrage signals |
| `execution.*` | signal_processor:5559 | Execution requests |

### Additional Subscription: Execution Service Output

Add new ZMQ PUB to execution_service for trade results:

```rust
// execution_service_rust additions
// PUB :5560 for trade results
// Topics: trades.executed.*, trades.failed.*, trades.cancelled.*
```

zmq_listener subscribes to this for complete pipeline visibility.

## Implementation Order

### Phase 1: Enhanced zmq_listener (Observer Mode) - COMPLETED
1. [x] Add `ZMQ_LISTENER_MODE` environment variable
2. [x] Implement observer mode with console logging
3. [x] Add Redis streams output (separate from pub/sub)
4. [x] Add signal_processor subscription (port 5559)
5. [x] Add execution_service subscription (port 5560)
6. [x] Add latency tracking
7. [x] Add sequence gap detection
8. [x] Color-coded console output by message type
9. [x] Update docker-compose.yml with new config
10. [x] Update .env.example with documentation

### Phase 2: Monitor ZMQ-First - COMPLETED
1. [x] Make ZMQ mandatory in monitors (pyzmq is now required)
2. [x] ZMQ is primary hot path, Redis publish is secondary/optional
3. [x] Both kalshi_monitor and polymarket_monitor updated

### Phase 3: Rust Services ZMQ-First - COMPLETED
1. [x] Update game_shard to ZMQ-only default
2. [x] Update signal_processor to ZMQ-only default
3. [x] Update execution_service to ZMQ-only default
4. [x] Add trade result publishing (PUB :5560) to execution_service

### Phase 4: Cleanup - COMPLETED
1. [x] Changed default ZMQ_TRANSPORT_MODE from redis_only to zmq_only
2. [x] Redis publish is now secondary (controlled by _redis_publish_prices flag)
3. [x] Updated docker-compose defaults for all services

## Port Allocation

| Port | Service | Direction | Topics |
|------|---------|-----------|--------|
| 5555 | kalshi_monitor | PUB | prices.kalshi.* |
| 5556 | polymarket_monitor | PUB | prices.poly.* |
| 5558 | game_shard | PUB | signals.trade.*, games.* |
| 5559 | signal_processor | PUB | execution.* |
| 5560 | execution_service | PUB | trades.* (NEW) |

## Redis Streams Schema (for slow-path consumers)

```
stream:prices:kalshi
  - id: auto
  - topic: prices.kalshi.KXNBA-DET-NYK
  - payload: {json}
  - zmq_seq: 12345
  - zmq_ts: 1705312245123
  - recv_ts: 1705312245125

stream:signals
  - id: auto
  - topic: signals.trade.uuid
  - payload: {json}
  - zmq_seq: 100
  - zmq_ts: 1705312245200
  - recv_ts: 1705312245201

stream:trades
  - id: auto
  - topic: trades.executed.uuid
  - payload: {json}
  - zmq_seq: 50
  - zmq_ts: 1705312245300
  - recv_ts: 1705312245301
```

## Benefits

1. **Lower Latency**: ZMQ direct paths eliminate Redis round-trips (~1-5ms savings per hop)
2. **Better Observability**: zmq_listener sees ALL traffic in one place
3. **Replay Capability**: Redis streams + optional file logs enable backtesting
4. **Simpler Hot Path**: Services don't need dual transport logic
5. **Decoupled Persistence**: Slow-path consumers don't affect hot-path latency

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| ZMQ connection failures | Automatic reconnection with backoff (already implemented) |
| Message loss | zmq_listener tracks sequence gaps, alerts on drops |
| Observer falls behind | Separate slow-path; doesn't affect hot-path |
| Debugging without Redis | Console logs + Redis streams provide visibility |

## Querying Redis Streams (Slow-Time Uses)

The zmq_listener populates Redis streams that can be queried for debugging, analytics, and historical analysis.

### Stream Keys

| Stream Key | Content | Max Size |
|------------|---------|----------|
| `stream:prices:kalshi` | Kalshi price updates | 50,000 |
| `stream:prices:polymarket` | Polymarket price updates | 50,000 |
| `stream:signals` | Arbitrage signals | 5,000 |
| `stream:executions` | Execution requests | 5,000 |
| `stream:trades` | Trade results | 5,000 |
| `stream:games` | Game state changes | 5,000 |

### Example Queries

```bash
# Get last 10 price updates from Kalshi
redis-cli XREVRANGE stream:prices:kalshi + - COUNT 10

# Get all signals in the last 5 minutes
redis-cli XRANGE stream:signals $(date -d '5 minutes ago' +%s)000-0 +

# Get prices for a specific game
redis-cli XREAD STREAMS stream:prices:kalshi 0 | grep "game_id.*12345"

# Monitor streams in real-time
redis-cli XREAD BLOCK 0 STREAMS stream:signals $

# Get stream info (length, first/last entry)
redis-cli XINFO STREAM stream:prices:kalshi
```

### Stream Entry Fields

Each entry contains:
- `topic` - Original ZMQ topic (e.g., `prices.kalshi.KXNBA-DET-NYK`)
- `payload` - JSON message payload
- `zmq_seq` - Sequence number from publisher
- `zmq_ts` - Timestamp when message was sent (ms)
- `recv_ts` - Timestamp when zmq_listener received it (ms)
- `source` - Source service name

### Latency Analysis

```bash
# Calculate average latency (recv_ts - zmq_ts) for recent messages
redis-cli XREVRANGE stream:prices:kalshi + - COUNT 100 | \
  grep -E "(zmq_ts|recv_ts)" | \
  # ... process to calculate latency
```

### Dashboard Integration

The frontend can read from streams instead of pub/sub for non-real-time displays:

```typescript
// Example: Fetch recent signals for dashboard
const signals = await redis.xrevrange('stream:signals', '+', '-', 'COUNT', 50);
```

## Success Metrics

- Hot-path latency reduced by 5-15ms (measure P50, P99)
- Zero message loss in observer (track sequence gaps)
- All services boot and communicate via ZMQ
- Dashboard/frontend continues working (reads Redis streams)
