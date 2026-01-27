# Arbees Operational Readiness Checklist

**Purpose**: Step-by-step guide to validate Arbees is ready for production trading
**Status**: Ready to Begin Testing
**Date**: 2026-01-27

---

## Pre-Flight Check ✅

### Code Status
- ✅ IOC orders implemented (commit 29bc99a)
- ✅ Rate limit handling implemented (commit 29bc99a)
- ✅ Order ID generation (atomic counter)
- ✅ WebSocket integration (sub-50ms latency)
- ✅ All core services in Rust
- ✅ Paper trading mode available

---

## Phase 1: Local Development Testing (Days 1-2)

### Step 1: Environment Setup

```bash
# 1. Copy environment template
cp .env.example .env

# 2. Configure critical settings
# Edit .env and set:
PAPER_TRADING=1  # CRITICAL: Must be 1 for testing
MIN_EDGE_PCT=2.0
MAX_POSITION_SIZE=100.0
MAX_DAILY_LOSS=500.0
KELLY_FRACTION=0.25

# 3. Add API credentials (for paper trading)
KALSHI_API_KEY=your_demo_api_key
KALSHI_PRIVATE_KEY=your_demo_private_key
# Note: Use demo/paper trading credentials for testing

# 4. Database credentials
POSTGRES_USER=arbees
POSTGRES_PASSWORD=your_secure_password
POSTGRES_DB=arbees
```

**Verification**:
```bash
# Check .env file exists and has required variables
grep -E "PAPER_TRADING|MIN_EDGE_PCT|KALSHI_API_KEY" .env
```

**Expected Output**:
```
PAPER_TRADING=1
MIN_EDGE_PCT=2.0
KALSHI_API_KEY=...
```

---

### Step 2: Infrastructure Start

```bash
# 1. Start core infrastructure
docker-compose up -d timescaledb redis

# 2. Wait for health checks (30 seconds)
sleep 30

# 3. Verify services are healthy
docker-compose ps

# Expected output:
# NAME                     STATUS              PORTS
# arbees-timescaledb       Up (healthy)        5432->5432
# arbees-redis             Up (healthy)        6379->6379
```

**Verification**:
```bash
# Test Redis connection
docker exec arbees-redis redis-cli ping
# Expected: PONG

# Test TimescaleDB connection
docker exec arbees-timescaledb pg_isready -U arbees
# Expected: /var/run/postgresql:5432 - accepting connections
```

---

### Step 3: Database Migration

```bash
# Migrations run automatically on TimescaleDB startup
# Verify tables exist:

docker exec -it arbees-timescaledb psql -U arbees -c "\dt"

# Expected tables:
# - paper_trades
# - bankroll
# - game_states
# - market_prices
# - trading_signals
```

**Verification**:
```bash
# Check paper_trades table exists
docker exec -it arbees-timescaledb psql -U arbees -c "SELECT COUNT(*) FROM paper_trades;"

# Expected: 0 (empty table)
```

---

### Step 4: Reset Paper Trading

```bash
# Reset to clean state
python scripts/reset_paper_trading.py --full

# Expected output:
# Paper trading data reset successfully
# Bankroll reset to $10,000.00
# 0 paper trades deleted
```

**Verification**:
```bash
# Verify bankroll
docker exec -it arbees-timescaledb psql -U arbees -c "SELECT * FROM bankroll ORDER BY timestamp DESC LIMIT 1;"

# Expected: balance = 10000.00, piggybank_balance = 0.00
```

---

### Step 5: Build Rust Services

```bash
cd services

# Check all Rust packages compile
cargo check

# Expected output:
# Checking arbees_rust_core...
# Checking market_discovery_rust...
# Checking orchestrator_rust...
# Checking game_shard_rust...
# Checking signal_processor_rust...
# Checking execution_service_rust...
# Checking position_tracker_rust...
# Finished dev [unoptimized + debuginfo] target(s) in X.XXs

cd ..
```

**Verification**:
```bash
# Run unit tests
cd services
cargo test --package arbees_rust_core --lib
cd ..

# Expected: All tests pass
```

---

### Step 6: Start Full Stack

