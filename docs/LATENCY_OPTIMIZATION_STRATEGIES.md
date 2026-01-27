# Latency Optimization Strategies for Arbees

**Current State**: ~160-200ms end-to-end (60ms Redis overhead)
**Target**: <100-120ms end-to-end (reduce overhead to <20ms)
**Date**: 2026-01-27

---

## Current Bottleneck Analysis

### Latency Breakdown

```
ESPN Update → orchestrator → market_discovery → game_shard → signal_processor → execution_service
              (~20ms Redis)   (~20ms Redis)      (~20ms Redis)   (~20ms Redis)     (~100ms API)

Total Pipeline: ~160-200ms
  - Redis overhead: ~60ms (3-4 hops × 15-20ms each)
  - Processing: ~40-60ms (model calculations, arbitrage detection)
  - API calls: ~100ms (Kalshi order placement)
```

### Why Redis is Slow

1. **Network round-trip**: Even localhost TCP has ~0.5-1ms base latency
2. **Serialization**: JSON encoding/decoding adds ~1-5ms
3. **Pub/Sub overhead**: Broker adds ~10-15ms per hop
4. **Multiple hops**: 3-4 services in chain compounds latency

---

## Solution 1: ZeroMQ (ZMQ) for Hot Path 🚀

### Overview

Replace Redis pub/sub with ZeroMQ for latency-critical messages.

**Architecture**:
```
┌─────────────────────────────────────────────────────────────┐
│                     HOT PATH (ZMQ)                          │
│  kalshi_monitor ──zmq pub──> game_shard ──zmq push──> execution │
│      (~0.5ms)                    (~0.5ms)                    │
│                                                               │
│  Total: ~1-2ms (vs 60ms with Redis)                         │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                   COLD PATH (Redis)                          │
│  orchestrator ──redis──> market_discovery                   │
│  (game discovery, market IDs, health checks, state)         │
└─────────────────────────────────────────────────────────────┘
```

### Latency Improvement

| Path | Current (Redis) | With ZMQ | Savings |
|------|----------------|----------|---------|
| Price update → game_shard | 20ms | 0.5ms | **19.5ms** |
| game_shard → signal_processor | 20ms | 0.5ms | **19.5ms** |
| signal_processor → execution | 20ms | 0.5ms | **19.5ms** |
| **Total hot path** | **60ms** | **~2ms** | **~58ms** |
| **End-to-end** | 160-200ms | 100-120ms | **~60ms** |

### Implementation

**Rust ZMQ Library**: `zeromq` or `zmq`

```rust
// Publisher (kalshi_monitor, game_shard)
use zmq::{Context, Socket, PUB};

let context = Context::new();
let publisher = context.socket(PUB)?;
publisher.bind("tcp://*:5555")?;

// Send price update
let update = serde_json::to_vec(&price_update)?;
publisher.send(&update, 0)?;

// Subscriber (game_shard, execution_service)
let subscriber = context.socket(SUB)?;
subscriber.connect("tcp://kalshi_monitor:5555")?;
subscriber.set_subscribe(b"")?;  // Subscribe to all

loop {
    let msg = subscriber.recv_bytes(0)?;
    let update: PriceUpdate = serde_json::from_slice(&msg)?;
    // Process...
}
```

### Pros
- ✅ **Very low latency**: <1ms per hop (vs 20ms Redis)
- ✅ **High throughput**: Millions of messages/second
- ✅ **Multiple patterns**: PUB-SUB, PUSH-PULL, REQ-REP
- ✅ **No broker overhead**: Direct TCP between services
- ✅ **Battle-tested**: Used in high-frequency trading

### Cons
- ❌ **No persistence**: Messages lost if service down
- ❌ **No message replay**: Can't replay history
- ❌ **Service discovery harder**: Need to know endpoints
- ❌ **Two messaging systems**: ZMQ + Redis = complexity

### Hybrid Approach (RECOMMENDED)

**Use ZMQ for hot path, Redis for cold path**:

```
Hot Path (ZMQ - sub-millisecond):
  - Price updates (kalshi_monitor → game_shard)
  - Game state updates (orchestrator → game_shard)
  - Trading signals (game_shard → execution_service)

Cold Path (Redis - acceptable latency):
  - Game discovery (orchestrator → market_discovery)
  - Market ID lookup (Redis cache with TTL)
  - Health checks (heartbeats every 10s)
  - Position tracking (position_tracker → database)
  - Configuration changes
```

**Complexity**: Moderate (two systems, but clear separation)

