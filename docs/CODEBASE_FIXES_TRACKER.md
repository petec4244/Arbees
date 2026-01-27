# Arbees Codebase Fixes Tracker

**Started:** 2026-01-27
**Last Updated:** 2026-01-27

This document tracks the implementation progress of fixes identified in the comprehensive codebase review.

---

## Summary

| Priority | Total Issues | Completed | In Progress | Pending |
|----------|-------------|-----------|-------------|---------|
| P0 (Critical) | 5 | 5 | 0 | 0 |
| P1 (High) | 5 | 5 | 0 | 0 |
| P2 (Medium) | 5 | 5 | 0 | 0 |
| P3 (Backlog) | 6 | 6 | 0 | 0 |

---

## P0 — Critical Fixes (COMPLETED)

### P0-1: Fix Floating-Point Precision in Financial Calculations
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Using f64 for Kelly criterion and P&L calculations could cause accumulated rounding errors.

**Files Changed:**
| File | Change |
|------|--------|
| `rust_core/src/utils/money.rs` | NEW - Money utilities module with i64 cents-based arithmetic |
| `rust_core/src/utils/mod.rs` | Added `pub mod money;` export |
| `rust_core/src/models/mod.rs` | Updated `PaperTrade::pnl()` to use cents-based calculation |

**Implementation Details:**
- Created `Money` struct wrapping i64 cents for precision
- `PaperTrade::pnl()` now converts to cents, calculates, then converts back
- Added helper functions: `to_cents()`, `from_cents()`, `round_to_cents()`, `calculate_pnl_cents()`
- Added comprehensive tests for precision edge cases

**Verification:**
```bash
cargo test --package arbees_rust_core utils::money
```

---

### P0-2: Fix Redis Bus with ConnectionManager + Reconnection
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Single shared connection behind Mutex could become bottleneck; no reconnection logic if connection dropped.

**Files Changed:**
| File | Change |
|------|--------|
| `rust_core/src/redis/bus.rs` | Replaced Mutex<Connection> with ConnectionManager |
| `Cargo.toml` | Added `connection-manager` feature to redis dependency |

**Implementation Details:**
- `ConnectionManager` provides automatic reconnection and connection pooling
- Added retry logic with exponential backoff (3 attempts, 50ms→100ms→200ms)
- Added `RedisBusStats` for monitoring:
  - `messages_published` - successful publishes
  - `publish_failures` - failed publish attempts
  - `reconnect_attempts` - reconnection attempts
- Added `health_check()` method (sends PING)
- No mutex contention (lock-free)

**Verification:**
```bash
cargo check --package arbees_rust_core
```

---

### P0-3: Add Unique Constraint to market_prices Table
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No unique constraint on market_prices allowed duplicate entries, corrupting signal generation.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/021_p0_fixes.sql` | NEW - Migration with unique constraint + CHECK constraints |

**Implementation Details:**
- Created unique index: `idx_market_prices_unique` on `(time, market_id, platform, COALESCE(contract_team, ''))`
- Added CHECK constraints for data validity:
  - `chk_game_states_home_prob` - probability 0-1
  - `chk_game_states_away_prob` - probability 0-1
  - `chk_market_prices_bid` - bid 0-1
  - `chk_market_prices_ask` - ask 0-1
  - `chk_trading_signals_edge` - edge -100 to +100
  - `chk_trading_signals_model_prob` - probability 0-1
  - `chk_paper_trades_entry_price` - price 0-1
  - `chk_paper_trades_exit_price` - price 0-1
- Migration safely removes existing duplicates before creating constraint

**Deployment:**
```bash
psql -d arbees -f shared/arbees_shared/db/migrations/021_p0_fixes.sql
```

---

### P0-4: Implement Optimistic Locking on Bankroll Table
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Concurrent updates to bankroll (from signal_processor and position_tracker) could cause write-after-read race conditions, losing updates.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/021_p0_fixes.sql` | Added `version` column and `update_bankroll_atomic()` function |
| `services/position_tracker_rust/src/main.rs` | Updated `save_bankroll()` to use optimistic locking |

**Implementation Details:**
- Added `bankroll.version` column (INTEGER DEFAULT 1)
- Created `update_bankroll_atomic()` PostgreSQL function for atomic updates
- `save_bankroll()` now:
  1. Attempts UPDATE with `WHERE version = expected_version`
  2. On success, increments local version
  3. On conflict (0 rows affected), reloads state from DB and retries
  4. Up to 3 retry attempts with 50ms delay between
  5. Logs conflict details for debugging
