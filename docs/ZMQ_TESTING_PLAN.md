# ZMQ Implementation Testing & Validation Plan

**Current State**: ZMQ hot path implemented with dual-mode support (Redis + ZMQ)
**Goal**: Validate ZMQ works, measure latency improvements, gradually phase out Redis
**Date**: 2026-01-27

---

## Current Implementation Status ✅

Based on docker-compose.yml, you have:

### Implemented Services
- ✅ **zmq_listener** - Bridge service (ZMQ → Redis)
- ✅ **kalshi_monitor** - ZMQ PUB on port 5555
- ✅ **polymarket_monitor** - ZMQ PUB on port 5556 (via VPN)
- ✅ **game_shard** - ZMQ SUB (monitors) + PUB (signals) on port 5558
- ✅ **execution_service** - ZMQ SUB (signals from game_shard)

### Configuration
All services have dual-mode support via `ZMQ_ENABLED` environment variable:
```yaml
ZMQ_ENABLED: "${ZMQ_ENABLED:-false}"  # Defaults to false (Redis mode)
```

---

## Testing Strategy

### Phase 1: Validate ZMQ Infrastructure (Days 1-2)

**Goal**: Verify ZMQ messages flow correctly without enabling production use

#### Step 1: Enable ZMQ Listener Only

```bash
# .env
ZMQ_ENABLED=true  # Enable ZMQ globally

# Start infrastructure
docker-compose up -d timescaledb redis

# Start zmq_listener only (no other services yet)
docker-compose up -d zmq_listener
```

**Expected**: zmq_listener should start and wait for ZMQ publishers

#### Step 2: Start Monitors (Publishers)

```bash
# Start monitors with ZMQ enabled
docker-compose up -d kalshi_monitor polymarket_monitor vpn
```

**Validate ZMQ Publishing**:
```bash
# Check kalshi_monitor is publishing to ZMQ port 5555
docker logs kalshi_monitor | grep -i zmq
# Expected: "ZMQ publisher initialized on port 5555" or similar

# Check polymarket_monitor is publishing to ZMQ port 5556
docker logs polymarket_monitor | grep -i zmq
# Expected: "ZMQ publisher initialized on port 5556" or similar
```

#### Step 3: Validate zmq_listener is Receiving

```bash
# Check zmq_listener is receiving and mirroring to Redis
docker logs zmq_listener -f | grep -i "mirrored"

# Expected output:
# "Mirrored prices.kalshi.KXNBAGAME-2024-01-15-MIA-v-BOS to prices:kalshi"
# "Mirrored prices.poly.0x123abc to prices:poly"
```

**Verify Redis is Populated**:
```bash
# Check Redis streams have messages
docker exec arbees-redis redis-cli XLEN prices:kalshi
# Expected: > 0 (messages present)

docker exec arbees-redis redis-cli XLEN prices:poly
# Expected: > 0 (messages present)

# Check latest message
docker exec arbees-redis redis-cli XREVRANGE prices:kalshi + - COUNT 1
# Expected: JSON data with ticker, yes_ask, yes_bid, etc.
```

**Success Criteria**:
- ✅ zmq_listener logs show "Mirrored..." messages
- ✅ Redis streams (prices:kalshi, prices:poly) have data
- ✅ No error logs in zmq_listener, kalshi_monitor, polymarket_monitor

---

### Phase 2: Enable game_shard with ZMQ (Days 3-4)

**Goal**: Validate game_shard can subscribe to ZMQ prices and publish signals

#### Step 1: Start game_shard (ZMQ Mode)

```bash
# game_shard should already have ZMQ_ENABLED=true from .env
docker-compose up -d orchestrator market-discovery-rust game_shard
```

**Validate game_shard ZMQ Subscription**:
```bash
# Check game_shard is subscribed to monitors
docker logs game_shard | grep -i zmq
# Expected: "ZMQ subscriber connected to tcp://kalshi_monitor:5555"
#           "ZMQ subscriber connected to tcp://polymarket_monitor:5556"

# Check game_shard is receiving prices
docker logs game_shard | grep -i "price update"
# Expected: Logs showing price updates with sub-millisecond timestamps
```

**Validate game_shard Signal Publishing**:
```bash
# Check game_shard is publishing signals to ZMQ
docker logs game_shard | grep -i "signal.*zmq"
# Expected: "Published signal to ZMQ: signals.trade.arb..."

# Verify zmq_listener sees signals
docker logs zmq_listener | grep "signals.trade"
# Expected: "Mirrored signals.trade.arb... to signals:trades"
```