---

## Solution 2: NATS with JetStream 🌊

### Overview

Replace Redis with NATS for messaging. NATS has sub-2ms latency with persistence.

**Architecture**:
```
┌─────────────────────────────────────────────────────────────┐
│                      NATS Core                              │
│  kalshi_monitor ──nats pub──> game_shard ──nats pub──> execution │
│      (~1-2ms)                      (~1-2ms)                  │
│                                                               │
│  JetStream (optional): Message persistence + replay         │
└─────────────────────────────────────────────────────────────┘
```

### Latency Improvement

| Path | Current (Redis) | With NATS | Savings |
|------|----------------|-----------|---------|
| Total hot path | 60ms | ~6-8ms | **~52ms** |
| End-to-end | 160-200ms | 110-140ms | **~40ms** |

### Implementation

**Rust NATS Library**: `async-nats`

```rust
use async_nats;

// Publisher
let client = async_nats::connect("nats://localhost:4222").await?;
let data = serde_json::to_vec(&price_update)?;
client.publish("prices.kalshi", data.into()).await?;

// Subscriber
let mut subscriber = client.subscribe("prices.kalshi").await?;
while let Some(msg) = subscriber.next().await {
    let update: PriceUpdate = serde_json::from_slice(&msg.payload)?;
    // Process...
}
```

### Pros
- ✅ **Low latency**: 1-2ms per hop
- ✅ **Persistence available**: JetStream for message replay
- ✅ **Simpler than ZMQ**: Single system for all messaging
- ✅ **Service discovery**: Built-in
- ✅ **Mature ecosystem**: Used in production by many companies

### Cons
- ❌ **New service**: Another container to run (nats-server)
- ❌ **Slightly slower than ZMQ**: 1-2ms vs <1ms
- ❌ **Learning curve**: Team needs to learn NATS

**Verdict**: Good middle ground between Redis and ZMQ.

---

## Solution 3: gRPC Streaming 📡

### Overview

Replace Redis pub/sub with gRPC bidirectional streaming.

**Architecture**:
```
┌─────────────────────────────────────────────────────────────┐
│              gRPC Streaming (HTTP/2)                        │
│  kalshi_monitor ──stream──> game_shard ──stream──> execution │
│      (~5-10ms)                   (~5-10ms)                   │
└─────────────────────────────────────────────────────────────┘
```

### Latency Improvement

| Path | Current (Redis) | With gRPC | Savings |
|------|----------------|-----------|---------|
| Total hot path | 60ms | ~20-30ms | **~30ms** |
| End-to-end | 160-200ms | 130-160ms | **~20ms** |

### Implementation

**Rust gRPC Library**: `tonic`

```rust
// Define proto
message PriceUpdate {
    string ticker = 1;
    double yes_ask = 2;
    double yes_bid = 3;
    int64 timestamp_ms = 4;
}

service PriceStream {
    rpc StreamPrices(Empty) returns (stream PriceUpdate);
}

// Server (kalshi_monitor)
impl PriceStream for PriceService {
    type StreamPricesStream = ReceiverStream<Result<PriceUpdate, Status>>;

    async fn stream_prices(&self, _: Request<Empty>)
        -> Result<Response<Self::StreamPricesStream>, Status>
    {
        let (tx, rx) = mpsc::channel(100);
        // Send price updates to tx
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// Client (game_shard)
let mut client = PriceStreamClient::connect("http://kalshi_monitor:50051").await?;
let mut stream = client.stream_prices(Request::new(Empty {})).await?.into_inner();

while let Some(update) = stream.message().await? {
    // Process price update
}
```

### Pros
- ✅ **Type safety**: Protobuf definitions
- ✅ **Bidirectional streaming**: Can push and pull
- ✅ **Good latency**: 5-10ms per hop (better than Redis)
- ✅ **HTTP/2**: Multiplexing, flow control
- ✅ **Tooling**: Great ecosystem (grpcurl, grpc-web)

### Cons
- ❌ **Slower than ZMQ/NATS**: Still 5-10ms per hop
- ❌ **Connection management**: Need to handle reconnects
- ❌ **Protobuf overhead**: Schema management, code generation

**Verdict**: Good for API boundaries, not optimal for ultra-low-latency hot path.

---

## Solution 4: Shared Memory (IPC) 💾

### Overview

Use shared memory for services on same host (Linux/Unix only).