- Added `bankroll_version` field to `PositionTrackerState`

**Verification:**
```bash
cargo check --package position_tracker_rust
```

---

### P0-5: Handle Price Parsing Failures with Counters/Alerts
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Failed price message parsing only logged at DEBUG level; no visibility into failure rates.

**Files Changed:**
| File | Change |
|------|--------|
| `services/game_shard_rust/src/shard.rs` | Added PriceListenerStats and improved error handling |

**Implementation Details:**
- Added `PriceListenerStats` struct with atomic counters:
  - `messages_received` - total messages
  - `messages_processed` - successfully processed
  - `parse_failures` - failed to parse (msgpack or JSON)
  - `no_liquidity_skipped` - skipped due to no liquidity
  - `no_team_skipped` - skipped due to missing contract_team
- Parse failures logged at WARN level (rate-limited: first 10, then every 100th)
- Payload preview logged for first 5 failures (for debugging)
- Stats logged every 60 seconds
- ERROR alert if parse failure rate exceeds 5%
- Added `get_price_stats()` method for external monitoring

**Verification:**
```bash
cargo check --package game_shard_rust
```

---

## P1 — High Priority Fixes (COMPLETED)

### P1-1: Handle Unwraps Properly in Client Initialization
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** `kalshi.rs:191` has unwrap that could panic on network errors.

**Files Changed:**
| File | Change |
|------|--------|
| `rust_core/src/clients/kalshi.rs` | Changed `new()` to return `Result<Self>`, fixed `with_credentials()` and `from_env()` |
| `services/execution_service_rust/src/engine.rs` | Updated to use `.expect()` on `KalshiClient::new()` |
| `services/market_discovery_rust/src/main.rs` | Updated to use `?` on `KalshiClient::new()` |
| `services/orchestrator_rust/src/main.rs` | Updated to use `?` on `KalshiClient::new()` |

**Implementation Details:**
- `KalshiClient::new()` now returns `Result<Self>` instead of `Self`
- HTTP client builder errors are propagated with context
- `Default` impl uses `.expect()` with clear error message
- All callers updated to handle the new Result type

---

### P1-2: Fix print() Calls → Logger in redis_bus.py
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** 4 print() calls in redis_bus.py instead of proper logging.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/messaging/redis_bus.py` | Added logger import, replaced all print() with logger.error() |

**Implementation Details:**
- Added `import logging` and `logger = logging.getLogger(__name__)`
- Line 286: Pattern subscription callback error → `logger.error(..., exc_info=True)`
- Line 294: Subscription callback error → `logger.error(..., exc_info=True)`
- Line 299: Listener error → `logger.error(..., exc_info=True)`
- Line 436: Typed subscriber error → `logger.error(..., exc_info=True)`
- All errors now include full stack traces via `exc_info=True`

---

### P1-3: Add CHECK Constraints for Probability Ranges
**Status:** ✅ COMPLETED (included in P0-3 migration)

**Note:** This was completed as part of migration 021_p0_fixes.sql

---

### P1-4: Synchronize price_staleness_secs Between Services
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** game_shard didn't check price staleness before using prices for signal generation.

**Files Changed:**
| File | Change |
|------|--------|
| `services/game_shard_rust/src/shard.rs` | Added `PRICE_STALENESS_TTL` config, updated `find_team_prices()` to filter stale |

**Implementation Details:**
- Added `price_staleness_secs` config reading from `PRICE_STALENESS_TTL` env var (default: 30)
- Updated `find_team_prices()` to accept `max_age_secs` parameter
- Stale prices (older than `max_age_secs`) are now filtered out before signal generation
- Uses same env var as `signal_processor_rust` and `position_tracker_rust`
- Already documented in `.env.example` as `PRICE_STALENESS_TTL=30.0`

---

### P1-5: Sanitize API Inputs (ESPN sport/league parameters)
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** ESPN URL constructed without validation of sport/league parameters.

**Files Changed:**
| File | Change |
|------|--------|
| `rust_core/src/clients/espn.rs` | Added input validation for sport/league parameters |

**Implementation Details:**
- Added `ALLOWED_SPORTS` constant: football, basketball, hockey, baseball, soccer, tennis, mma
- Added `ALLOWED_LEAGUES` constant: nfl, nba, nhl, mlb, mls, college-football, etc.
- Added `is_safe_path_segment()` to validate characters (alphanumeric, hyphen, period only)
- Added `validate_sport()` and `validate_league()` functions
- `get_games()` now validates inputs before constructing URL
- Polymarket client already safe: uses hardcoded tag_id lookup

**Note:** Polymarket client was already safe - sport parameter is validated through `tag_id_for_slug()` match statement which defaults to safe values.

---

## P2 — Medium Priority Fixes (COMPLETED)

### P2-1: Performance - Cache Normalized Team Aliases
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Team aliases were being rebuilt on every call to `get_team_aliases()`.

**Files Changed:**
| File | Change |
|------|--------|
| `rust_core/src/utils/matching.rs` | Added static `OnceLock` cache for team aliases |

**Implementation Details:**
- Added `TEAM_ALIASES_CACHE: OnceLock<SportAliasCache>` for lazy initialization
- Created `init_team_aliases_cache()` to pre-build aliases for all sports at startup
- Created `build_team_aliases(sport)` to build alias map for a single sport
- `get_team_aliases()` now returns `&'static AliasMap` (no allocations after first call)
- All 24 matching tests pass