**Verify Redis Streams**:
```bash
# Check signals stream has messages
docker exec arbees-redis redis-cli XLEN signals:trades
# Expected: > 0 (if any signals generated)
```

**Success Criteria**:
- ✅ game_shard logs show ZMQ subscription successful
- ✅ game_shard receives price updates (verify ticker names match)
- ✅ game_shard publishes signals to ZMQ
- ✅ zmq_listener mirrors signals to Redis

---

### Phase 3: Enable execution_service with ZMQ (Days 5-6)

**Goal**: Validate execution_service can subscribe to ZMQ signals and execute trades

#### Step 1: Start execution_service (ZMQ Mode)

```bash
# execution_service should already have ZMQ_ENABLED=true
docker-compose up -d execution_service
```

**Validate execution_service ZMQ Subscription**:
```bash
# Check subscription
docker logs execution_service | grep -i zmq
# Expected: "ZMQ subscriber connected to tcp://game_shard:5558"

# Check signal reception
docker logs execution_service | grep -i "received signal"
# Expected: "Received signal via ZMQ: arb..."
```

**Validate Trade Execution**:
```bash
# Check IOC orders are placed
docker logs execution_service | grep -i "IOC order"
# Expected: "Placing IOC order (arb...): buy yes x10 @ 45c on KXNBAGAME-..."
#           "IOC order arb... placed: ... (filled: 10/10)"

# Check database
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT order_id, status, filled_qty, latency_ms
   FROM paper_trades
   ORDER BY created_at DESC
   LIMIT 10;"

# Expected: Recent trades with latency_ms < 120ms (vs ~180ms with Redis)
```

**Success Criteria**:
- ✅ execution_service receives signals from ZMQ
- ✅ Orders are executed with IOC
- ✅ Latency is lower than Redis mode (measure below)

---

### Phase 4: Latency Measurement (Days 5-6)

**Goal**: Measure actual latency improvement vs Redis

#### Instrumentation Points

Add timestamps at each hop:

```rust
// In kalshi_monitor (ZMQ publish)
let msg = PriceUpdate {
    ticker,
    yes_ask,
    yes_bid,
    timestamp_ms: now_millis(),
    zmq_pub_ts: now_millis(),  // ADD THIS
};

// In game_shard (ZMQ receive + signal generate)
let recv_ts = now_millis();
info!("Price latency: {}ms", recv_ts - msg.zmq_pub_ts);

let signal = generate_signal(&game_state)?;
let signal_ts = now_millis();

let signal_msg = TradingSignal {
    ...
    zmq_pub_ts: signal_ts,  // ADD THIS
};

// In execution_service (ZMQ receive + execute)
let recv_ts = now_millis();
info!("Signal latency: {}ms", recv_ts - signal.zmq_pub_ts);

let order = place_ioc_order(...).await?;
let exec_ts = now_millis();
info!("Execution latency: {}ms", exec_ts - recv_ts);
info!("End-to-end latency: {}ms", exec_ts - signal.zmq_pub_ts);
```

#### Collect Metrics

```bash
# Run for 1 hour, collect latency stats
docker logs game_shard | grep "Price latency" | \
  awk '{print $NF}' | sed 's/ms//' | \
  awk '{sum+=$1; sumsq+=$1*$1; count++}
       END {print "Avg:", sum/count, "ms  Stddev:", sqrt(sumsq/count - (sum/count)^2), "ms"}'

# Expected: Avg ~0.5-2ms (vs 15-20ms with Redis)

docker logs execution_service | grep "Signal latency" | \
  awk '{print $NF}' | sed 's/ms//' | \
  awk '{sum+=$1; sumsq+=$1*$1; count++}
       END {print "Avg:", sum/count, "ms  Stddev:", sqrt(sumsq/count - (sum/count)^2), "ms"}'

# Expected: Avg ~0.5-2ms (vs 15-20ms with Redis)

docker logs execution_service | grep "End-to-end latency" | \
  awk '{print $NF}' | sed 's/ms//' | \
  awk '{sum+=$1; sumsq+=$1*$1; count++}
       END {print "Avg:", sum/count, "ms  Stddev:", sqrt(sumsq/count - (sum/count)^2), "ms"}'

# Expected: Avg ~100-120ms (vs 160-200ms with Redis)
```

#### Compare: ZMQ vs Redis Mode

```bash
# Test 1: ZMQ mode (current)
ZMQ_ENABLED=true docker-compose --profile full up -d
# Let run for 1 hour
# Collect latency metrics (above)

# Test 2: Redis mode (fallback)
ZMQ_ENABLED=false docker-compose --profile full restart
# Let run for 1 hour
# Collect latency metrics (above)

# Compare results
```