**Architecture**:
```
┌─────────────────────────────────────────────────────────────┐
│              Same Host - Shared Memory                       │
│  kalshi_monitor ──shm──> game_shard ──shm──> execution      │
│      (~0.01ms)                (~0.01ms)                      │
│                                                               │
│  Total: <0.1ms (NANOSECOND latency!)                        │
└─────────────────────────────────────────────────────────────┘
```

### Latency Improvement

| Path | Current (Redis) | With Shared Memory | Savings |
|------|----------------|-------------------|---------|
| Total hot path | 60ms | **~0.1ms** | **~60ms** |
| End-to-end | 160-200ms | **100-140ms** | **~60ms** |

### Implementation

**Rust Library**: `shared_memory`

```rust
use shared_memory::{Shmem, ShmemConf};

// Writer (kalshi_monitor)
let mut shmem = ShmemConf::new()
    .size(4096)
    .flink("/arbees_prices")
    .create()?;

let data = serde_json::to_vec(&price_update)?;
unsafe {
    let ptr = shmem.as_ptr() as *mut u8;
    ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
}

// Reader (game_shard)
let shmem = ShmemConf::new()
    .flink("/arbees_prices")
    .open()?;

let data = unsafe {
    slice::from_raw_parts(shmem.as_ptr(), shmem.len())
};
let update: PriceUpdate = serde_json::from_slice(data)?;
```

### Pros
- ✅ **Fastest possible**: Nanosecond latency
- ✅ **Zero copy**: Direct memory access
- ✅ **No serialization overhead**: Raw bytes

### Cons
- ❌ **Single machine only**: Doesn't scale across hosts
- ❌ **Complex synchronization**: Need mutexes/semaphores
- ❌ **No Docker isolation**: Breaks container boundaries
- ❌ **Platform-specific**: Linux/Unix only

**Verdict**: Extreme performance, but kills microservices benefits. Not recommended.

---

## Solution 5: Unix Domain Sockets 🔌

### Overview

Use Unix domain sockets instead of TCP for same-host communication.

**Architecture**:
```
┌─────────────────────────────────────────────────────────────┐
│         Same Host - Unix Domain Sockets                     │
│  kalshi_monitor ──uds──> game_shard ──uds──> execution      │
│      (~0.1-0.5ms)             (~0.1-0.5ms)                   │
└─────────────────────────────────────────────────────────────┘
```

### Latency Improvement

| Path | Current (Redis) | With UDS | Savings |
|------|----------------|----------|---------|
| Total hot path | 60ms | ~2-3ms | **~57ms** |
| End-to-end | 160-200ms | 100-130ms | **~50ms** |

### Implementation

Works with ZMQ, gRPC, or raw sockets:

```rust
// ZMQ over Unix domain socket
publisher.bind("ipc:///tmp/arbees_prices.ipc")?;
subscriber.connect("ipc:///tmp/arbees_prices.ipc")?;

// gRPC over Unix domain socket
let server = Server::builder()
    .add_service(PriceStreamServer::new(svc))
    .serve_with_incoming(UnixListener::bind("/tmp/arbees.sock")?)
    .await?;
```

### Pros
- ✅ **Very fast**: 0.1-0.5ms (faster than TCP)
- ✅ **Works with existing protocols**: ZMQ, gRPC, etc.
- ✅ **Simple**: Drop-in replacement for TCP sockets
- ✅ **Docker compatible**: Mount /tmp volume

### Cons
- ❌ **Single machine only**: Doesn't scale across hosts
- ❌ **File system dependency**: Need shared volume

**Verdict**: Good optimization if all services on same host (likely in your case).

---

## Solution 6: Optimize Current Redis Setup 🔧

### Overview

Stay with Redis but optimize configuration and usage patterns.

### Optimizations

#### A. Use Redis Streams Instead of Pub/Sub

**Why**: Streams have lower latency and better delivery guarantees.

```rust
// Publisher (XADD)
redis::cmd("XADD")
    .arg("prices:kalshi")
    .arg("*")  // Auto-generate ID
    .arg(&["ticker", ticker, "yes_ask", yes_ask.to_string()])
    .query_async(&mut con)
    .await?;

// Consumer (XREAD)
let items: StreamReadReply = redis::cmd("XREAD")
    .arg("BLOCK")
    .arg(100)  // 100ms timeout
    .arg("STREAMS")
    .arg("prices:kalshi")
    .arg("$")  // Read from latest
    .query_async(&mut con)
    .await?;
```

**Latency**: ~12-15ms per hop (vs 20ms with pub/sub)

