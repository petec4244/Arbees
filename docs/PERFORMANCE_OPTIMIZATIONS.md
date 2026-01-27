# Performance Optimization Analysis

## Current System Delays

### Game Shard (`game_shard_rust`)
- **Poll Interval**: 1.0s (`POLL_INTERVAL`)
- **Signal Debounce**: 30s (`SIGNAL_DEBOUNCE_SECS`) ⚠️ **TOO LONG**
- **Heartbeat**: 10s
- **Signal Expiry**: 30s

### Position Tracker (`position_tracker_rust`)
- **Exit Check Interval**: 1.0s (`EXIT_CHECK_INTERVAL_SECS`)
- **Heartbeat**: 10s
- **Orphan Sweep**: 300s (5 min)
- **Price Staleness TTL**: 30s
- **Min Hold**: 10s (`MIN_HOLD_SECONDS`)

### Signal Processor (`signal_processor_rust`)
- **Heartbeat**: 10s
- **Cleanup Interval**: 60s
- **Price Staleness TTL**: 30s

### Other Services
- **Polymarket Monitor**: 1.5s poll interval
- **Market Discovery**: 30s-3600s (varies)

---

## Critical Bottlenecks Found

### 🔴 **HIGH PRIORITY: Sequential Price Lookups in Position Tracker**

**Location**: `services/position_tracker_rust/src/main.rs:686-779`

**Problem**: `check_exit_conditions()` calls `get_current_price()` sequentially for each position:
```rust
for (idx, position) in self.open_positions.iter().enumerate() {
    let price_row = self.get_current_price(position).await?;  // Sequential!
    // ...
}
```

**Impact**: With 10 positions, this adds ~10-50ms latency per exit check cycle, even when hitting cache.

**Fix**: Batch price lookups using `futures::future::join_all()` or `tokio::join!()`:
```rust
// Collect all price lookups
let price_futures: Vec<_> = self.open_positions.iter()
    .map(|p| self.get_current_price(p))
    .collect();
let prices = futures::future::join_all(price_futures).await;
```

**Expected Gain**: 5-10x faster exit checks (10ms → 1-2ms for 10 positions)

---

### 🟡 **MEDIUM PRIORITY: Signal Debounce Too Long**

**Location**: `services/game_shard_rust/src/shard.rs:514`

**Problem**: Default signal debounce is 30 seconds, meaning if an edge appears and disappears within 30s, we miss it.

**Current**: `SIGNAL_DEBOUNCE_SECS=30`

**Recommendation**: Reduce to 10-15s for faster signal generation:
- **10s**: More aggressive, captures fast-moving edges
- **15s**: Balanced (recommended)
- **30s**: Too conservative, misses opportunities

**Expected Gain**: 2-3x more signals captured during volatile periods

---

### 🟡 **MEDIUM PRIORITY: Sequential Position Closes**

**Location**: `services/position_tracker_rust/src/main.rs:767-776`

**Problem**: Positions are closed sequentially, blocking on DB updates:
```rust
for (idx, exec_price, reason) in positions_to_exit.into_iter().rev() {
    self.close_position(&position, exec_price, &reason, was_win).await?;
}
```

**Fix**: Parallelize closes (but serialize bankroll updates):
```rust
let close_futures: Vec<_> = positions_to_exit.into_iter()
    .map(|(idx, exec_price, reason)| {
        let position = self.open_positions.remove(idx);
        self.close_position(&position, exec_price, &reason, was_win)
    })
    .collect();
let results = futures::future::join_all(close_futures).await;
// Then batch update bankroll once
```

**Expected Gain**: 2-5x faster multi-position exits

---

### 🟢 **LOW PRIORITY: Exit Check Interval**

**Current**: 1.0s (`EXIT_CHECK_INTERVAL_SECS`)

**Analysis**: Already quite fast. Could reduce to 0.5s for sub-second exit latency, but:
- **Pros**: Faster exits on stop-loss/take-profit
- **Cons**: 2x DB/Redis load, minimal benefit if price cache is working

**Recommendation**: Keep at 1.0s unless you have <5 open positions regularly.

---

### 🟢 **LOW PRIORITY: Game Shard Poll Interval**

**Current**: 1.0s (`POLL_INTERVAL`)

**Analysis**: Already fast. ESPN API may rate-limit if too aggressive.

**Recommendation**: Keep at 1.0s, or reduce to 0.5s if ESPN allows.

---

## Recommended Optimizations

### Immediate (High Impact, Low Risk)

1. **Parallelize position price lookups** (Position Tracker)
   - **File**: `services/position_tracker_rust/src/main.rs`
   - **Function**: `check_exit_conditions()`
   - **Effort**: 30 minutes
   - **Impact**: 5-10x faster exit checks

2. **Reduce signal debounce** (Game Shard)
   - **File**: `.env`
   - **Change**: `SIGNAL_DEBOUNCE_SECS=15` (from 30)
   - **Effort**: 1 minute
   - **Impact**: 2-3x more signals during volatility