**Success Criteria**:
- ✅ ZMQ mode: End-to-end latency ~100-120ms (p50)
- ✅ Redis mode: End-to-end latency ~160-200ms (p50)
- ✅ Improvement: ~40-80ms (25-40% faster)

---

### Phase 5: Stability Test (Days 7-14)

**Goal**: Validate ZMQ mode is stable for extended period

#### 48-Hour Soak Test

```bash
# Enable ZMQ mode
echo "ZMQ_ENABLED=true" >> .env

# Start full stack
docker-compose --profile full up -d

# Monitor for 48 hours
./monitor.sh > zmq_monitor.log 2>&1 &
```

**Monitor Script** (`monitor.sh`):
```bash
#!/bin/bash
while true; do
  echo "=== $(date) ==="

  # Check all services running
  docker-compose ps | grep -v "Up"

  # Check for ZMQ errors
  docker-compose logs --since 5m | grep -i "zmq.*error" | wc -l

  # Check message rates
  docker logs zmq_listener --since 5m | grep "Mirrored" | wc -l

  # Check latency (if instrumented)
  docker logs execution_service --since 5m | grep "End-to-end latency" | \
    awk '{print $NF}' | sed 's/ms//' | \
    awk '{sum+=$1; count++} END {if (count>0) print "Avg latency:", sum/count, "ms"}'

  echo ""
  sleep 300  # Every 5 minutes
done
```

**Success Criteria**:
- ✅ No service crashes for 48 hours
- ✅ No ZMQ connection errors
- ✅ Message rates steady (not dropping)
- ✅ Latency consistent (~100-120ms p50)
- ✅ Memory usage stable (not growing)

---

## Rollback Plan

If ZMQ causes issues, rollback is instant:

```bash
# Disable ZMQ, revert to Redis
echo "ZMQ_ENABLED=false" >> .env

# Restart services
docker-compose --profile full restart

# Verify Redis mode working
docker logs game_shard | grep -i "redis"
# Expected: "Connected to Redis" or similar
```

**Rollback Triggers**:
- ❌ Service crashes repeatedly
- ❌ Messages not flowing (zmq_listener stops mirroring)
- ❌ Latency WORSE than Redis mode (unexpected)
- ❌ High error rate in logs

---

## Troubleshooting Common Issues

### Issue 1: zmq_listener not receiving messages

**Symptom**:
```bash
docker logs zmq_listener
# No "Mirrored..." messages
```

**Debug**:
```bash
# Check ZMQ publishers are running
docker ps | grep -E "kalshi_monitor|polymarket_monitor|game_shard"

# Check network connectivity
docker exec zmq_listener nc -zv kalshi_monitor 5555
# Expected: Connection succeeded

# Check ZMQ_ENABLED in publisher
docker exec kalshi_monitor env | grep ZMQ_ENABLED
# Expected: ZMQ_ENABLED=true
```

**Fix**:
- Verify `ZMQ_ENABLED=true` in .env
- Restart publishers: `docker-compose restart kalshi_monitor polymarket_monitor`
- Check firewall/network rules

---

### Issue 2: game_shard not receiving prices

**Symptom**:
```bash
docker logs game_shard | grep "price update"
# No price updates
```

**Debug**:
```bash
# Check ZMQ subscription
docker logs game_shard | grep -i "zmq.*connect"
# Expected: "ZMQ subscriber connected to..."

# Check monitors are publishing
docker logs kalshi_monitor | grep -i "publish"
# Expected: Publish logs with ticker names

# Manual ZMQ test (if zmq tools installed)
docker run --rm --network arbees-network \
  zmqtools zmq-sub tcp://kalshi_monitor:5555
# Expected: Stream of messages
```

**Fix**:
- Verify `ZMQ_SUB_ENDPOINTS` correct in game_shard env
- Restart game_shard: `docker-compose restart game_shard`
- Check kalshi_monitor WebSocket is connected

---

### Issue 3: execution_service not receiving signals

**Symptom**:
```bash
docker logs execution_service | grep "signal"
# No signals received
```

**Debug**:
```bash
# Check game_shard is publishing
docker logs game_shard | grep "Published signal"
# Expected: "Published signal to ZMQ: signals.trade..."

# Check execution_service subscription
docker logs execution_service | grep -i "zmq.*connect"
# Expected: "ZMQ subscriber connected to tcp://game_shard:5558"

# Check if signals are being generated at all
docker logs game_shard | grep "arbitrage"
# Expected: Arbitrage detection logs
```

**Fix**:
- Check `MIN_EDGE_PCT` not too high (no signals if no good opportunities)
- Verify game_shard is running: `docker ps | grep game_shard`
- Restart execution_service: `docker-compose restart execution_service`

