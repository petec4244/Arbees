# Arbees Codebase Review

**Date:** 2026-01-27
**Reviewer:** Claude Code (Automated Analysis)
**Scope:** Full codebase review including Rust core, Rust services, Python components, database schema, configuration, and testing

---

## Executive Summary

**Overall Grade: B+** — Production-quality architecture with solid design patterns, but several issues require attention before live trading at scale.

| Component | Grade | Key Strength | Critical Issue |
|-----------|-------|--------------|----------------|
| Rust Core Library | A- | Team matching (1786 LOC) | Floating-point precision in financial calcs |
| Rust Services | B+ | Lock-free execution tracking | Redis single connection bottleneck |
| Python Components | A- | Pydantic v2 type safety | 4 print() calls instead of logger |
| Database Schema | B | TimescaleDB hypertables | Missing unique constraints on market_prices |
| Configuration | B+ | Comprehensive env docs | Duplicate MIN_EDGE_PCT definitions |
| Testing | C+ | Good team matching tests | No Rust service tests, no migration tests |

**Risk Level: MEDIUM** — Most issues are fixable and non-critical, but several could impact trading accuracy or cause runtime panics.

---

## Table of Contents

1. [Rust Core Library](#1-rust-core-library)
2. [Rust Services](#2-rust-services)
3. [Python Components](#3-python-components)
4. [Database Schema](#4-database-schema)
5. [Configuration](#5-configuration)
6. [Testing Coverage](#6-testing-coverage)
7. [Critical Issues Summary](#7-critical-issues-summary)
8. [Prioritized Recommendations](#8-prioritized-recommendations)

---

## 1. Rust Core Library

### 1.1 Code Quality Issues

#### 1.1.1 Floating-Point Precision in Financial Calculations
**Severity:** CRITICAL
**Files:** `rust_core/src/models/mod.rs:235-275`, `rust_core/src/circuit_breaker.rs:67-68`

**Issue:** Using `f64` for financial calculations without explicit precision handling. This is especially risky for:
- Kelly criterion calculation (line 274 in models/mod.rs)
- Mean reversion Z-score calculations (line 367)
- Daily loss tracking in cents converted to dollars (circuit_breaker.rs:67-68)

**Example (models/mod.rs:274):**
```rust
((p * b - q) / b).max(0.0)  // Can lose precision
```

**Impact:**
- Rounding errors accumulate in backtesting/reporting
- Kelly fraction miscalculations could over/under-leverage positions
- Daily loss limits might miss edge cases near the limit

**Recommendation:**
- Use fixed-point arithmetic (i64 cents) for all financial values
- Store prices as basis points (1/10000) instead of decimals
- Only convert to f64 for display

#### 1.1.2 Unwrap Pattern in Production Code
**Severity:** MEDIUM
**File:** `rust_core/src/clients/kalshi.rs:191`

**Issue:**
```rust
.build()
.unwrap_or_else(|_| Client::new()),
```

This chain has two potential panics:
1. The `Client::new()` fallback also calls `.build()` internally
2. If both fail, creates a panic in a client initialization path

**Impact:** If network is severely broken, service could panic on startup.

**Recommendation:** Return `Result` from `new()` and handle errors properly.

#### 1.1.3 Unwrap in Circuit Breaker Error Handling
**Severity:** MEDIUM
**File:** `rust_core/src/circuit_breaker.rs:502, 515`

Multiple `.unwrap()` calls when setting Python dict items:
```rust
dict.set_item("daily_pnl", status.daily_pnl_cents as f64 / 100.0).unwrap();
```

**Impact:** If Python interop fails, panics instead of propagating error.

**Recommendation:** Replace all `.unwrap()` on `set_item` with `?` operator or `.map_err()`.

### 1.2 Error Handling Analysis

#### 1.2.1 Circuit Breaker Implementation ✓ EXCELLENT
**File:** `rust_core/src/circuit_breaker.rs`

**Strengths:**
- Proper state machine (Closed → Open → Half-Open)
- Configurable thresholds and recovery times
- Separate configs for different APIs (Kalshi more aggressive, ESPN more tolerant)
- Atomic operations for thread-safety

#### 1.2.2 API Client Error Handling ✓ GOOD
**File:** `rust_core/src/clients/kalshi.rs:280-290`

**Strengths:**
```rust
if !resp.status().is_success() {
    let status = resp.status();
    let error_text = resp.text().await.unwrap_or_else(|_| "Unknown error".to_string());
    return Err(anyhow!("Kalshi API error ({}): {}", status, error_text));
}
```
- Graceful fallback on error reading
- Uses `anyhow::Result` for proper error propagation
- Includes both status code and error text

#### 1.2.3 Redis Bus Single Connection
**Severity:** HIGH
**File:** `rust_core/src/redis/bus.rs:24, 29-30`

**Issue:**
```rust
let connection = client.get_async_connection().await?;
...
let mut conn = self.connection.lock().await;
conn.publish::<_, _, ()>(channel, payload).await?
```

**Problems:**
1. Single shared connection behind a Mutex can become a bottleneck
2. If a long-running operation locks the connection, other publishers block
3. No connection pooling or retry logic
4. `publish` is fire-and-forget but could silently fail if connection drops

**Impact:**
- Under high throughput, Redis operations could timeout
- No automatic reconnection if connection drops
- Could miss trading signals during network blips

**Recommendation:**
- Use `redis::aio::ConnectionManager` (has built-in pooling)
- Add automatic reconnection with exponential backoff
- Add metrics for connection health

### 1.3 Team Matching Logic ⭐ EXCELLENT
**File:** `rust_core/src/utils/matching.rs` (1786 lines)

This is the crown jewel of the codebase. Robust, well-tested, and handles edge cases carefully.

**Strengths:**

1. **Multi-layer matching strategy** (lines 706-823):
   - Exact phrase match (highest confidence)
   - Alias-based matching (sport-specific)
   - Mascot matching with shared mascot handling
   - Fuzzy matching only for long words with high threshold (0.95)

2. **Context validation** (lines 1271-1348):
   - Sport validation (prevents cross-league confusion)
   - Opponent validation
   - Score correlation checking
   - Weighted combination of factors

3. **Shared mascot detection** (lines 745-780):
   ```rust
   let shared_mascots = [
       "tigers", "wildcats", "bulldogs",
       "eagles", "cardinals", "panthers"
   ];
   if !shared_mascots.contains(&mascot) {
       // Can match on mascot alone
   } else {
       // Require additional context (e.g., city name)
   }
   ```
   This prevents matching "Florida Panthers" (NHL) to "Carolina Panthers" (NFL).

4. **Comprehensive test coverage** (lines 1433-1785):
   - Sport mismatch rejection (line 1473-1498)
   - Cross-league rejection for Panthers (line 1757-1784)
   - Score extraction and validation
   - Backward compatibility tests

**Minor Improvement:**
**Line 1039-1042 (parse_period_number):**
```rust
for c in p.chars() {
    if let Some(num) = c.to_digit(10) {
        return Some(num);
    }
}
```
This finds the FIRST digit, not necessarily the period number. E.g., "P20" would return 2, not 20.

### 1.4 API Client Design

#### 1.4.1 Kalshi Client
**File:** `rust_core/src/clients/kalshi.rs` (514 lines)

**Strengths:**
- RSA-PSS signature authentication correctly implemented (lines 220-239)
- Proper handling of API key + private key (optional for read-only)
- Environment variable loading with fallbacks (lines 157-197)
- Circuit breaker integration
- Timestamp generation for signature

**Issues:**
1. **Line 136:** API key truncation in logs could leak key portion
2. **Line 340:** Memory leak risk with `series_ticker` scope — if `sport` is None, `series_ticker` is uninitialized but pushed to params

#### 1.4.2 ESPN Client
**File:** `rust_core/src/clients/espn.rs` (227 lines)

**Strengths:**
- Clean circuit breaker integration
- Good error handling with logging
- Flexible configuration

**Issue (Line 101):** URL constructed without validation — no validation that `sport` and `league` don't contain path traversal chars.

#### 1.4.3 Polymarket Client
**File:** `rust_core/src/clients/polymarket.rs` (205 lines)

**Strengths:**
- Efficient pagination (lines 99-142)
- Good fallback tag ID handling (lines 102-105)
- Proxy support for EU geo-bypass

**Issues:**
1. **Line 81:** Silent failure on JSON parse — returns `Null` instead of error
2. **Line 125-126:** Error handling swallows details

### 1.5 Performance Concerns

#### 1.5.1 Matching Algorithm Efficiency
**File:** `rust_core/src/utils/matching.rs:644-646`

For each alias, calls `contains_phrase` which does O(n*m) comparison. With 30+ aliases per sport, this could be slow for 1000s of markets.

**Recommendation:** Cache normalized aliases or use trie structure for faster prefix matching.

#### 1.5.2 Arbitrage Detection Logic
**File:** `rust_core/src/lib.rs:56-124`

**Strengths:**
- Correct mathematical logic for cross-platform and same-platform arbs
- Uses proper bid/ask spread calculations
- Batch processing with rayon parallelization (line 427)

**Issue (Line 68-70):** Computes NO ask from YES bid — assumes symmetric spreads which may not always be true.

### 1.6 Concurrency & Atomicity

#### 1.6.1 Execution Tracker ✓ EXCELLENT
**File:** `rust_core/src/execution.rs`

Lock-free 512-bit atomic bitmask for deduplication:
```rust
let prev = self.in_flight[word_idx].fetch_or(mask, Ordering::SeqCst);
(prev & mask) == 0 // True if bit was not previously set
```

Uses `SeqCst` (sequential consistency) which is correct for a trading system where order matters.

#### 1.6.2 Atomic Orderbook
**File:** `rust_core/src/atomic_orderbook.rs`

Uses CAS (compare-and-swap) for lock-free updates. Good design.

### 1.7 Code Organization ✓ GOOD
**File:** `rust_core/src/lib.rs:15-32`

Clean separation:
```
- types: Core enums (Sport, Platform)
- win_prob: Win probability models
- clients: API clients (ESPN, Kalshi, Polymarket)
- models: Shared data structures
- redis: Message bus
- utils: Team matching, utilities
- circuit_breaker, execution, atomic_orderbook: Advanced features
```

**No circular dependencies detected.**

---

## 2. Rust Services

### 2.1 Service Architecture Overview

| Service | Purpose | Grade |
|---------|---------|-------|
| `orchestrator_rust` | Game discovery via ESPN, shard assignment | B+ |
| `game_shard_rust` | Live game state and price monitoring | B |
| `execution_service_rust` | Trade execution (paper/live) | B+ |
| `market_discovery_rust` | Market ID discovery | A- |
| `signal_processor_rust` | Signal generation | A- |
| `position_tracker_rust` | Position monitoring and exit logic | B |
| `notification_service_rust` | Notifications | B+ |

### 2.2 Orchestrator Service
**Location:** `services/orchestrator_rust/`

**Strengths:**
- Good separation into managers (GameManager, ShardManager, KalshiDiscoveryManager)
- Parallel ESPN game fetching using `futures_util::future::join_all()` (lines 63-75)
- Five independent async tasks for different concerns

**Issues:**

1. **Potential Message Loss - Market Discovery Listener** (main.rs:108-134)
   - Creates dedicated Redis connection without reconnection logic
   - If Redis connection drops, listener exits and won't recover
   - **Severity:** HIGH

2. **Race Condition in Game Assignments** (game_manager.rs:143-160)
   - `handle_shard_heartbeat()` removes assignments without checking if they're currently being processed
   - Between reading heartbeat and removing from assignments, a signal could reference deleted game
   - **Severity:** HIGH

3. **Unbounded Task Spawning** (main.rs:62-74)
   - `run_discovery_cycle()` spawns one task per ESPN client without throttling
   - **Severity:** MEDIUM

### 2.3 Game Shard Service
**Location:** `services/game_shard_rust/`

**Strengths:**
- Smart use of Arc<RwLock> for concurrent price updates (shard.rs:100-101)
- Sophisticated debouncing for signal spam prevention (shard.rs:516-520)
- Cross-platform arbitrage checking using SIMD (shard.rs:625-661)
- Good conditional logic for skipping signals (0-0 games, overtime, etc.)

**Critical Issues:**

1. **Message Loss in Price Listener Loop** (shard.rs:389-468)
   - No error handling for msgpack/JSON parsing failures beyond debug logging
   - If price message fails to parse, entire game's price state becomes stale
   - **Severity:** HIGH

2. **Atomicity Issue in Probability Tracking** (shard.rs:592-595)
   - Reads `old_prob`, updates it, then uses both values with temporal gap
   - **Severity:** MEDIUM

3. **Per-Game Task Never Completes** (shard.rs:220-248)
   - Spawned tasks run infinite loops with no shutdown mechanism
   - When `remove_game()` calls `entry.task.abort()`, pending Redis publishes may be lost
   - **Severity:** MEDIUM

4. **Price Data Race Condition** (shard.rs:459-463)
   - Multiple price updates for same (game, team, platform) can overwrite concurrently
   - **Severity:** MEDIUM

### 2.4 Execution Service
**Location:** `services/execution_service_rust/`

**Strengths:**
- Clean fee calculation logic matching Kalshi's atomic orderbook (engine.rs:16-32)
- Proper error handling in main loop (main.rs:35-49)
- Realistic paper trading with accurate platform fees

**Issues:**
1. **Polymarket Live Trading Deferred** (engine.rs:125-136) — Returns hardcoded rejection
2. **No Kalshi Error Handling** (engine.rs:121-123) — Credentials checked lazily

### 2.5 Market Discovery Service
**Location:** `services/market_discovery_rust/`

**Strengths:**
- Comprehensive sport keyword validation (main.rs:43-147)
- Smart caching with TTL for Polymarket/Kalshi markets (main.rs:258-261)
- Parallel request listener without blocking (main.rs:380-425)

**Issues:**
1. **Duplicate Responses from Team Matching RPC** (main.rs:495-523) — No idempotency tracking
2. **Cache Stampede on TTL Expiry** (main.rs:658-679) — No locking mechanism prevents thundering herd

### 2.6 Signal Processor Service
**Location:** `services/signal_processor_rust/`

**Strengths:**
- Outstanding parallel risk check implementation using `tokio::join!()` (main.rs:437-451)
- Comprehensive rule system with caching and expiry (main.rs:269-305)
- Proper in-flight deduplication with cleanup (main.rs:1057-1060)
- Clear game cooldown tracking with win/loss differentiation

**Issues:**
1. **Rule Loading Race Condition** (main.rs:1180-1186)
2. **In-Flight Cleanup Missing Expiration** (main.rs:1057-1060)
3. **Price Staleness Check Inconsistent** (main.rs:128-131 vs game_shard.rs:517) — Configurable vs hardcoded

### 2.7 Position Tracker Service
**Location:** `services/position_tracker_rust/`

**Strengths:**
- Sophisticated parallel price lookups with cache + DB fallback (main.rs:716-812)
- Correct P&L calculation with fee breakdown (main.rs:535-556)
- Piggybank savings mechanism with configurable percentage (main.rs:561-568)
- Orphan sweep with game state validation (main.rs:995-1087)

**Issues:**
1. **Price Cache Update Race** (main.rs:1254-1258)
2. **Bankroll Write-After-Read Race** (main.rs:205-225) — **CRITICAL**
3. **Game Ended Handler Doesn't Verify Trades Are Open** (main.rs:464-482)

### 2.8 Redis Pub/Sub Patterns

**Message Loss Risks:**

1. **No Message Persistence** — Redis pub/sub is fire-and-forget; unsubscribed listeners miss messages
2. **Pattern Subscription Ordering** — Single pubsub connection for both psubscribe and subscribe
3. **Implicit Ordering Assumptions** — No guarantees if execution happens out of order

**Recommendation:** Use Redis Streams instead of pub/sub for persistence

### 2.9 Concurrency & Tokio Patterns

**Strengths:**
- Good use of `Arc<RwLock<T>>` for shared state
- Proper async/await throughout
- No blocking operations in async contexts

**Issues:**
1. **Unbounded RwLock Contention** — Write locks held for entire signal emission
2. **No Timeout on Lock Acquisition** — Deadlocked task never releases lock
3. **Silent Task Failures** — Spawned tasks only log errors; main loop doesn't know tasks died

### 2.10 Health Monitoring

**Design:** Each service publishes heartbeat every 10 seconds to `health:heartbeats` channel.

**Issues:**
1. **Incomplete Health Checks** — Always reports "healthy" even if DB connection failed
2. **No Service Dependencies Tracked** — If Redis dies, heartbeat publishes to nowhere
3. **TTL-Based Liveness Insufficient** — Race condition between expiry check and observation

---

## 3. Python Components

### 3.1 Strengths

#### 3.1.1 Type Safety ✓ EXCELLENT
**Files:** All models (game.py, trade.py, signal.py, market.py)

- Comprehensive use of Pydantic v2 with proper field validation
- Example: `PaperTrade` fields use `Field(ge=0.0, le=1.0)` for price constraints
- All async functions have proper type hints
- No bare `Any` types in critical paths

#### 3.1.2 Data Validation & Models ✓ EXCELLENT
**Files:** `shared/arbees_shared/models/*`

- Frozen Pydantic models prevent accidental mutations
- Computed fields avoid duplication (e.g., `time_remaining_seconds`, `pnl`)
- Proper enum usage for Sports, SignalTypes, TradeStatus
- Field constraints prevent invalid states

#### 3.1.3 Async/Await Correctness ✓ GOOD
**Files:** redis_bus.py, connection.py, team_matching/client.py

- Proper use of `asynccontextmanager` for resource management
- Connection pooling correctly implemented
- No blocking calls in async functions
- Timeout handling with `asyncio.wait_for()`

#### 3.1.4 Database Safety ✓ EXCELLENT
**File:** `shared/arbees_shared/db/connection.py`

- Parameterized queries throughout — NO SQL injection risks
- All values passed as separate arguments ($1, $2, etc.)
- Proper date parsing with ISO format validation

#### 3.1.5 Risk Controller Logic ✓ EXCELLENT
**File:** `shared/arbees_shared/risk/controller.py`

Multi-layered risk management:
- Daily loss limits
- Per-game exposure limits
- Per-sport exposure limits
- Position correlation detection
- Circuit breaker with cooldown
- Latency-based trading gate

#### 3.1.6 Fee Calculation ✓ EXCELLENT
**File:** `shared/arbees_shared/utils/fees.py`

- Comprehensive fee schedules for all platforms
- Methods for net edge calculation after fees
- Proper validation of minimum trade sizes
- Round-trip cost calculations for arbitrage

### 3.2 Issues Found

#### 3.2.1 Print Instead of Logger
**Severity:** HIGH
**Files:** `shared/arbees_shared/messaging/redis_bus.py:286, 294, 299, 436`

**Issue:**
```python
print(f"Callback error: {e}")
```

**Problem:** Uses `print()` instead of logger:
- Gets lost in container stdout buffering
- Not captured in structured logging systems
- Makes debugging production issues harder

**Recommendation:**
```python
logger.error(f"Callback error: {e}", exc_info=True)
```

#### 3.2.2 Potential Division by Zero
**Severity:** MEDIUM
**File:** `shared/arbees_shared/risk/controller.py:514`

**Issue:**
```python
pnl_pct = (pnl / risk * 100) if risk > 0 else 0
```

**Recommendation:** Use epsilon: `if risk > 0.0001`

#### 3.2.3 Missing staticmethod Decorator
**Severity:** LOW
**File:** `shared/arbees_shared/team_matching/client.py:176`

`_cache_key` is a static method but defined as instance method.

#### 3.2.4 Loose Exception Handling
**Severity:** MEDIUM
**File:** `shared/arbees_shared/messaging/redis_bus.py:277-278`

```python
except Exception:
    continue  # Silent skip!
```

Silently skips malformed messages without logging.

#### 3.2.5 Race Condition in Cache Eviction
**Severity:** LOW
**File:** `shared/arbees_shared/team_matching/client.py:201-206`

Between `len()` check and eviction, another async task could add an entry.

### 3.3 Code Quality Metrics

| Aspect | Rating | Notes |
|--------|--------|-------|
| Type Hints | 95% | Comprehensive coverage |
| Error Handling | 85% | Good but 4 print() calls instead of logger |
| Async Correctness | 90% | Very good; minor race condition in cache |
| Security | 95% | Parameterized queries, validated inputs |
| Documentation | 85% | Good docstrings |
| Code Organization | 90% | Clear separation of concerns |

---

## 4. Database Schema

### 4.1 Schema Structure Overview

**Location:** `shared/arbees_shared/db/migrations/`

**Migration Files (20 total):**
- `001_initial.sql` — Core schema with 8 tables + TimescaleDB setup
- `013-020` — Incremental enhancements

#### Core Tables

| Table | Type | Purpose | Hypertable |
|-------|------|---------|-----------|
| `games` | Relational | Game metadata, status tracking | No |
| `market_mappings` | Relational | Game-to-market ID mapping | No |
| `bankroll` | Relational | Account balance tracking | No |
| `game_states` | Time-series | Live game snapshots | Yes |
| `plays` | Time-series | Individual play events | Yes |
| `market_prices` | Time-series | Price snapshots | Yes |
| `trading_signals` | Time-series | Generated trading signals | Yes |
| `arbitrage_opportunities` | Time-series | Detected arb opportunities | Yes |
| `paper_trades` | Time-series | Trade records with P&L | Yes |
| `latency_metrics` | Time-series | Performance tracking | Yes |

### 4.2 Critical Schema Issues

#### 4.2.1 Insufficient Constraints in market_prices
**Severity:** CRITICAL
**Location:** `001_initial.sql:171-186`

**Issue:** No unique constraint. TimescaleDB hypertables have `time` as part of the time-partition key, but no explicit uniqueness constraint.

**Risk:** Duplicate price entries for same market at same timestamp not prevented.

**Recommendation:** Add composite unique constraint on `(time, market_id, platform, contract_team)`

#### 4.2.2 Loose Data Type for Probabilities
**Severity:** HIGH
**Location:** Multiple tables

**Issue:** `DECIMAL(5, 4)` allows value range 0.0000-9.9999. Probabilities must be 0.0-1.0.

**Recommendation:** Add `CHECK (value >= 0 AND value <= 1)`

#### 4.2.3 No Constraints on Edge Percentage
**Severity:** MEDIUM
**Location:** `trading_signals.edge_pct`, `arbitrage_opportunities.edge_pct`

**Issue:** Uses `DECIMAL(6, 3)` with no range check. Allows -999.999 to +999.999.

**Recommendation:** Add `CHECK (edge_pct >= -100 AND edge_pct <= 100)`

#### 4.2.4 Loose Enum Handling in Archive Tables
**Severity:** MEDIUM
**Location:** `014_archive_tables.sql:50, 103, 123`

**Issue:** Archives use `VARCHAR(50)` instead of enum types for `signal_type`, `outcome`.

### 4.3 High Priority Findings

1. **Missing Index on trading_signals.executed** — Partial index exists but no index for executed=true queries
2. **No Retention Policy Violations Detection** — No alert mechanism if data retention fails
3. **Continuous Aggregates Missing Error Handling** — `market_prices_hourly` doesn't include `contract_team`
4. **No Sequence/Version Control for Bankroll Updates** — Multiple concurrent writes can conflict

### 4.4 Good Implementations ✓

- TimescaleDB hypertables properly created
- Appropriate retention policies (30-day detail)
- Continuous aggregates for hourly and daily rollups
- Helper functions for common queries
- Foreign key cascade deletes
- Sport enum properly defined (all 10 sports)

---

## 5. Configuration

### 5.1 Docker Compose Configuration
**Location:** `docker-compose.yml` (442 lines)

#### Architecture

**Profiles:**
- `full` — Complete stack (default for development)
- `vpn` — VPN + Polymarket monitor only
- `legacy` — Deprecated services

**Service Flow:**
```
timescaledb (5432) <- Database
redis (6379)       <- Message bus
↓
orchestrator       <- Game discovery (ESPN)
↓
market_discovery   <- Market ID lookup
↓
game_shard         <- Live game state & prices
↓
signal_processor   <- Signal generation
execution_service  <- Trade execution
position_tracker   <- Exit monitoring
↓
notification       <- Notifications
api (8000)        <- REST/WebSocket API
frontend (3000)   <- React UI
```

#### Issues

1. **Resource Limits** (Lines 6-10)
   - Max 2GB per service may be insufficient during live games
   - No CPU limits defined

2. **Volume Management** (Lines 433-436)
   - No backup volumes defined for timescaledb

3. **Health Checks**
   - Good for infrastructure (timescaledb, redis)
   - Missing for Rust services

### 5.2 Environment Variables
**Location:** `.env.example` (434 lines)

#### Key Trading Parameters

| Parameter | Value | Impact |
|-----------|-------|--------|
| `MIN_EDGE_PCT` | 15.0% | Signal generation filter |
| `MAX_DAILY_LOSS` | 100.0 | Circuit breaker |
| `MAX_SPORT_EXPOSURE` | 1000.0 | Per-sport budget |
| `MAX_GAME_EXPOSURE` | -1 | Disabled |
| `KELLY_FRACTION` | 0.15 | Position sizing |
| `PAPER_TRADING` | 1 | Mode flag |

#### Sport-Specific Stop-Loss

| Sport | Stop Loss | Logic |
|-------|-----------|-------|
| NBA/NCAAB | 3% | High-scoring, frequent changes |
| NFL/NCAAF | 5% | Medium pace |
| NHL | 7% | Low-scoring |
| MLB | 6% | Low-scoring but innings can swing |
| MLS/Soccer | 7% | Low-scoring |
| Tennis | 4% | Point-by-point volatility |
| MMA | 8% | Binary outcome |

#### Issues

1. **Duplicate MIN_EDGE_PCT** — Defined at lines 166 and 195
2. **Legacy Configuration Cruft** — InfluxDB, Grafana, Discord settings (lines 389-433)
3. **Missing Documentation** — `MARKET_DISCOVERY_MODE` options not explained

### 5.3 Multi-Region Deployment

**Current Architecture:**
- **US-East-1:** Kalshi market access, TimescaleDB, Redis
- **EU-Central-1:** Polymarket geo-bypass via VPN

**Issues:**
1. **Single Database Instance** — All writes to single region
2. **VPN Tunnel as SPOF** — EU Polymarket access depends on single VPN container
3. **Cross-Region Redis Latency** — EU VPN → US Redis could have 100ms+ latency
4. **AWS ECS Fargate Limitations** — Doesn't support NET_ADMIN for VPN

---

## 6. Testing Coverage

### 6.1 Test Summary

| Test File | Type | Focus |
|-----------|------|-------|
| `test_critical_fixes.py` | Smoke | Syntax & imports |
| `tests/unit/test_team_matching_client.py` | Unit | Team matching RPC |
| `tests/integration/test_team_matching_e2e.py` | Integration | SignalProcessor & PositionTracker |
| `Polymarket-Kalshi-Arbitrage-bot/tests/integration_tests.rs` | Integration | Position tracking, P&L |

### 6.2 Critical Test Gaps

| Gap | Risk | Priority |
|-----|------|----------|
| No Rust service tests | Critical services untested | P0 |
| No migration rollback tests | Schema changes untested | P0 |
| No circuit breaker failover tests | Can't verify risk controls | P1 |
| No concurrent bankroll tests | Race conditions undetected | P1 |
| No VPN failover tests | Polymarket access SPOF | P1 |
| No signal generation unit tests | Edge calculations untested | P2 |
| No futures lifecycle E2E test | Futures tables unused | P2 |

### 6.3 Test Infrastructure Issues

1. **No pytest.ini** — Missing configuration
2. **No .coveragerc** — No coverage tracking
3. **No Docker test database** — Tests require real instance

---

## 7. Critical Issues Summary

### Severity: CRITICAL (Must Fix)

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| 1 | Floating-point financial precision | `models/mod.rs:274`, `circuit_breaker.rs:67` | Kelly & loss calculations inaccurate |
| 2 | Redis single connection bottleneck | `redis/bus.rs:24` | Could miss signals under load |
| 3 | Market prices duplicate prevention | `001_initial.sql:171-186` | Silent data corruption |
| 4 | Concurrent bankroll updates | `position_tracker.rs:205-225` | Balance tracking errors |
| 5 | Silent price message drops | `game_shard.rs:418-427` | Stale prices lead to bad trades |

### Severity: HIGH (Should Fix Soon)

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| 6 | Unwrap in client init | `kalshi.rs:191` | Panic on network errors |
| 7 | Race in game assignments | `game_manager.rs:143-160` | Crash when signal references deleted game |
| 8 | Cache stampede on TTL | `market_discovery.rs:653-679` | API rate limit exceeded |
| 9 | Rule loading race | `signal_processor.rs:1180-1186` | New rules not applied until reload |
| 10 | Price staleness mismatch | signal_processor vs game_shard | Accept stale prices |
| 11 | Print instead of logger | `redis_bus.py:286, 294, 299, 436` | Debugging production issues harder |
| 12 | Probability constraints missing | All DECIMAL(5,4) columns | Invalid values allowed |

### Severity: MEDIUM

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| 13 | Uninitialized series_ticker | `kalshi.rs:340` | Panic if sport is None |
| 14 | ESPN URL not sanitized | `espn.rs:101` | Potential path traversal |
| 15 | Polymarket silent JSON parse | `polymarket.rs:81` | Format changes ignored |
| 16 | Period number parsing greedy | `matching.rs:1039` | "P20" returns 2 not 20 |
| 17 | Archive table type safety | `014_archive_tables.sql` | Type safety lost after archival |
| 18 | Duplicate MIN_EDGE_PCT | `.env.example:166, 195` | Inconsistent updates |
| 19 | No Rust service health checks | All Rust services | Can't verify service health |

---

## 8. Prioritized Recommendations

### P0 — This Week (Critical for Trading)

1. **Fix floating-point precision in financial calculations**
   - Switch all financial values to i64 cents throughout
   - Only use f64 for display/reports
   - Audit all calculations in models/mod.rs and circuit_breaker.rs

2. **Fix Redis bus**
   - Replace with `redis::aio::ConnectionManager`
   - Add reconnection logic with exponential backoff
   - Add metrics/monitoring

3. **Add unique constraint to market_prices**
   ```sql
   ALTER TABLE market_prices ADD CONSTRAINT market_prices_unique
   UNIQUE (time, market_id, platform, contract_team);
   ```

4. **Implement optimistic locking on bankroll**
   - Add version column
   - Check version on update

5. **Handle price parsing failures**
   - Add dead letter queue or counter
   - Alert on parsing failure rate

### P1 — Next Sprint

6. **Handle unwraps properly**
   - Client initialization should return Result
   - All Python dict operations need proper error handling
   - Test failure scenarios

7. **Fix print() calls → logger in redis_bus.py**
   - Replace all 4 print() calls with proper logging
   - Add exc_info=True for stack traces

8. **Add CHECK constraints for probability ranges**
   ```sql
   ALTER TABLE game_states ADD CONSTRAINT home_win_prob_range
   CHECK (home_win_prob >= 0 AND home_win_prob <= 1);
   ```

9. **Synchronize price_staleness_secs between services**
   - Use same env var in signal_processor and game_shard
   - Document the setting

10. **Sanitize API inputs**
    - Validate ESPN sport/league parameters
    - Validate Polymarket filter parameters

### P2 — Following Sprint

11. **Performance optimization**
    - Cache normalized team aliases
    - Consider trie for faster fuzzy matching
    - Profile batch_scan_arbitrage under load

12. **Add Rust service integration tests**
    - game_shard_rust tests
    - orchestrator_rust tests
    - signal_processor_rust tests

13. **Add database migration tests**
    - Test retention policy deletions
    - Validate hypertable compression
    - Test continuous aggregate refresh

14. **Implement VPN failover**
    - Multi-region VPN or AWS Client VPN
    - Add health checks for VPN container

15. **Profile memory under live game load**
    - game_shard_rust with MAX_GAMES_PER_SHARD=20
    - Set per-service limits based on profiling

### P3 — Backlog

16. Recreate continuous aggregates with contract_team dimension
17. Add audit triggers for bankroll/trade updates
18. Document Fargate limitations and alternatives
19. Add stress tests for concurrent database updates
20. Monitor retention policy execution
21. Add deletion audit table

---

## Appendix A: File Reference Guide

### Critical Files to Review

| Category | File | Lines of Interest |
|----------|------|-------------------|
| Financial calcs | `rust_core/src/models/mod.rs` | 235-275 |
| Redis bus | `rust_core/src/redis/bus.rs` | 24, 29-30 |
| Team matching | `rust_core/src/utils/matching.rs` | 706-823, 1271-1348 |
| Risk controller | `shared/arbees_shared/risk/controller.py` | Full file |
| Schema | `shared/arbees_shared/db/migrations/001_initial.sql` | 171-186 |
| Config | `docker-compose.yml` | 6-10, 433-436 |
| Env vars | `.env.example` | 165-195, 259-268 |

### Service Entry Points

| Service | Main File |
|---------|-----------|
| Orchestrator | `services/orchestrator_rust/src/main.rs` |
| Game Shard | `services/game_shard_rust/src/shard.rs` |
| Execution | `services/execution_service_rust/src/main.rs` |
| Market Discovery | `services/market_discovery_rust/src/main.rs` |
| Signal Processor | `services/signal_processor_rust/src/main.rs` |
| Position Tracker | `services/position_tracker_rust/src/main.rs` |
| Notification | `services/notification_service_rust/src/main.rs` |

---

## Appendix B: What's Working Well

✅ **Team matching logic** — Sophisticated, well-tested, handles edge cases
✅ **Arbitrage detection** — Mathematically correct
✅ **Circuit breaker design** — Proper state machine, good for trading risk
✅ **Execution tracking** — Lock-free atomic design is clever and correct
✅ **Error handling** — Generally good use of `anyhow::Result` (Rust) and type hints (Python)
✅ **Module organization** — Clean separation of concerns
✅ **Risk management** — Multi-layered controls (daily loss, exposure, correlation, latency)
✅ **Pydantic v2 models** — Frozen, validated, computed fields
✅ **SQL injection prevention** — All queries parameterized
✅ **Fee calculations** — Comprehensive and accurate
✅ **Tests for team matching** — 1400+ lines of test code

---

## Appendix C: Estimated Fix Times

| Priority | Issue Count | Estimated Effort |
|----------|-------------|------------------|
| P0 (Critical) | 5 | 2-3 days |
| P1 (High) | 5 | 1 week |
| P2 (Medium) | 5 | 1-2 weeks |
| P3 (Backlog) | 6 | Ongoing |

**Total for production-ready:** ~2-3 weeks of focused work

---

*End of Review*