```bash
# Start all services
docker-compose --profile full up -d

# Wait for services to start (60 seconds)
sleep 60

# Check all services are running
docker-compose ps
```

**Expected Services**:
```
arbees-timescaledb              Up (healthy)
arbees-redis                    Up (healthy)
arbees-orchestrator             Up
arbees-market-discovery         Up
arbees-game-shard-rust          Up
arbees-signal-processor-rust    Up
arbees-execution-service-rust   Up
arbees-position-tracker-rust    Up
arbees-kalshi-monitor           Up
arbees-polymarket-monitor       Up (with VPN)
arbees-vpn                      Up (healthy)
arbees-api                      Up
arbees-frontend                 Up
arbees-notification-service-rust Up
arbees-analytics                Up
```

**Verification**:
```bash
# Check for critical errors in logs
docker-compose logs --tail=50 orchestrator
docker-compose logs --tail=50 execution_service

# Should see startup messages, no errors
```

---

## Phase 2: Functional Testing (Days 3-4)

### Test 1: Service Health Checks

```bash
# 1. Check orchestrator is discovering games
docker-compose logs --tail=20 orchestrator | grep -i "discovered"

# Expected: Lines showing game discovery
# Example: "Discovered 15 NBA games"

# 2. Check market discovery is finding markets
docker-compose logs --tail=20 market-discovery-rust | grep -i "market"

# Expected: Market discovery activity

# 3. Check game shard is receiving updates
docker-compose logs --tail=20 game_shard | grep -i "update"

# Expected: Game state updates

# 4. Check signal processor is generating signals
docker-compose logs --tail=20 signal_processor | grep -i "signal"

# Expected: Signal generation or "No signals" message
```

**Success Criteria**:
- ✅ Orchestrator discovers games
- ✅ Market discovery finds market IDs
- ✅ Game shard processes updates
- ✅ Signal processor evaluates opportunities

---

### Test 2: Order Execution (IOC Orders)

```bash
# Watch execution service logs in real-time
docker-compose logs -f execution_service | grep -i "IOC"

# Look for:
# - "Placing IOC order"
# - "IOC order ... placed"
# - client_order_id in format "arb{timestamp}{counter}"
```

**Success Criteria**:
- ✅ All orders have `client_order_id`
- ✅ All orders show "Placing IOC order" log
- ✅ Order IDs are unique (timestamp + counter)
- ✅ No orders with status "resting"

**Check Database**:
```bash
# Query recent paper trades
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT order_id, status, filled_qty, created_at
   FROM paper_trades
   ORDER BY created_at DESC
   LIMIT 10;"
```

**Expected**:
- `status` is "filled" or "cancelled" (never "resting" or "pending")
- `filled_qty` <= `quantity`

---

### Test 3: Rate Limit Handling

```bash
# Monitor for rate limit warnings
docker-compose logs -f execution_service | grep -i "rate limit"

# Look for:
# - "Kalshi rate limit hit, backing off Xms"
# - Automatic retry after backoff
# - Eventually successful request
```

**Simulate Rate Limiting** (if needed):
```bash
# Trigger burst of trades to hit rate limits
# Monitor execution_service logs for exponential backoff:
# - "backing off 4000ms (retry 1/5)"
# - "backing off 8000ms (retry 2/5)"
# - "backing off 16000ms (retry 3/5)"
```

**Success Criteria**:
- ✅ Rate limits logged with backoff time
- ✅ Automatic retry after backoff
- ✅ Eventually succeeds (within 5 retries)
- ✅ Circuit breaker does NOT trip on 429 errors

---

### Test 4: One-Sided Fill Detection

```bash
# Query trades to check for one-sided fills
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT COUNT(*) as total_trades,
          SUM(CASE WHEN status = 'filled' THEN 1 ELSE 0 END) as filled,
          SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END) as cancelled,
          SUM(CASE WHEN status = 'partial' THEN 1 ELSE 0 END) as partial
   FROM paper_trades;"
```

**Success Criteria**:
- ✅ No "partial" fills for IOC orders (should be 0)
- ✅ Only "filled" or "cancelled" statuses
- ✅ Zero one-sided arbitrage positions