---

### P2-2: Add Rust Service Integration Tests
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No integration tests for Rust services like execution engine.

**Files Changed:**
| File | Change |
|------|--------|
| `services/execution_service_rust/src/lib.rs` | NEW - Library entry point exposing engine module |
| `services/execution_service_rust/Cargo.toml` | Added lib and bin sections |
| `services/execution_service_rust/tests/paper_trading_test.rs` | NEW - 7 integration tests |

**Implementation Details:**
- Tests cover paper trading execution flow:
  - `test_paper_trading_execution_fills_order` - Basic execution
  - `test_paper_trading_calculates_kalshi_fees` - Fee calculation at 50%
  - `test_paper_trading_calculates_fees_at_extreme_prices` - Fee calc at 95%
  - `test_paper_trading_tracks_latency` - Latency tracking
  - `test_paper_trading_preserves_request_fields` - Field passthrough
  - `test_kalshi_live_disabled_in_paper_mode` - Mode checking
  - `test_polymarket_execution_rejected_without_clob` - CLOB rejection

**Verification:**
```bash
cargo test --package execution_service_rust
```

---

### P2-3: Add Database Migration Tests
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No tests to verify migration files are syntactically valid.

**Files Changed:**
| File | Change |
|------|--------|
| `tests/db/__init__.py` | NEW - Package marker |
| `tests/db/test_migrations.py` | NEW - 8 migration validation tests |

**Implementation Details:**
- Tests run without database connection (file-only validation):
  - `test_migrations_directory_exists` - Directory check
  - `test_migrations_follow_naming_convention` - NNN_name.sql format
  - `test_no_duplicate_migration_numbers` - No duplicate NNN prefixes
  - `test_migrations_are_valid_sql` - Contains SQL keywords
  - `test_migrations_have_balanced_parentheses` - Syntax check
  - `test_p0_fixes_migration_structure` - P0 migration specifics
  - `test_initial_migration_creates_core_tables` - 001_initial check
  - `test_migrations_ordered_sequence` - Strictly increasing numbers

**Verification:**
```bash
pytest tests/db/test_migrations.py -v
```

---

### P2-4: Implement VPN Failover for Polymarket
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** VPN container had no automatic failover if primary server failed.

**Files Changed:**
| File | Change |
|------|--------|
| `docker-compose.yml` | Added VPN failover config and improved healthcheck |
| `.env.example` | Added `VPN_COUNTRIES` documentation |

**Implementation Details:**
- Added `SERVER_COUNTRIES` env var for gluetun server rotation
- Default failover chain: Netherlands → Germany → Belgium → France
- Improved healthcheck: 30s interval, 5 retries, 45s start_period
- Added restart policy with exponential backoff (10s delay, 10 max attempts)
- Added public IP monitoring (PUBLICIP_API=ipinfo.io, 60s period)

---

### P2-5: Profile Memory Under Live Game Load
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No tooling to monitor memory usage during live games.

