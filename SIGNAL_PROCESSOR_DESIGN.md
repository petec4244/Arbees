# Signal Processor - Detailed Planning Document

**Version**: 1.0
**Last Updated**: 2026-01-30
**Service**: `services/signal_processor_rust/src/main.rs`
**Language**: Rust (async/tokio)
**Status**: ✅ Active

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Architecture Overview](#architecture-overview)
3. [Main Responsibilities](#main-responsibilities)
4. [Signal Filtering Pipeline](#signal-filtering-pipeline)
5. [Risk Management](#risk-management)
6. [Position Sizing](#position-sizing)
7. [Market Price Resolution](#market-price-resolution)
8. [Async Background Tasks](#async-background-tasks)
9. [State Management](#state-management)
10. [Configuration & Environment](#configuration--environment)
11. [Database Interactions](#database-interactions)
12. [Rule System](#rule-system)
13. [Execution Modes](#execution-modes)
14. [Performance Characteristics](#performance-characteristics)
15. [Integration Points](#integration-points)
16. [Troubleshooting Guide](#troubleshooting-guide)

---

## Executive Summary

The Signal Processor is the **central gatekeeper and risk validator** for sports trading signals in the Arbees arbitrage system. It sits between signal generation (game_shard_rust) and execution (execution_service_rust), applying multi-stage filtering and risk validation to ensure only high-confidence, risk-compliant trades reach execution.

**Key Metrics**:
- **Latency**: 60-120ms signal-to-execution (vs 300-600ms sequential)
- **Throughput**: ~100 signals/minute (typical sports markets)
- **Rejection Rate**: 70-90% of incoming signals (high bar for trades)
- **Uptime**: Continuous with automatic reconnection

**Data Flow**:
```
game_shard_rust (ZMQ :5558)
         ↓
   signal_processor_rust
     (7-stage filtering)
         ↓
execution_service_rust (ZMQ :5559)
         ↓
    paper_trades DB
```

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                   Signal Processor Service                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │         Four Concurrent Async Tasks                       │   │
│  ├──────────────────────────────────────────────────────────┤   │
│  │ 1. ZMQ Listener Loop (port 5558)                         │   │
│  │    └─ Receives TradingSignals from game_shard_rust       │   │
│  │                                                           │   │
│  │ 2. Heartbeat Publisher Loop (10s interval)              │   │
│  │    └─ Publishes service health to Redis                │   │
│  │                                                           │   │
│  │ 3. Rule Update Subscriber                               │   │
│  │    └─ Listens for dynamic trading rule updates          │   │
│  │                                                           │   │
│  │ 4. In-Flight Cleanup Loop (60s interval)                │   │
│  │    └─ Garbage collects deduplication map                │   │
│  └──────────────────────────────────────────────────────────┘   │
│         ↓                                                         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Signal Processing Pipeline (per signal)                 │   │
│  ├──────────────────────────────────────────────────────────┤   │
│  │ 1. Pre-Trade Filtering                                  │   │
│  │    ├─ Market data check (has market_prob)              │   │
│  │    ├─ Edge threshold (>= MIN_EDGE_PCT)                 │   │
│  │    ├─ Probability bounds (not near-certainties)        │   │
│  │    ├─ Duplicate check (same-side positions)            │   │
│  │    ├─ Cooldown check (team-specific)                   │   │
│  │    └─ Rule-based filtering (dynamic rules)             │   │
│  │         ↓ (Survives all filters?)                       │   │
│  │ 2. Risk Management (7 parallel DB checks)              │   │
│  │    ├─ Bankroll sufficiency                             │   │
│  │    ├─ Daily loss limit                                 │   │
│  │    ├─ Per-game exposure limit                          │   │
│  │    ├─ Per-sport exposure limit                         │   │
│  │    ├─ Opposing position check                          │   │
│  │    ├─ Position count limit                             │   │
│  │    └─ Liquidity threshold                              │   │
│  │         ↓ (All checks pass?)                            │   │
│  │ 3. Position Sizing                                      │   │
│  │    ├─ Kelly criterion calculation                      │   │
│  │    ├─ Fee reservation                                  │   │
│  │    └─ Liquidity capping (max 80% of available)         │   │
│  │         ↓                                               │   │
│  │ 4. Execution Request Publishing                        │   │
│  │    ├─ Create ExecutionRequest (idempotent key)         │   │
│  │    ├─ Publish to ZMQ (port 5559)                       │   │
│  │    └─ Track in-flight deduplication                    │   │
│  └──────────────────────────────────────────────────────────┘   │
│         ↓                                                         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Backend Services                                        │   │
│  ├──────────────────────────────────────────────────────────┤   │
│  │ • PostgreSQL (market_prices, paper_trades, rules)       │   │
│  │ • Redis (heartbeat, notifications, rule updates)        │   │
│  │ • ZMQ (signal input, execution output)                  │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Main Responsibilities

### 1. Signal Ingestion

**ZMQ Subscriber** (port 5558, configurable):
- Topic subscription: `signals.trade.*`
- Multipart message handling: [topic, JSON envelope]
- Envelope format: `{ seq, timestamp_ms, source, payload: TradingSignal }`
- 30-second receive timeout (normal for low-traffic periods)
- Automatic reconnect on connection loss (5-second retry)

### 2. Pre-Trade Signal Filtering (6 stages)

Sequential filtering that rejects signals **before** database access:

| Stage | Check | Threshold | Rejection Counter |
|-------|-------|-----------|-------------------|
| 1 | Market data present | `market_prob != None` | `no_market` |
| 2 | Edge threshold | `edge_pct >= MIN_EDGE_PCT` (15%) | `edge` |
| 3 | Probability bounds | Not `> 0.95` (buy) or `< 0.05` (sell) | `prob` |
| 4 | No same-side duplicate | No existing open position, same direction | `duplicate` |
| 5 | Team cooldown expired | Not in `WIN_COOLDOWN` (180s) or `LOSS_COOLDOWN` (300s) | `cooldown` |
| 6 | Rule not blocking | Trading rule doesn't reject signal | `rule_blocked` |

**Key Points**:
- All filters applied sequentially; first failure triggers rejection
- Edge threshold can be overridden by trading rules
- Probability bounds skipped for arbitrage signals (risk-free)
- Cooldowns are **team-specific** (allows trading opposite team in same game)
- Rules loaded from `trading_rules` table at startup + on update notification

### 3. Risk Management (7 Parallel DB Checks)

All queries run in **parallel** using `tokio::join!` for ~80% latency reduction:

```rust
tokio::join!(
    get_available_balance(),        // Check bankroll
    get_daily_loss(),               // Check daily loss limit
    get_game_exposure(game_id),     // Check per-game exposure
    get_sport_exposure(sport),      // Check per-sport exposure
    count_game_positions(game_id),  // Check position count
    has_opposing_position(game_id, team, direction), // Check flip
    get_market_price(signal)        // Get current market price
)
```

**Risk Checks**:

| Check | Query | Limit | Rejection Reason |
|-------|-------|-------|-------------------|
| Bankroll | `SELECT balance FROM bankroll` | `proposed_size <= available` | Insufficient balance |
| Daily Loss | `SELECT SUM(loss) FROM trades WHERE date=today AND pnl<0` | `daily_loss < MAX_DAILY_LOSS` | Daily loss exceeded |
| Game Exposure | `SELECT SUM(size) FROM trades WHERE game_id=? AND (open OR closed<60s)` | `game_exposure + size < MAX_GAME_EXPOSURE` | Game exposure exceeded |
| Sport Exposure | `SELECT SUM(size) FROM trades WHERE sport=? AND open` | `sport_exposure + size < MAX_SPORT_EXPOSURE` | Sport exposure exceeded |
| Opposing Position | `SELECT COUNT(*) FROM trades WHERE game_id=? AND side=opposite AND open` | Must be 0 | Flip-flopping prevented |
| Position Count | `SELECT COUNT(*) FROM trades WHERE game_id=? AND open` | `<= 2` | Too many positions in game |
| Liquidity | Market data `yes_bid_size` / `yes_ask_size` | `>= LIQUIDITY_MIN_THRESHOLD` | Insufficient liquidity |

**Output**: `(bool approved, String rejection_reason, RiskSnapshot current_state)`

### 4. Position Sizing

**Kelly Criterion** with fee reservation and cap:

```
1. Calculate full Kelly: f = (bp - q) / b
   where b=odds, p=probability, q=1-p

2. Apply fractional Kelly: f * KELLY_FRACTION (default 0.25x)

3. Cap to maximum position: min(f * 100%, MAX_POSITION_PCT) (default 10%)

4. Reserve fees: balance / (1 + fee_rate)
   - Kalshi: 1.4% round-trip (0.7% entry + 0.7% exit)
   - Polymarket: 4% round-trip (~2% each side)

5. Calculate size: fee_adjusted_balance * (capped_pct / 100%)

6. Cap to liquidity: min(size, available_liquidity * 0.80)

7. Minimum: Always >= $1.00
```

### 5. Liquidity Validation

**Three-level minimum threshold hierarchy**:

1. **Per-platform override** (if set):
   - `LIQUIDITY_MIN_THRESHOLD_KALSHI`
   - `LIQUIDITY_MIN_THRESHOLD_POLYMARKET`
   - `LIQUIDITY_MIN_THRESHOLD_PAPER`

2. **Per-market-type override** (if set):
   - `LIQUIDITY_MIN_THRESHOLD_SPORT`
   - `LIQUIDITY_MIN_THRESHOLD_CRYPTO`
   - `LIQUIDITY_MIN_THRESHOLD_ECONOMICS`
   - `LIQUIDITY_MIN_THRESHOLD_POLITICS`
   - `LIQUIDITY_MIN_THRESHOLD_ENTERTAINMENT`

3. **Default**: `LIQUIDITY_MIN_THRESHOLD` ($10)

**Position Capping**: Max position = `available * 80%` (configurable via `LIQUIDITY_MAX_POSITION_PCT`)

### 6. Market Price Resolution

**Query Strategy**:
1. Prefer platform specified in signal (`signal.platform_buy`)
2. Use prices fresher than `PRICE_STALENESS_TTL` (default 30s)
3. Fuzzy team matching: `match_team_in_text()` with >= 0.7 confidence
4. **Fallback**: Synthesize price from signal data if DB lookup fails

**Rejection**: If no market price found AND signal has no `market_prob`, reject as `no_market`

### 7. Execution Request Publishing

**Creation**:
- Unique UUID per request
- Idempotency key: `game_id:team:direction` (prevents duplicate execution)
- Limit price:
  - **Buy**: `market.yes_ask` (seller's asking price)
  - **Sell**: `1.0 - market.yes_bid` (NO side = complement of YES bid)
- Size: Kelly-adjusted, liquidity-capped

**ZMQ Publishing**:
- Topic: `execution.request.{request_id}`
- Envelope: `{ seq, timestamp_ms, source: "signal_processor", payload: ExecutionRequest }`
- Atomic seq counter using `AtomicU64`
- Multipart message: [topic, JSON]

### 8. Deduplication

**In-Flight Tracking**:
- Maps `idempotency_key` → timestamp
- Rejects if key already in flight (prevents duplicate execution)
- **Cleanup**: Async loop (every 60s) removes entries > 5 minutes old
- **Rationale**: Handles ZMQ retries and recovery scenarios

---

## Signal Filtering Pipeline

**Flow Diagram**:

```
Incoming TradingSignal
        ↓
┌─ Has market_prob? ──NO──→ REJECT (no_market)
│
├─ edge_pct >= MIN_EDGE_PCT? ──NO──→ REJECT (edge)
│
├─ Probability bounds OK? ──NO──→ REJECT (prob_high/prob_low)
│  (Skip for arbitrage)
│
├─ No same-side duplicate? ──NO──→ REJECT (duplicate)
│
├─ Team not in cooldown? ──NO──→ REJECT (cooldown)
│
├─ Trading rule allows? ──NO──→ REJECT (rule_blocked)
│  (Rules can override edge_pct)
│
└─ PASS FILTERS ──→ Risk Management Stage
```

**Rejection Statistics**:
```
Typical sports market:
  100 signals received
   ├─  15 rejected: edge threshold
   ├─  20 rejected: probability bounds
   ├─   8 rejected: duplicate
   ├─  12 rejected: cooldown
   ├─   5 rejected: rule blocked
   ├─  15 rejected: risk limits
   └─  25 approved → execution
```

---

## Risk Management

### Parallel Query Execution

All 7 risk checks run concurrently using `tokio::join!`:

```rust
let (
    balance_result,
    daily_loss_result,
    game_exposure_result,
    sport_exposure_result,
    position_count_result,
    opposing_position_result,
    market_price_result,
) = tokio::join!(
    get_available_balance(),
    get_daily_loss(),
    get_game_exposure(game_id),
    get_sport_exposure(sport),
    count_game_positions(game_id),
    has_opposing_position(game_id, team, direction),
    get_market_price(signal)
);
```

**Latency Profile**:
- Parallel execution: ~50-100ms
- Sequential would be: ~300-600ms
- **80% latency reduction**

### Risk Check Details

#### 1. Bankroll Sufficiency
```sql
SELECT COALESCE(current_balance, 0.0)::float8
FROM bankroll
ORDER BY updated_at DESC
LIMIT 1
```
- Falls back to `INITIAL_BANKROLL` if table empty
- Rejects if `proposed_size > available_balance`

#### 2. Daily Loss Limit
```sql
SELECT SUM(CASE WHEN pnl < 0 THEN -pnl ELSE 0 END)::float8
FROM paper_trades
WHERE DATE(closed_at) = CURRENT_DATE
  AND status IN ('closed', 'settled')
```
- Sums all losses from today's closed trades
- Rejects if `daily_loss >= MAX_DAILY_LOSS` (default $100)

#### 3. Per-Game Exposure
```sql
SELECT SUM(size)::float8 as exposure
FROM paper_trades
WHERE game_id = $1
  AND (status = 'open' OR (status = 'closed' AND closed_at > NOW() - 60s))
```
- Includes both open positions AND recently-closed (60s window)
- Rejects if `game_exposure + proposed_size > MAX_GAME_EXPOSURE` (default $50)
- **Special**: If `MAX_GAME_EXPOSURE < 0`, limit disabled entirely

#### 4. Per-Sport Exposure
```sql
SELECT SUM(size)::float8 as exposure
FROM paper_trades p
JOIN market_prices m ON p.game_id = m.game_id
WHERE m.sport = $1
  AND p.status = 'open'
```
- Sums all open positions for the sport
- Rejects if `sport_exposure + proposed_size > MAX_SPORT_EXPOSURE` (default $200)

#### 5. Opposing Position Check
```sql
SELECT COUNT(*)::int as count
FROM paper_trades
WHERE game_id = $1
  AND market_title = $2
  AND side = opposite($3)
  AND status = 'open'
```
- Prevents flip-flopping (buying then immediately selling same team)
- Rejects if existing opposing position found

#### 6. Position Count Per Game
```sql
SELECT COUNT(*)::int as count
FROM paper_trades
WHERE game_id = $1
  AND status = 'open'
```
- Limits max 2 concurrent positions in single game
- Rejects if `>= 2` open positions already exist

#### 7. Liquidity Check
- Buy: Requires `yes_ask_size >= LIQUIDITY_MIN_THRESHOLD`
- Sell: Requires `yes_bid_size >= LIQUIDITY_MIN_THRESHOLD`
- Falls back to signal's `liquidity_available` if market has no data

### Risk Rejection Output

When any risk check fails:

```json
{
  "approved": false,
  "rejection_reason": "Daily loss limit reached: $100.50 >= $100.00",
  "risk_snapshot": {
    "balance": 950.0,
    "daily_loss": 100.50,
    "game_exposure": 45.0,
    "sport_exposure": 180.0,
    "position_count": 1
  },
  "notification_event": {
    "type": "risk_rejection",
    "priority": "WARNING",
    "data": { ... above ... }
  }
}
```

---

## Position Sizing

### Kelly Criterion Calculation

**Formula**: `f = (bp - q) / b`
- `b` = odds (ratio of potential gain to stake)
- `p` = probability of winning
- `q` = 1 - p (probability of losing)

**Example**:
```
Signal: BTC at 0.45 YES, model_prob 0.55
  → odds = 1/0.45 = 2.22 (if yes wins, get $2.22 per $1 bet)
  → f = (2.22 * 0.55 - 0.45) / 2.22 = 0.3 (30% of bankroll)
  → Apply 0.25x Kelly: 0.3 * 0.25 = 7.5% of bankroll
```

### Fee Reservation

Different platforms charge different fees:

**Kalshi**: ~1.4% round-trip
```
Reserve: balance / (1 + 0.014) = balance * 0.986
Available for trading: 98.6% of balance
```

**Polymarket**: ~4% round-trip
```
Reserve: balance / (1 + 0.04) = balance * 0.962
Available for trading: 96.2% of balance
```

### Position Capping

1. **Kelly cap**: Limited to `MAX_POSITION_PCT` (default 10%)
   ```
   position_pct = min(kelly_pct, max_position_pct)
   ```

2. **Liquidity cap**: Limited to 80% of available market liquidity
   ```
   max_size = available_liquidity * LIQUIDITY_MAX_POSITION_PCT (80%)
   final_size = min(kelly_size, max_size)
   ```

3. **Minimum**: Always >= $1.00

---

## Market Price Resolution

### Database Lookup Strategy

```rust
async fn get_market_price(&self, signal: &TradingSignal) -> Option<MarketPriceRow> {
    // 1. Query for market price from game_id
    let price = self.pool.query(
        "SELECT * FROM market_prices
         WHERE game_id = $1
         AND time > NOW() - $2::interval
         ORDER BY time DESC LIMIT 100",
        &[game_id, price_staleness_ttl]
    ).await?;

    // 2. Fuzzy match team with contract_team
    for row in price {
        let confidence = match_team_in_text(
            &signal.team,
            &row.contract_team,
            &signal.sport
        );

        if confidence >= TEAM_MATCH_MIN_CONFIDENCE {  // 0.7 default
            return Some(row);
        }

        if confidence >= 0.9 { break; }  // Early exit on high confidence
    }

    // 3. Fallback: Synthesize from signal
    MarketPriceRow {
        yes_bid: signal.market_prob - 0.02,
        yes_ask: signal.market_prob + 0.02,
        liquidity: signal.liquidity_available.unwrap_or(100.0),
        platform: "paper",
        time: Utc::now(),
        ...
    }
}
```

### Fallback Behavior

If no database price found but signal has `market_prob`, the service synthesizes a market price:
- Bid: `market_prob - 0.02`
- Ask: `market_prob + 0.02`
- Liquidity: Signal's `liquidity_available` or conservative $100

This allows trading to proceed even if market_prices table is temporarily unavailable.

---

## Async Background Tasks

### 1. ZMQ Signal Listener Loop

```rust
async fn zmq_listener_loop(state: Arc<Mutex<State>>, endpoint: &str) {
    loop {
        match SubSocket::new().connect(endpoint) {
            Ok(socket) => {
                socket.subscribe("signals.trade.*").await.ok();

                loop {
                    match timeout(Duration::from_secs(30), socket.recv()).await {
                        Ok(Ok(msg)) => {
                            let parts = msg.iter().collect::<Vec<_>>();
                            let payload = serde_json::from_slice(parts[1])?;

                            let mut state = state.lock().await;
                            state.handle_signal(payload).await.ok();
                        }
                        _ => continue,
                    }
                }
            }
            Err(_) => {
                sleep(Duration::from_secs(5)).await;  // Retry
            }
        }
    }
}
```

**Behavior**:
- Creates new SubSocket on each iteration (stateless reconnection)
- 30-second receive timeout (normal for low-traffic)
- Retries every 5 seconds on connection failure
- Logs errors but doesn't crash

### 2. Heartbeat Publisher Loop

Publishes every 10 seconds:

```json
{
  "service": "signal_processor_rust",
  "instance_id": "signal-processor-1",
  "status": "healthy",
  "timestamp": "2026-01-30T22:30:00Z",
  "checks": {
    "redis_ok": true,
    "db_ok": true
  },
  "metrics": {
    "signals_received": 1234,
    "signals_approved": 456,
    "signals_rejected": 778,
    "signals_rejected_insufficient_liquidity": 45,
    "db_pool_size": 10,
    "db_pool_idle": 7,
    "db_pool_active": 3
  }
}
```

**Channel**: `health:heartbeats` (Redis pub/sub)

### 3. Rule Update Subscriber

Listens on `feedback:rules` channel:

```rust
async fn rule_subscriber_loop(state: Arc<Mutex<State>>) {
    let subscription = redis.subscribe("feedback:rules").await?;

    for msg in subscription {
        if msg.get("type") == Some("rules_update") {
            let mut state = state.lock().await;
            state.load_rules_from_db().await.ok();
        }
    }
}
```

**Behavior**:
- Auto-reconnect on failure
- Reloads all active, non-expired rules from database
- Enables dynamic rule updates without service restart

### 4. In-Flight Cleanup Loop

Runs every 60 seconds:

```rust
async fn cleanup_loop(state: Arc<Mutex<State>>) {
    loop {
        sleep(Duration::from_secs(60)).await;

        let mut state = state.lock().await;
        let cutoff = Utc::now() - Duration::from_secs(300);  // 5 minutes
        state.in_flight.retain(|_, ts| ts > &cutoff);
    }
}
```

**Purpose**: Prevents unbounded memory growth in deduplication map

---

## State Management

### SignalProcessorState Structure

```rust
struct SignalProcessorState {
    // Configuration & Pools
    config: Config,
    pool: PgPool,                    // 10 max connections
    redis: RedisBus,                 // Auto-reconnecting

    // Execution mode selection
    execution_inline: bool,          // true = direct, false = ZMQ
    execution_engine: Option<Arc<ExecutionEngine>>,
    pnl_tracker: Option<Arc<DailyPnlTracker>>,

    // ZMQ transport
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,         // Atomic counter for messages

    // Metrics
    signal_count: u64,               // Total received
    approved_count: u64,             // Total approved
    rejected_counts: HashMap<String, u64>,  // By rejection reason

    // State maps
    team_cooldowns: HashMap<String, (DateTime, bool)>,  // "game_id:team" → (time, was_win)
    in_flight: HashMap<String, DateTime>,               // Deduplication

    // Dynamic rules
    rules: Vec<CachedRule>,          // Cached from DB
    rules_last_updated: DateTime,    // Last reload time
}
```

### Rejection Counter Categories

```
rejected_counts = {
    "edge": N,                  // Edge threshold not met
    "prob": N,                  // Probability bounds violated
    "duplicate": N,             // Same-side duplicate
    "no_market": N,             // Missing market price
    "cooldown": N,              // Team in cooldown
    "risk": N,                  // Risk checks failed
    "rule_blocked": N,          // Trading rule rejected
    "insufficient_liquidity": N // Below liquidity threshold
}
```

---

## Configuration & Environment

### Core Trading Thresholds

```env
MIN_EDGE_PCT=15.0              # Minimum edge percentage to trade
KELLY_FRACTION=0.25            # Fractional Kelly (0.25x recommended)
MAX_POSITION_PCT=10.0          # Max position as % of bankroll
MAX_BUY_PROB=0.95              # Don't buy near-certainties
MIN_SELL_PROB=0.05             # Don't sell near-impossibilities
ALLOW_HEDGING=false            # Allow opposite-side trades same game
```

### Risk Limits

```env
MAX_DAILY_LOSS=100.0           # Daily loss limit ($)
MAX_GAME_EXPOSURE=50.0         # Per-game exposure limit ($)
                               # -1 = disabled
MAX_SPORT_EXPOSURE=200.0       # Per-sport exposure limit ($)
WIN_COOLDOWN_SECONDS=180       # Cooldown after win (3 min)
LOSS_COOLDOWN_SECONDS=300      # Cooldown after loss (5 min)
```

### Market & Liquidity

```env
PRICE_STALENESS_TTL=30         # Price staleness threshold (seconds)
LIQUIDITY_MIN_THRESHOLD=10.0   # Default minimum liquidity ($)

# Platform-specific overrides
LIQUIDITY_MIN_THRESHOLD_KALSHI=50.0
LIQUIDITY_MIN_THRESHOLD_POLYMARKET=75.0
LIQUIDITY_MIN_THRESHOLD_PAPER=5.0

# Market-type-specific overrides
LIQUIDITY_MIN_THRESHOLD_SPORT=10.0
LIQUIDITY_MIN_THRESHOLD_CRYPTO=50.0

LIQUIDITY_MAX_POSITION_PCT=80.0  # Max % of available liquidity
```

### Other

```env
INITIAL_BANKROLL=1000.0        # Default starting balance
TEAM_MATCH_MIN_CONFIDENCE=0.7  # Team name matching threshold
MAX_LATENCY_MS=5000.0          # Max signal latency for monitoring
```

---

## Database Interactions

### Tables (Read-Only)

| Table | Purpose | Query Pattern |
|-------|---------|---------------|
| `market_prices` | Current market data | `WHERE game_id = ? AND time > NOW() - interval` |
| `paper_trades` | Open/closed positions | `WHERE game_id/sport = ? AND status IN ('open', 'closed')` |
| `bankroll` | Account balance | `SELECT current_balance ORDER BY updated_at DESC LIMIT 1` |
| `trading_rules` | Dynamic trade rules | `WHERE status = 'active' AND (expires_at IS NULL OR expires_at > NOW())` |

### Connection Pooling

```rust
let pool = PgPoolOptions::new()
    .max_connections(10)       // 10 concurrent connections
    .min_connections(2)        // Always keep 2 warm
    .acquire_timeout(Duration::from_secs(5))
    .create_pool(db_url)
    .await?;
```

**Rationale**: 7 parallel risk check queries + buffer = 10 connections ideal

### Query Optimization

- **Indexes**: `game_id`, `sport`, `status`, `game_id,status`, `sport,status`
- **Time range**: Queries only recent prices (`NOW() - 30s`)
- **Batch queries**: Risk checks run in parallel, not sequential

---

## Rule System

### Rule Structure

```rust
struct CachedRule {
    rule_id: String,
    rule_type: String,  // "reject", "override"
    conditions: HashMap<String, serde_json::Value>,  // Match criteria
    action: HashMap<String, serde_json::Value>,      // Action if matched
    expires_at: Option<DateTime>,                      // Auto-expire
}
```

### Condition Matching

Supports signal field comparisons:

```json
{
    "sport": "NBA",                    // Exact match
    "edge_lt": 10.0,                   // edge < 10
    "edge_gte": 5.0,                   // edge >= 5
    "direction": "Buy",                // Exact match
    "team": "Lakers"                   // Exact match (case-insensitive)
}
```

**Matched Fields**: `sport`, `signal_type`, `direction`, `edge_pct`, `model_prob`, `team`, `game_id`

**Operators**: `_lt` (less than), `_lte` (<=), `_gt` (>), `_gte` (>=)

### Actions

**Reject Action**:
```json
{
    "type": "reject",
    "reason": "Custom rejection message"
}
```
→ Signal blocked immediately, rejection counter incremented

**Override Action**:
```json
{
    "type": "override",
    "min_edge_pct": 25.0
}
```
→ Raises minimum edge threshold to 25% (still passes if edge >= 25%)

**Precedence**: Highest override wins if multiple rules match

### Rule Lifecycle

1. **Load**: At startup, query `trading_rules` table for active rules
2. **Cache**: Rules stored in-memory in `SignalProcessorState::rules`
3. **Update**: On `feedback:rules` notification, reload from database
4. **Expire**: Rules with `expires_at < NOW()` are skipped
5. **Match**: For each signal, check all rules and apply matching action

---

## Execution Modes

### 1. ZMQ-Only Transport (Default & Recommended)

```
signal_processor_rust              execution_service_rust
        │                                   ▲
        │ ExecutionRequest                  │
        │ (ZMQ :5559)                       │
        ├──────────────────────────────────→
        │
        │ (Async, decoupled)                │
        │                                   │ Execute
        │                                   │ (Paper or Live)
        │                                   │
        │                        ← ExecutionResult (Redis)
        │ (Optional feedback)               │
```

**Configuration**:
```env
EXECUTION_INLINE=0              # or unset (default)
ZMQ_PUB_ENDPOINT=tcp://*:5559
```

**Behavior**:
- Publishes ExecutionRequest to ZMQ
- Doesn't wait for execution_service to process
- execution_service subscribes and executes asynchronously
- Decoupled, scalable, recommended for production

**Advantages**:
- ✅ Low latency (only 60-120ms signal-to-request)
- ✅ Scalable (decoupled services)
- ✅ Resilient (no blocking)

**Disadvantages**:
- ❌ Can't see execution result in signal_processor
- ❌ Requires execution_service running

### 2. Inline Execution (Consolidated)

```
signal_processor_rust
   ├─ ExecutionEngine
   │  ├─ Check safeguards
   │  ├─ Call Kalshi/Polymarket API
   │  └─ Return ExecutionResult (sync)
   │
   └─ Publish result to Redis (async)
```

**Configuration**:
```env
EXECUTION_INLINE=1
```

**Behavior**:
- Calls `ExecutionEngine::execute()` directly
- Blocks signal processor waiting for result
- Result published to `execution:results` Redis channel
- P&L tracked in `DailyPnlTracker` inline

**Advantages**:
- ✅ Can see execution result immediately
- ✅ Single service (simpler for testing)

**Disadvantages**:
- ❌ Higher latency (300-600ms total)
- ❌ Blocking (queues up signals)
- ❌ Single point of failure

**Use Case**: Testing and development only; not recommended for production

---

## Performance Characteristics

### Latency Profile

```
Signal receipt on ZMQ     T+0ms
  ├─ Message parse        T+1ms
  ├─ Pre-trade filters    T+5ms
  │  └─ Database lookup
  ├─ Risk checks (parallel) T+50ms
  │  └─ 7 concurrent queries
  ├─ Position sizing      T+55ms
  ├─ Execution request    T+60ms
  │
  └─ ZMQ publish         T+65ms (60-120ms range)
       ↓
  execution_service subscribes & processes
  (can be 100-300ms later if batching)
```

**Total Signal-to-Execution**: 60-120ms (ZMQ mode) vs 300-600ms (sequential)

### Throughput

- **Typical**: 100 signals/minute (sports markets)
- **Peak**: 300+ signals/minute (high-volume matches)
- **Rejection rate**: 70-90% (high bar for execution)
- **Execution rate**: 10-30 trades/minute

### Memory Usage

- **Base**: ~20-30MB (code, config, shared libraries)
- **Per-market**: ~1-2MB (market_prices cache, state)
- **Stable**: Memory doesn't grow unbounded (cleanup loops prune maps)
- **Typical**: 50-80MB with 100+ markets

### Database Load

- **Risk check queries**: 7 parallel queries per approved signal
- **Pool utilization**: 2-4 active connections (out of 10 max)
- **Query time**: 30-50ms for 7 parallel queries
- **Connection efficiency**: High (connection pool reuse)

---

## Integration Points

### 1. Upstream: game_shard_rust

**Input**: ZMQ `signals.trade.*` on port 5558

**Message Format**:
```json
{
    "seq": 12345,
    "timestamp_ms": 1769812209000,
    "source": "game_shard_rust",
    "payload": {
        "signal_id": "sig_...",
        "game_id": "nfl_...",
        "sport": "NFL",
        "team": "Kansas City Chiefs",
        "direction": "Buy",
        "signal_type": "WinProbabilityShift",
        "edge_pct": 8.5,
        "model_prob": 0.62,
        "market_prob": 0.55,
        "kelly_fraction": 0.25,
        "liquidity_available": 1500.0,
        "platform": "kalshi",
        "created_at": "2026-01-30T22:30:00Z"
    }
}
```

### 2. Downstream: execution_service_rust

**Output**: ZMQ `execution.request.{request_id}` on port 5559

**Message Format**:
```json
{
    "seq": 456,
    "timestamp_ms": 1769812209065,
    "source": "signal_processor_rust",
    "payload": {
        "request_id": "req_...",
        "idempotency_key": "nfl_...:Kansas City Chiefs:Buy",
        "game_id": "nfl_...",
        "signal_id": "sig_...",
        "signal_type": "WinProbabilityShift",
        "side": "Yes",
        "limit_price": 0.555,
        "size": 45.0,
        "edge_pct": 8.5,
        "model_prob": 0.62,
        "market_prob": 0.55
    }
}
```

### 3. Database: PostgreSQL

**Tables**:
- Read `market_prices` for current bid/ask/liquidity
- Read `paper_trades` for exposure limits
- Read `bankroll` for available balance
- Read `trading_rules` for dynamic filtering

**Indexes**: `game_id`, `sport`, `status` (critical for performance)

### 4. Messaging: Redis

**Publish**:
- `health:heartbeats` (every 10 seconds)
- `notification:events` (on risk rejection)
- `execution:results` (if inline mode)

**Subscribe**:
- `feedback:rules` (dynamic rule updates)

---

## Troubleshooting Guide

### Issue: No signals being processed

**Diagnosis**:
```bash
# Check if ZMQ endpoint is reachable
docker logs signal_processor_rust | grep "connected to"

# Check if game_shard is publishing
docker logs game_shard_rust | grep "signals.trade"

# Check for ZMQ connection errors
docker logs signal_processor_rust | grep -i "zmq\|websocket\|subscribe"
```

**Common Causes**:
1. game_shard not running or ZMQ port wrong
2. Network connectivity (containers on different networks)
3. ZMQ SUB_ENDPOINT misconfigured

**Fix**:
```bash
# Verify endpoint is correct (check docker-compose)
docker compose logs signal_processor_rust | grep "ZMQ_SUB"

# Verify game_shard is publishing
docker compose exec game_shard bash -c "netstat -tuln | grep 5558"

# Restart signal_processor
docker compose restart signal_processor_rust
```

### Issue: All signals rejected with "insufficient_liquidity"

**Diagnosis**:
```bash
# Check liquidity thresholds
docker logs signal_processor_rust | grep -i "liquidity\|THRESHOLD"

# Check market prices in database
psql $DATABASE_URL -c "SELECT game_id, yes_bid_size, yes_ask_size FROM market_prices LIMIT 10"
```

**Common Causes**:
1. `LIQUIDITY_MIN_THRESHOLD` too high
2. Markets have no liquidity data (NULL in database)
3. Platform-specific threshold too high for market

**Fix**:
```bash
# Lower threshold temporarily
export LIQUIDITY_MIN_THRESHOLD=5.0

# Or set platform-specific lower threshold
export LIQUIDITY_MIN_THRESHOLD_POLYMARKET=25.0

# Restart with new config
docker compose up -d --force-recreate signal_processor_rust
```

### Issue: Most signals rejected with "edge"

**Diagnosis**:
```bash
# Check edge threshold
docker logs signal_processor_rust | grep "MIN_EDGE_PCT"

# Check actual edge values in signals
docker logs game_shard_rust | grep "edge" | head -20
```

**Common Causes**:
1. Market not efficient enough (real-world variation)
2. `MIN_EDGE_PCT` too high (15% default is aggressive)
3. Spread widening (especially late in game)

**Fix**:
```bash
# Lower edge threshold for testing
export MIN_EDGE_PCT=7.0

# Or enable rule override for specific markets
INSERT INTO trading_rules
  (rule_id, conditions, action, status)
VALUES
  ('lower_edge_xyz',
   '{"game_id": "nfl_..."}',
   '{"type": "override", "min_edge_pct": 10.0}',
   'active');
```

### Issue: Signal processor crashes

**Diagnosis**:
```bash
# Check logs for panic
docker logs signal_processor_rust | grep -i "panic\|error\|fatal"

# Check database connectivity
docker logs signal_processor_rust | grep -i "database\|pool\|connection"

# Check Redis connectivity
docker logs signal_processor_rust | grep -i "redis\|subscription"
```

**Common Causes**:
1. Database connection pool exhausted
2. Malformed signal from game_shard
3. Redis connection lost

**Fix**:
```bash
# Increase database pool size
export DB_POOL_SIZE=20

# Check database is healthy
psql $DATABASE_URL -c "SELECT version();"

# Check Redis is healthy
redis-cli ping

# Restart service
docker compose restart signal_processor_rust
```

### Issue: High latency (>300ms signal-to-execution)

**Diagnosis**:
```bash
# Check database query times
docker logs signal_processor_rust | grep -i "risk_check\|query.*ms"

# Check ZMQ publish latency
docker logs signal_processor_rust | grep -i "published.*ms"

# Check pool utilization
docker logs signal_processor_rust | grep -i "pool\|connections"
```

**Common Causes**:
1. Database slow (query > 100ms)
2. Connection pool exhausted (waiting for connection)
3. Inline execution mode (don't use in production)

**Fix**:
```bash
# Check database indexes
psql $DATABASE_URL -c "SELECT schemaname, tablename, indexname FROM pg_indexes WHERE tablename IN ('market_prices', 'paper_trades');"

# Add missing indexes if needed
CREATE INDEX IF NOT EXISTS idx_market_prices_game_id ON market_prices(game_id);
CREATE INDEX IF NOT EXISTS idx_paper_trades_sport ON paper_trades(sport);

# Ensure ZMQ-only mode (not inline)
export EXECUTION_INLINE=0

# Restart
docker compose restart signal_processor_rust
```

---

## Summary

The Signal Processor is a high-throughput, low-latency gatekeeper that ensures only high-quality, risk-compliant trades reach execution. With 7-stage filtering, parallel risk checks, and dynamic rules, it provides sophisticated control over which trading signals become executed trades.

**Key Strengths**:
- ✅ 80% latency reduction (parallel risk checks)
- ✅ Sophisticated multi-stage filtering
- ✅ Dynamic trading rules without service restart
- ✅ Comprehensive metrics and monitoring
- ✅ Resilient (auto-reconnect on failures)

**Production Checklist**:
- [ ] Set appropriate `MIN_EDGE_PCT` for your market
- [ ] Configure risk limits: `MAX_DAILY_LOSS`, `MAX_GAME_EXPOSURE`, `MAX_SPORT_EXPOSURE`
- [ ] Configure liquidity thresholds
- [ ] Ensure `EXECUTION_INLINE=0` (ZMQ mode)
- [ ] Verify database indexes exist and are used
- [ ] Monitor heartbeat on `health:heartbeats` Redis channel
- [ ] Alert on rejection rates (especially `insufficient_liquidity`)
- [ ] Review trading_rules table for active rules
- [ ] Test with low edge threshold first, then increase as needed

---

**Document Version**: 1.0
**Last Updated**: 2026-01-30
**Status**: ✅ Complete
