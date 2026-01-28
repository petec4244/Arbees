# Arbees - Recommended Next Steps

*Generated: 2026-01-28*

## Recently Fixed Issues

### 1. Signal Processor Liquidity Rejection (FIXED)
**Problem**: All signals were being rejected with "insufficient_liquidity: $0.00 available < $10.00 minimum"

**Root Cause**: When the signal processor couldn't find market price data in the database, it fell back to creating a synthetic `MarketPriceRow` with hardcoded `yes_bid_size: Some(0.0)`. This caused the liquidity check to always fail.

**Fix Applied**: Changed `signal_processor_rust/src/main.rs` to use `signal.liquidity_available` (which is populated by game_shard from the orderbook) instead of hardcoded 0.0.

---

## Recommended Improvements

### High Priority

#### 1. Improve Polymarket WebSocket Reconnection Logic
**Issue**: WebSocket 1006 (abnormal closure) errors cause the monitor to disconnect without proper reconnection.

**Current State**: The monitor has basic reconnection but gets stuck doing VPN verification without reconnecting the WebSocket.

**Recommendation**:
- Add exponential backoff for reconnection (5s → 10s → 30s → cap at 2min)
- Set explicit `ping_interval=30` and `ping_timeout=10` on WebSocket connections
- Add connection health monitoring with automatic restart if no messages received in 60s
- Consider using the official Polymarket real-time-data-client or battle-tested community libraries

**File**: `services/polymarket_monitor/monitor.py`

#### 2. Add Orchestrator Health Check for Monitors
**Issue**: When Polymarket monitor disconnects, the orchestrator doesn't detect the failure and trigger a restart.

**Recommendation**:
- Register Polymarket monitor with orchestrator's health check system (like game_shard does)
- Add heartbeat publishing from polymarket_monitor
- Enable orchestrator to restart unhealthy monitors via Docker API

#### 3. Remove REST Poll Fallback (Low Value)
**Issue**: REST poll only publishes to Redis, not ZMQ. In ZMQ-only mode, REST poll prices never reach game_shard.

**Recommendation**: Since ZMQ is the primary transport and REST polling adds latency, consider:
- Removing REST poll entirely, OR
- Making REST poll publish to ZMQ as well (if kept for redundancy)

**File**: `services/polymarket_monitor/monitor.py` (lines 531-659)

---

### Medium Priority

#### 4. Reduce Liquidity Rejection Rate
**Current Stats**: ~50% of price messages are rejected for "no_liquidity" at the game_shard level.

**Current Check** (`game_shard_rust/src/shard.rs:769`):
```rust
let has_liquidity = price.yes_bid > 0.01 || price.yes_ask < 0.99;
```

**Recommendation**: This check may be too aggressive. Consider:
- Logging which markets are being filtered to understand patterns
- Adjusting thresholds based on market type (Kalshi vs Polymarket)
- Adding a "stale market" detection instead of pure liquidity check

#### 5. Add ZMQ Connection Monitoring Dashboard
**Issue**: Hard to diagnose ZMQ connectivity issues without manual log inspection.

**Recommendation**:
- Add Prometheus metrics for ZMQ message counts per channel
- Create Grafana dashboard showing message flow rates
- Add alerting when message rates drop to zero

---

### Low Priority

#### 6. Consolidate Stashed Changes
**Current State**: Multiple stashes exist with partial work:
- `stash@{1}`: "Other AI changes" on master (rust_core providers module)
- `stash@{2}`: "Liquidity fallback fix" on feature branch

**Recommendation**: Review and either apply or drop these stashes to avoid confusion.

#### 7. Update Dependencies
**Warning from build**:
```
the following packages contain code that will be rejected by a future version of Rust: redis v0.24.0, sqlx-postgres v0.7.4
```

**Recommendation**: Update redis and sqlx crates to latest versions before Rust deprecation.

---

## Architecture Notes

### Current Data Flow (ZMQ-Only Mode)
```
ESPN → orchestrator → Redis (game discovery)
                   ↓
         market_discovery → Redis (market IDs)
                   ↓
         game_shard ←ZMQ← kalshi_monitor
                   ←ZMQ← polymarket_monitor (via VPN)
                   ↓ ZMQ port 5558
         signal_processor
                   ↓ ZMQ port 5559
         execution_service → DB (paper_trades)
```

### Key Configuration
- `ZMQ_TRANSPORT_MODE=zmq_only` - Lowest latency mode
- `MIN_EDGE_PCT=4.0` - Minimum edge to generate signal
- `PAPER_TRADING=1` - Paper trading enabled