**Files Changed:**
| File | Change |
|------|--------|
| `scripts/profile_memory.py` | NEW - Memory profiling script |
| `docs/PERFORMANCE_OPTIMIZATIONS.md` | Added memory profiling section |

**Implementation Details:**
- Created `profile_memory.py` script with features:
  - Real-time container memory/CPU stats display
  - CSV output for analysis (`reports/memory_profile_{timestamp}.csv`)
  - Configurable duration, interval, threshold
  - Container filtering (`--containers game_shard`)
  - Alert mode for high memory usage
  - Quiet mode for automated monitoring
- Updated performance docs with:
  - Memory limits documentation (2GB limit, 512MB reservation)
  - Profiler usage examples
  - CSV analysis commands
  - Warning signs table
  - Rust memory debugging instructions
  - Key memory hotspots

**Verification:**
```bash
python scripts/profile_memory.py --duration 60 --interval 5
```

---

## P3 — Backlog (COMPLETED)

### P3-1: Recreate Continuous Aggregates with contract_team
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** Continuous aggregates `market_prices_hourly` didn't include `contract_team` column added in migration 020.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/022_p3_backlog.sql` | NEW - Recreated continuous aggregates with contract_team |

**Implementation Details:**
- Removed existing continuous aggregate policies
- Dropped and recreated `market_prices_hourly` with `contract_team` in GROUP BY
- Recreated `trading_performance_daily` for consistency
- Re-added refresh policies (1 hour / 1 day intervals)

---

### P3-2: Add Audit Triggers for Bankroll/Trade Updates
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No audit trail for bankroll or trade updates.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/022_p3_backlog.sql` | Added audit tables and triggers |

**Implementation Details:**
- Created `bankroll_audit` table with: operation, old/new balance, old/new piggybank, old/new version
- Created `paper_trades_audit` table with: operation, trade_id, old/new status, old/new pnl, old/new exit_price
- Created `audit_bankroll_changes()` trigger function
- Created `audit_paper_trades_changes()` trigger function
- Triggers fire on INSERT, UPDATE, DELETE
- Only logs actual changes (uses IS DISTINCT FROM)

---

### P3-3: Document Fargate Limitations and Alternatives
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No documentation explaining why VPN can't run on Fargate.

**Files Changed:**
| File | Change |
|------|--------|
| `docs/AWS_DEPLOYMENT.md` | Added Section 9: Fargate Limitations for VPN |

**Implementation Details:**
- Documented why Fargate doesn't support VPN (NET_ADMIN, /dev/net/tun)
- Added comparison table of alternatives (EC2, EU Proxy, EU Region)
- Provided EC2 task definition JSON example for VPN
- Included cost estimates and monitoring setup
- Documented failover configuration

---

### P3-4: Add Stress Tests for Concurrent Database Updates
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No tests to verify database handles concurrent updates correctly.

**Files Changed:**
| File | Change |
|------|--------|
| `tests/db/test_concurrent_updates.py` | NEW - Stress tests for concurrent DB operations |

**Implementation Details:**
- Tests require database connection (skipped if unavailable)
- `TestBankrollOptimisticLocking`:
  - `test_concurrent_bankroll_updates_with_locking` - 10 workers, 5 updates each
  - `test_bankroll_audit_trail` - Verifies audit trigger works
- `TestConcurrentPaperTrades`:
  - `test_concurrent_trade_insertions` - 5 workers, 20 trades each
- `TestConcurrentMarketPrices`:
  - `test_unique_constraint_prevents_duplicates` - Verifies unique index
  - `test_high_volume_price_inserts` - 5 workers, 50 prices each

**Verification:**
```bash
DATABASE_URL=postgresql://arbees:password@localhost:5432/arbees pytest tests/db/test_concurrent_updates.py -v
```

---

### P3-5: Monitor Retention Policy Execution
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No visibility into whether retention policies are running.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/022_p3_backlog.sql` | Added retention monitoring |

**Implementation Details:**
- Created `retention_policy_log` table: executed_at, table_name, rows_deleted, oldest_remaining, execution_time_ms, status
- Created `log_retention_execution()` helper function
- Created `v_retention_status` view joining hypertables with retention job info and log history

---

### P3-6: Add Deletion Audit Table
**Status:** ✅ COMPLETED (2026-01-27)

**Problem:** No record of deleted data for compliance/debugging.