#### B. Redis Pipeline/Batch

Batch multiple operations to reduce round-trips:

```rust
let pipe = redis::pipe();
pipe.cmd("XADD").arg("prices:kalshi").arg(price1)
    .cmd("XADD").arg("prices:kalshi").arg(price2)
    .cmd("XADD").arg("prices:kalshi").arg(price3);

pipe.query_async(&mut con).await?;
```

**Latency**: ~8-10ms for 3 operations (vs 60ms with 3 separate calls)

#### C. Co-locate Services on Same Host

Run all services on same physical machine to minimize network latency:

```yaml
# docker-compose.yml
services:
  # All services share same host network
  orchestrator:
    network_mode: "host"
  game_shard:
    network_mode: "host"
  execution_service:
    network_mode: "host"
  redis:
    network_mode: "host"
```

**Latency**: ~10-12ms per hop (vs 20ms across network)

#### D. Use Redis on Unix Socket

```bash
# redis.conf
unixsocket /tmp/redis.sock
unixsocketperm 777
```

```rust
let client = redis::Client::open("unix:///tmp/redis.sock")?;
```

**Latency**: ~5-8ms per hop (vs 20ms on TCP)

### Total Savings with All Optimizations

| Optimization | Latency per hop | Total (3 hops) |
|--------------|----------------|----------------|
| Current (Redis TCP pub/sub) | 20ms | 60ms |
| Redis Streams | 15ms | 45ms |
| Co-located (host network) | 12ms | 36ms |
| Unix socket | 8ms | 24ms |
| **All combined** | **~5-8ms** | **~15-24ms** |

**End-to-end improvement**: 160-200ms → 120-140ms (~30-40ms savings)

---

## Recommendation Matrix

| Solution | Latency Gain | Complexity | Scalability | Recommendation |
|----------|--------------|------------|-------------|----------------|
| **ZMQ + Redis hybrid** | ⭐⭐⭐⭐⭐ (~58ms) | Medium | Good | ✅ **BEST** for <100ms target |
| **NATS + JetStream** | ⭐⭐⭐⭐ (~40ms) | Low | Excellent | ✅ Good alternative |
| **Redis optimized** | ⭐⭐⭐ (~30ms) | Very Low | Good | ✅ **EASIEST** quick win |
| **gRPC streaming** | ⭐⭐ (~20ms) | Medium | Good | ⚠️ Not worth the effort |
| **Unix domain sockets** | ⭐⭐⭐⭐ (~50ms) | Low | Poor | ⚠️ Only if single host |
| **Shared memory** | ⭐⭐⭐⭐⭐ (~60ms) | High | Very Poor | ❌ Breaks architecture |

---

## Phased Implementation Plan

### Phase 1: Quick Wins (Week 1) ✅

**Goal**: Reduce latency to ~120-140ms with minimal risk

**Steps**:
1. Switch Redis pub/sub → Redis Streams (~5ms savings per hop)
2. Enable Redis Unix socket (`/tmp/redis.sock`)
3. Use `network_mode: host` in docker-compose (if acceptable)

**Expected Outcome**: 160-200ms → 120-140ms (~30-40ms savings)

**Risk**: Low (configuration changes only)

---

### Phase 2: ZMQ Hot Path (Week 2-3) 🚀

**Goal**: Reduce latency to <100ms with ZMQ

**Steps**:
1. Add ZMQ to Rust services (`zeromq` or `zmq` crate)
2. Replace hot path only:
   - `kalshi_monitor` → `game_shard` (ZMQ PUB-SUB)
   - `game_shard` → `execution_service` (ZMQ PUSH-PULL)
3. Keep Redis for:
   - Game discovery
   - Market ID cache
   - Health checks
   - Position tracking

**Implementation**:

```rust
// services/game_shard_rust/src/main.rs

// ZMQ subscriber for prices
let zmq_context = zmq::Context::new();
let zmq_sub = zmq_context.socket(zmq::SUB)?;
zmq_sub.connect("tcp://kalshi_monitor:5555")?;
zmq_sub.set_subscribe(b"prices")?;

// ZMQ publisher for signals
let zmq_pub = zmq_context.socket(zmq::PUB)?;
zmq_pub.bind("tcp://*:5556")?;

loop {
    // Receive price from ZMQ
    let msg = zmq_sub.recv_bytes(0)?;
    let price: PriceUpdate = serde_json::from_slice(&msg)?;

    // Process and generate signal
    let signal = generate_signal(&price)?;

    // Publish signal via ZMQ
    let signal_bytes = serde_json::to_vec(&signal)?;
    zmq_pub.send(&signal_bytes, 0)?;
}
```