**Check Position Balance**:
```bash
# All positions should be balanced (both legs filled or both cancelled)
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT game_id, platform, side, SUM(filled_qty) as net_position
   FROM paper_trades
   GROUP BY game_id, platform, side
   HAVING SUM(filled_qty) != 0;"
```

**Expected**: Empty result (no unbalanced positions)

---

### Test 5: WebSocket Latency

```bash
# Check monitor latencies
docker-compose logs --tail=100 kalshi_monitor | grep -i "latency"
docker-compose logs --tail=100 polymarket_monitor | grep -i "latency"

# Look for latency measurements in logs
```

**Success Criteria**:
- ✅ Kalshi WebSocket latency: <50ms
- ✅ Polymarket WebSocket latency: <50ms
- ✅ No disconnection/reconnection loops

---

### Test 6: End-to-End Latency

```bash
# Trace a signal from game update to execution
# 1. Find game update timestamp in orchestrator logs
# 2. Find signal generation timestamp in signal_processor logs
# 3. Find execution timestamp in execution_service logs

# Calculate total latency
```

**Target**: <200ms p95 (game update → signal → execution)

**Measurement Method**:
```bash
# Enable latency instrumentation in .env
LOG_LEVEL=DEBUG

# Restart services
docker-compose --profile full restart

# Monitor timestamps in logs
docker-compose logs -f | grep -E "game_update|signal_generated|order_placed"
```

---

## Phase 3: 48-Hour Soak Test (Days 5-6)

### Monitoring Setup

```bash
# Create monitoring script
cat > monitor.sh << 'EOF'
#!/bin/bash
while true; do
  echo "=== $(date) ==="

  # Service health
  docker-compose ps

  # Trade count
  docker exec arbees-timescaledb psql -U arbees -qAt -c \
    "SELECT COUNT(*) FROM paper_trades;"

  # P&L
  docker exec arbees-timescaledb psql -U arbees -qAt -c \
    "SELECT balance FROM bankroll ORDER BY timestamp DESC LIMIT 1;"

  # Errors
  docker-compose logs --since 5m | grep -i error | wc -l

  echo ""
  sleep 300  # 5 minutes
done
EOF

chmod +x monitor.sh

# Run in background
./monitor.sh > monitor.log 2>&1 &
```

### Daily Checks

**Morning Check (Day 5, Day 6)**:
```bash
# 1. Check all services still running
docker-compose ps

# 2. Check error count
docker-compose logs --since 24h | grep -i error | wc -l

# 3. Check trade volume
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT COUNT(*) as trades_last_24h,
          AVG(pnl) as avg_pnl,
          SUM(pnl) as total_pnl
   FROM paper_trades
   WHERE created_at > NOW() - INTERVAL '24 hours';"

# 4. Check bankroll
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT balance, piggybank_balance,
          (balance - 10000.00) as profit_loss
   FROM bankroll
   ORDER BY timestamp DESC
   LIMIT 1;"
```

**Evening Check (Day 5, Day 6)**:
```bash
# Same checks as morning
# Compare results to detect trends
```

---

### Soak Test Success Criteria

After 48 hours, verify:

#### ✅ Stability
- [ ] All services still running (no crashes)
- [ ] No service restarts required
- [ ] Memory usage stable (not growing unbounded)
- [ ] CPU usage reasonable (<50% average)

#### ✅ Execution Quality
- [ ] Zero one-sided fills
- [ ] All orders have `client_order_id`
- [ ] All orders have `time_in_force = "immediate_or_cancel"`
- [ ] No orders with status "resting"
- [ ] Rate limits handled automatically (no circuit breaker trips)

#### ✅ Performance
- [ ] WebSocket latency <50ms (kalshi_monitor, polymarket_monitor)
- [ ] End-to-end latency <200ms p95 (game update → execution)
- [ ] Signal generation latency <50ms
- [ ] Database query latency <100ms

#### ✅ Data Quality
- [ ] No NULL `order_id` or `client_order_id` in trades
- [ ] No orphaned positions (unbalanced arb legs)
- [ ] Bankroll balance matches trade P&L
- [ ] No negative piggybank balance

---

## Phase 4: Analysis & Optimization (Days 7-8)

### Generate Report