**Files Changed:**
| File | Change |
|------|--------|
| `shared/arbees_shared/db/migrations/022_p3_backlog.sql` | Added deletion_audit table |

**Implementation Details:**
- Created `deletion_audit` table: table_name, record_id, deleted_data (JSONB), deleted_at, deleted_by, deletion_reason
- Paper trades trigger automatically logs to deletion_audit on DELETE
- Full record preserved as JSONB for recovery if needed
- Indexed by table_name and deleted_at

---

## Deployment Checklist

### Before Deploying P0 Fixes:
- [ ] Run migration 021: `psql -d arbees -f shared/arbees_shared/db/migrations/021_p0_fixes.sql`
- [ ] Rebuild Rust services: `cargo build --release`
- [ ] Verify bankroll has version column: `SELECT version FROM bankroll LIMIT 1;`
- [ ] Test Redis reconnection by restarting Redis during low traffic
- [ ] Monitor price_stats logs for parse failure rates

### After Deploying:
- [ ] Check Redis stats via `RedisBus::get_stats()`
- [ ] Check price listener stats via `GameShard::get_price_stats()`
- [ ] Monitor for bankroll version conflict warnings in logs
- [ ] Verify no duplicate market_prices entries: `SELECT COUNT(*) FROM market_prices GROUP BY time, market_id, platform, contract_team HAVING COUNT(*) > 1;`

### Before Deploying P3 Fixes:
- [ ] Run migration 022: `psql -d arbees -f shared/arbees_shared/db/migrations/022_p3_backlog.sql`
- [ ] Verify continuous aggregates recreated: `SELECT * FROM timescaledb_information.continuous_aggregates;`
- [ ] Verify audit tables exist: `\dt *_audit`
- [ ] Verify triggers active: `SELECT tgname FROM pg_trigger WHERE tgname LIKE 'trg_%';`

### After Deploying P3:
- [ ] Check audit tables are populating: `SELECT COUNT(*) FROM bankroll_audit;`
- [ ] Check retention monitoring: `SELECT * FROM v_retention_status;`
- [ ] Run concurrent stress tests (optional): `pytest tests/db/test_concurrent_updates.py -v`

---

## Change Log

| Date | Priority | Issue | Status | Notes |
|------|----------|-------|--------|-------|
| 2026-01-27 | P0-1 | Float precision | ✅ Done | Added money.rs module |
| 2026-01-27 | P0-2 | Redis reconnection | ✅ Done | Using ConnectionManager |
| 2026-01-27 | P0-3 | Market prices unique | ✅ Done | Migration 021 |
| 2026-01-27 | P0-4 | Bankroll locking | ✅ Done | Optimistic locking |
| 2026-01-27 | P0-5 | Price parse failures | ✅ Done | Stats + alerting |
| 2026-01-27 | P1-1 | Kalshi unwraps | ✅ Done | `new()` returns Result |
| 2026-01-27 | P1-2 | Python print→logger | ✅ Done | 4 print() → logger.error() |
| 2026-01-27 | P1-4 | Price staleness sync | ✅ Done | game_shard uses PRICE_STALENESS_TTL |
| 2026-01-27 | P1-5 | ESPN input sanitization | ✅ Done | Allowlist validation |
| 2026-01-27 | P2-1 | Team alias cache | ✅ Done | OnceLock static cache |
| 2026-01-27 | P2-2 | Rust integration tests | ✅ Done | 7 execution tests |
| 2026-01-27 | P2-3 | Migration tests | ✅ Done | 8 validation tests |
| 2026-01-27 | P2-4 | VPN failover | ✅ Done | gluetun server rotation |
| 2026-01-27 | P2-5 | Memory profiling | ✅ Done | Script + docs |
| 2026-01-27 | P3-1 | Continuous aggregates | ✅ Done | Added contract_team |
| 2026-01-27 | P3-2 | Audit triggers | ✅ Done | bankroll + paper_trades |
| 2026-01-27 | P3-3 | Fargate limitations | ✅ Done | AWS_DEPLOYMENT.md |
| 2026-01-27 | P3-4 | Stress tests | ✅ Done | Concurrent DB tests |
| 2026-01-27 | P3-5 | Retention monitoring | ✅ Done | retention_policy_log |
| 2026-01-27 | P3-6 | Deletion audit | ✅ Done | deletion_audit table |