---

### Issue 4: High latency (worse than Redis)

**Symptom**: End-to-end latency >200ms (worse than Redis ~160-200ms)

**Debug**:
```bash
# Check if using ZMQ or Redis
docker logs game_shard | grep -i "mode"
# Expected: "ZMQ mode enabled" or similar

# Check message sizes (large payloads slow ZMQ)
docker logs kalshi_monitor | grep "payload.*bytes"
# Expected: <10KB per message

# Check CPU usage
docker stats --no-stream
# Expected: game_shard, execution_service <50% CPU
```

**Fix**:
- Verify ZMQ is actually enabled (`ZMQ_ENABLED=true`)
- Check network latency: `docker exec game_shard ping kalshi_monitor`
- Reduce message size (compress, remove unnecessary fields)
- Check for CPU throttling

---

## Next Steps After Validation

Once ZMQ hot path is validated (48+ hours stable):

### Option 1: Optimize Further (If Needed) ⚡

Only if latency target not met (<100ms):

```yaml
# Use Unix domain sockets instead of TCP (if all on same host)
ZMQ_KALSHI_ENDPOINT: "ipc:///tmp/kalshi.ipc"
ZMQ_POLYMARKET_ENDPOINT: "ipc:///tmp/poly.ipc"
# Expected: 0.1ms per hop (vs 0.5ms TCP)
```

Or switch to binary serialization:
```rust
// MessagePack instead of JSON (10x faster)
let payload = rmp_serde::to_vec(&price_update)?;
```

### Option 2: Phase Out Redis (Gradually) 🧹

Once confident in ZMQ:

**Week 1**: Keep dual mode (ZMQ + Redis + zmq_listener)
**Week 2**: Remove Redis publishes from hot path services
**Week 3**: Test without zmq_listener (ZMQ only, no Redis mirror)
**Week 4**: Remove Redis code from game_shard/execution_service

```rust
// Eventually remove Redis fallback code
// if zmq_enabled {
    let msg = zmq_sub.recv_multipart(0)?;
// } else {
//     let msg = redis_sub.recv()?;  // DELETE THIS
// }
```

### Option 3: Add More Services (If Beneficial) 📈

Based on [SERVICE_LATENCY_ANALYSIS.md](./SERVICE_LATENCY_ANALYSIS.md):

Priority if you want further improvements:
1. **position_tracker** - Faster exit signals (10-20ms benefit)
2. **api (WebSocket)** - Real-time dashboard (10-20ms benefit)

But **ONLY if hot path validation is successful first**.

---

## Decision Matrix

After 48-hour test, evaluate:

| Metric | Target | Actual | Decision |
|--------|--------|--------|----------|
| **Stability** | No crashes | ? | If crashes → Rollback |
| **Latency (p50)** | <120ms | ? | If >150ms → Debug |
| **Latency (p95)** | <150ms | ? | If >200ms → Debug |
| **Error rate** | <1% | ? | If >5% → Rollback |
| **Message rate** | Steady | ? | If dropping → Debug |

**Go/No-Go**:
- ✅ **GO**: Stability good + latency <120ms p50 → Keep ZMQ, phase out Redis
- ⚠️ **DEBUG**: Latency 120-150ms or minor issues → Investigate, optimize
- ❌ **NO-GO**: Crashes or latency >150ms → Rollback to Redis

---

## Summary Checklist

### Phase 1: Infrastructure ✅
- [ ] zmq_listener running
- [ ] Monitors publishing to ZMQ
- [ ] Messages mirrored to Redis

### Phase 2: game_shard ✅
- [ ] Subscribes to ZMQ prices
- [ ] Publishes signals to ZMQ
- [ ] No errors in logs

### Phase 3: execution_service ✅
- [ ] Subscribes to ZMQ signals
- [ ] Executes IOC orders
- [ ] Trades in database

### Phase 4: Measurement 📊
- [ ] Latency instrumentation added
- [ ] Metrics collected (1+ hour)
- [ ] Comparison vs Redis mode

### Phase 5: Stability 🛡️
- [ ] 48-hour soak test
- [ ] No crashes or errors
- [ ] Latency consistent

### Decision ✅
- [ ] Go/No-Go evaluation
- [ ] Document results
- [ ] Plan next steps

---

**Current Status**: Ready for Phase 1 testing
**Next Action**: Enable `ZMQ_ENABLED=true` and start Phase 1
**Estimated Time**: 7-14 days for full validation

**Document Status**: ✅ Ready to Execute
**Last Updated**: 2026-01-27