**Expected Outcome**: 160-200ms → 100-120ms (~60ms savings)

**Risk**: Medium (new dependency, dual messaging systems)

---

### Phase 3: NATS Migration (Alternative to Phase 2)

**Goal**: Single messaging system with low latency

**Steps**:
1. Deploy NATS server (`nats-server` container)
2. Migrate all Redis pub/sub to NATS
3. Use JetStream for messages needing persistence

**docker-compose.yml**:
```yaml
nats:
  image: nats:latest
  container_name: arbees-nats
  command: ["-js"]  # Enable JetStream
  ports:
    - "4222:4222"
```

**Expected Outcome**: 160-200ms → 110-130ms (~40-50ms savings)

**Risk**: Medium (new service, full migration)

---

## Benchmarking Plan

Before committing to a solution, benchmark each option:

### Test Setup

```rust
// benchmark/latency_test.rs

#[tokio::test]
async fn benchmark_redis_pubsub() {
    let start = Instant::now();
    for _ in 0..1000 {
        redis_pub.publish("test", "data").await?;
        let msg = redis_sub.recv().await?;
    }
    let avg = start.elapsed() / 1000;
    println!("Redis pub/sub avg: {:?}", avg);
}

#[tokio::test]
async fn benchmark_zmq() {
    let start = Instant::now();
    for _ in 0..1000 {
        zmq_pub.send(b"data", 0)?;
        let msg = zmq_sub.recv_bytes(0)?;
    }
    let avg = start.elapsed() / 1000;
    println!("ZMQ avg: {:?}", avg);
}

#[tokio::test]
async fn benchmark_nats() {
    let start = Instant::now();
    for _ in 0..1000 {
        nats.publish("test", "data").await?;
        let msg = nats_sub.next().await?;
    }
    let avg = start.elapsed() / 1000;
    println!("NATS avg: {:?}", avg);
}
```

### Expected Results (localhost)

| System | Latency (avg) | Throughput |
|--------|---------------|------------|
| Redis pub/sub (TCP) | 15-20ms | 50K msg/s |
| Redis Streams (TCP) | 12-15ms | 80K msg/s |
| Redis (Unix socket) | 5-8ms | 150K msg/s |
| ZMQ (TCP) | 0.5-1ms | 1M+ msg/s |
| ZMQ (IPC/UDS) | 0.1-0.3ms | 5M+ msg/s |
| NATS (TCP) | 1-2ms | 500K msg/s |
| gRPC streaming | 5-10ms | 100K msg/s |

---

## Final Recommendation

### For Your Use Case (Arbees)

**Phase 1 (Do This Now)**: ✅ **Redis Optimizations**
- Lowest risk, immediate ~30ms savings
- No new dependencies
- Start here before testing complete

**Phase 2 (If <100ms Required)**: 🚀 **ZMQ Hot Path + Redis Cold Path**
- Best latency (~60ms savings)
- Proven in HFT systems
- Keeps microservices benefits
- Clear separation: fast path vs slow path

**Alternative**: 🌊 **NATS Migration**
- If you want to simplify to one system
- Good balance of latency and features
- Easier than ZMQ + Redis hybrid

---

## Trade-off Summary

```
┌────────────────────────────────────────────────────────────────┐
│  Latency vs Complexity vs Scalability                          │
│                                                                  │
│  Shared Memory ────────────────────────── FASTEST (0.1ms)      │
│      │                                      ↑                   │
│      │                                      │ COMPLEXITY        │
│  ZMQ (IPC) ──────────────────────────────  │                   │
│      │                                      │                   │
│  ZMQ (TCP) ──────────────────────────────  │                   │
│      │                                      │                   │
│  NATS ───────────────────────────────────  │                   │
│      │                                      │                   │
│  Redis (optimized) ───────────────────────  │                   │
│      │                                      │                   │
│  Redis (current) ───────────────────────── SLOWEST (20ms/hop)  │
│                                                                  │
│  SCALABILITY: Redis/NATS > ZMQ > Shared Memory                 │
└────────────────────────────────────────────────────────────────┘
```

**Sweet Spot**: ZMQ for hot path + Redis for everything else

---

**Next Action**: Run Phase 1 (Redis optimizations) during 48-hour test to measure actual impact.

**Document Status**: ✅ Ready for Discussion
**Last Updated**: 2026-01-27