```bash
# Run analytics
docker exec -it arbees-analytics python -c "
from services.analytics_service.analyzer import generate_report
report = generate_report(lookback_hours=48)
print(report)
"

# Or use frontend dashboard
# Open browser to http://localhost:3000
```

### Key Metrics to Review

1. **Trade Metrics**
   ```sql
   -- Fill rate by platform
   SELECT platform,
          COUNT(*) as total_orders,
          SUM(CASE WHEN status='filled' THEN 1 ELSE 0 END) as filled,
          ROUND(100.0 * SUM(CASE WHEN status='filled' THEN 1 ELSE 0 END) / COUNT(*), 2) as fill_rate
   FROM paper_trades
   GROUP BY platform;
   ```

2. **Latency Distribution**
   ```sql
   -- Execution latency percentiles
   SELECT percentile_cont(0.50) WITHIN GROUP (ORDER BY latency_ms) as p50,
          percentile_cont(0.90) WITHIN GROUP (ORDER BY latency_ms) as p90,
          percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms) as p95,
          percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms) as p99
   FROM paper_trades
   WHERE latency_ms IS NOT NULL;
   ```

3. **Edge Distribution**
   ```sql
   -- Average edge by trade type
   SELECT opportunity_type,
          COUNT(*) as count,
          ROUND(AVG(edge_pct), 2) as avg_edge,
          ROUND(AVG(pnl), 2) as avg_pnl
   FROM paper_trades
   GROUP BY opportunity_type;
   ```

4. **P&L Analysis**
   ```sql
   -- Daily P&L
   SELECT DATE(created_at) as trade_date,
          COUNT(*) as num_trades,
          ROUND(SUM(pnl), 2) as daily_pnl,
          ROUND(AVG(pnl), 2) as avg_pnl_per_trade
   FROM paper_trades
   GROUP BY DATE(created_at)
   ORDER BY trade_date;
   ```

---

### Optimization Decisions

Based on 48-hour test results:

#### If Fill Rate < 50%
- **Issue**: Not enough liquidity at target prices
- **Actions**:
  - Reduce `MIN_EDGE_PCT` (try 1.5% instead of 2.0%)
  - Increase order size to hit more liquidity
  - Review market selection (focus on liquid markets)

#### If Latency > 200ms p95
- **Issue**: Pipeline too slow
- **Actions**:
  - Profile Redis pub/sub latency
  - Check game_shard processing time
  - Verify network latency to APIs
  - Consider reducing logging verbosity

#### If Rate Limits Frequent
- **Issue**: Hitting Kalshi rate limits often
- **Actions**:
  - Increase `KALSHI_REQUEST_DELAY_MS` (try 500ms)
  - Reduce market discovery refresh frequency
  - Batch market requests more aggressively

#### If One-Sided Fills Detected
- **Issue**: IOC implementation not working
- **Actions**:
  - ❌ **CRITICAL BUG** - This should NOT happen
  - Review execution_service logs for errors
  - Verify `time_in_force` in Kalshi API requests
  - Check order status transitions

---

## Phase 5: Production Preparation (Days 9-10)

### Infrastructure Checklist

#### ✅ Security
- [ ] API keys stored securely (not in code)
- [ ] Database password changed from default
- [ ] Redis password set (optional but recommended)
- [ ] VPN credentials secured
- [ ] .env file not committed to git

#### ✅ Monitoring
- [ ] Log aggregation configured (if deploying to cloud)
- [ ] Alert thresholds set (circuit breaker trips, daily loss limit)
- [ ] Dashboard accessible
- [ ] Notification service configured (Signal/SMS)

#### ✅ Backups
- [ ] Database backup schedule configured
- [ ] TimescaleDB volume persisted
- [ ] Redis volume persisted (or AOF enabled)
- [ ] Trade history exported regularly

#### ✅ Risk Management
- [ ] `PAPER_TRADING=1` for initial production run
- [ ] `MAX_DAILY_LOSS` set conservatively ($500)
- [ ] `MAX_POSITION_SIZE` set conservatively ($100)
- [ ] `KELLY_FRACTION` set conservatively (0.25)
- [ ] Emergency stop procedure documented

---

### Pre-Live Checklist (CRITICAL)