### Short-term (Medium Impact, Medium Risk)

3. **Parallelize position closes** (Position Tracker)
   - **File**: `services/position_tracker_rust/src/main.rs`
   - **Function**: `check_exit_conditions()`
   - **Effort**: 1 hour (need to handle bankroll updates carefully)
   - **Impact**: 2-5x faster multi-position exits

4. **Batch orphan sweep queries** (Position Tracker)
   - **File**: `services/position_tracker_rust/src/main.rs`
   - **Function**: `sweep_orphaned_positions()`
   - **Current**: N queries (one per game_id)
   - **Fix**: Single query with `WHERE game_id IN (...)` 
   - **Effort**: 30 minutes
   - **Impact**: 10x faster orphan sweeps

### Long-term (Lower Priority)

5. **Reduce exit check interval** to 0.5s (if <5 positions)
6. **Reduce game shard poll** to 0.5s (if ESPN allows)
7. **Add connection pooling metrics** to identify DB bottlenecks

---

## Performance Metrics to Track

Add these metrics to your monitoring:

1. **Position Tracker**:
   - `exit_check_duration_ms`: Time per exit check cycle
   - `price_lookup_cache_hit_rate`: % of price lookups hitting Redis cache
   - `positions_closed_per_second`: Throughput metric

2. **Game Shard**:
   - `signals_emitted_per_minute`: Signal generation rate
   - `poll_duration_ms`: ESPN fetch + signal generation time
   - `debounced_signals_count`: Signals blocked by debounce

3. **Signal Processor**:
   - `signal_processing_latency_ms`: Time from signal receipt to execution request
   - `market_price_lookup_duration_ms`: DB query time for market prices

---

## Configuration Recommendations

Add to `.env`:

```bash
# Signal generation (faster)
SIGNAL_DEBOUNCE_SECS=15  # Reduced from 30

# Position tracking (already optimal)
EXIT_CHECK_INTERVAL_SECS=1.0  # Keep at 1.0s
PRICE_STALENESS_TTL=30  # Keep at 30s

# Game shard (already optimal)
POLL_INTERVAL=1.0  # Keep at 1.0s
```

---

## Implementation Priority

1. ✅ **Parallelize price lookups** (30 min, high impact)
2. ✅ **Reduce signal debounce** (1 min, medium impact)
3. ⚠️ **Parallelize position closes** (1 hour, medium impact, needs testing)
4. ⚠️ **Batch orphan sweep** (30 min, low impact but easy win)

---

## Memory Profiling

### Memory Limits (docker-compose.yml)

All Rust services have memory limits configured via `x-common-resources`:
- **Limit**: 2GB (hard cap)
- **Reservation**: 512MB (guaranteed)

### Running the Profiler

Use the memory profiling script to monitor containers during live games:

```bash
# 1-hour profile with 10s sampling (default)
python scripts/profile_memory.py

# Quick 10-minute check, 5s sampling
python scripts/profile_memory.py --duration 600 --interval 5

# Focus on specific services
python scripts/profile_memory.py --containers game_shard position_tracker

# Set alert threshold to 70%
python scripts/profile_memory.py --threshold 70

# Quiet mode (alerts only)
python scripts/profile_memory.py --quiet
```

### Output

The profiler creates:
1. **Real-time display**: Memory/CPU stats for all arbees containers
2. **CSV file**: `reports/memory_profile_{timestamp}.csv` for analysis
3. **Alerts**: Console warnings when memory exceeds threshold

### Analyzing Results

```bash
# View CSV summary
python -c "
import pandas as pd
df = pd.read_csv('reports/memory_profile_*.csv')
print(df.groupby('container')['mem_pct'].agg(['mean', 'max', 'std']))
"
```

### Memory Warning Signs

| Symptom | Likely Cause | Fix |
|---------|--------------|-----|
| Steadily increasing memory | Memory leak | Check for unbounded caches, vec growth |
| Spikes during game starts | Large game state allocations | Pre-allocate or limit concurrent games |
| High baseline usage | Excessive caching | Reduce cache sizes, add TTLs |
| OOM kills | Exceeded 2GB limit | Profile and optimize hot paths |

### Rust Memory Debugging

For detailed Rust memory analysis:

```bash
# Build with debug symbols
cd services && cargo build --profile dev

# Run with memory profiler (Linux)
MALLOC_CONF=prof:true,prof_active:true ./target/debug/game_shard_rust

# Use Valgrind/Massif (Linux)
valgrind --tool=massif ./target/debug/game_shard_rust
ms_print massif.out.*
```

### Key Memory Hotspots

1. **Game Shard**: HashMap of game states - limit via `MAX_GAMES_PER_SHARD`
2. **Position Tracker**: Open positions vector - naturally bounded
3. **Market Discovery**: Cache of market IDs - has TTL
4. **Team Matching**: Alias cache - static, initialized once (OnceLock)