**BEFORE setting `PAPER_TRADING=0`:**

- [ ] ✅ 48-hour soak test completed successfully
- [ ] ✅ Zero one-sided fills in test
- [ ] ✅ All IOC orders working correctly
- [ ] ✅ Rate limit handling tested and working
- [ ] ✅ End-to-end latency <200ms p95
- [ ] ✅ P&L tracking accurate (matches expected)
- [ ] ✅ Risk limits tested (daily loss cutoff works)
- [ ] ✅ Emergency stop procedure tested
- [ ] ✅ Monitoring and alerts configured
- [ ] ✅ Real API credentials (production) configured
- [ ] ✅ Sufficient balance in trading accounts
- [ ] ✅ Team aware of go-live timing
- [ ] ✅ Runbook documented for common issues

**WARNING**: Do NOT proceed to live trading until ALL items checked.

---

## Emergency Procedures

### Stop All Trading Immediately

```bash
# Method 1: Stop execution service only (keeps monitoring running)
docker-compose stop execution_service

# Method 2: Stop entire stack
docker-compose --profile full down

# Method 3: Set emergency flag in Redis
docker exec arbees-redis redis-cli SET "trading:emergency_stop" "1"
```

### Cancel All Open Orders

```bash
# For paper trading (clears pending orders)
python scripts/reset_paper_trading.py

# For live trading (would need Kalshi API calls)
# NOT IMPLEMENTED - manual intervention required
```

### Check Current Positions

```bash
# View all open positions
docker exec -it arbees-timescaledb psql -U arbees -c \
  "SELECT game_id, platform, side, SUM(filled_qty) as position
   FROM paper_trades
   WHERE status = 'filled'
   GROUP BY game_id, platform, side
   HAVING SUM(filled_qty) > 0;"
```

---

## Common Issues & Solutions

### Issue: "Circuit breaker open"
**Cause**: Too many API failures
**Solution**:
```bash
# Check API status
curl -I https://api.elections.kalshi.com/trade-api/v2/markets

# Reset circuit breaker (restart service)
docker-compose restart execution_service

# Review logs for root cause
docker-compose logs --tail=100 execution_service | grep -i error
```

### Issue: "No liquidity"
**Cause**: Markets too illiquid or edge too aggressive
**Solution**:
- Reduce `MIN_EDGE_PCT`
- Increase order size
- Focus on liquid markets only

### Issue: "VPN disconnected"
**Cause**: VPN container lost connection
**Solution**:
```bash
# Check VPN health
docker-compose logs vpn | tail -20

# Restart VPN (will reconnect)
docker-compose restart vpn polymarket_monitor

# Check new IP location
docker exec arbees-vpn wget -qO- http://ipinfo.io/json
```

### Issue: "Redis connection refused"
**Cause**: Redis not running or crashed
**Solution**:
```bash
# Restart Redis
docker-compose restart redis

# Check persistence
docker exec arbees-redis redis-cli INFO persistence

# Restart dependent services
docker-compose restart game_shard signal_processor execution_service
```

---

## Success Definition

**Arbees is production-ready when**:

✅ **Stability**: 48+ hours uptime without crashes or manual intervention
✅ **Execution**: Zero one-sided fills, all IOC orders working
✅ **Performance**: <200ms p95 end-to-end latency
✅ **Resilience**: Rate limits handled automatically, circuit breaker only trips on real errors
✅ **Profitability**: Positive expected value (edge > fees + slippage)

---

## Timeline Summary

| Phase | Duration | Key Activities |
|-------|----------|---------------|
| **Phase 1** | Days 1-2 | Environment setup, infrastructure start, service build |
| **Phase 2** | Days 3-4 | Functional testing, order execution validation |
| **Phase 3** | Days 5-6 | 48-hour soak test, monitoring, data collection |
| **Phase 4** | Days 7-8 | Analysis, optimization, configuration tuning |
| **Phase 5** | Days 9-10 | Production prep, security audit, go/no-go decision |

**Total**: 10 days from start to production-ready decision

---

**Next Action**: Begin Phase 1, Step 1 (Environment Setup)
**Document Status**: ✅ Ready to Execute
**Last Updated**: 2026-01-27
