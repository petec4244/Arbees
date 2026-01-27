# Reference Bot vs Arbees: File Mapping

**Purpose**: Map Polymarket-Kalshi-Arbitrage-bot files to Arbees equivalents
**Date**: 2026-01-27
**Status**: ✅ All Files Mapped

---

## Quick Reference

| Status | Meaning |
|--------|---------|
| ✅ **COMPLETE** | Fully implemented and tested |
| 🔄 **ENHANCED** | Implemented with improvements |
| ➕ **ADDED** | New functionality not in reference bot |
| ❌ **MISSING** | Not implemented (none found) |

---

## Reference Bot Structure

```
Polymarket-Kalshi-Arbitrage-bot/
├── scripts/
│   └── build_sports_cache.py    # Team name cache generation
└── src/
    ├── circuit_breakers.rs       # API failure detection
    ├── execution.rs              # Order execution logic
    ├── kalshi.rs                 # Kalshi API client
    ├── polymarket_clob.rs        # Polymarket CLOB client
    ├── polymarket.rs             # Polymarket Gamma API client
    ├── position_tracker.rs       # P&L and position tracking
    ├── discovery.rs              # Market discovery and team matching
    ├── Cache.rs                  # In-memory price cache
    ├── lib.rs                    # Core library
    ├── main.rs                   # Single binary entry point
    └── types.rs                  # Type definitions
```

---

## File-by-File Mapping

### 1. scripts/build_sports_cache.py

**Reference Bot**: Pre-builds static team name cache for matching

**Arbees Equivalent**:
- 🔄 **ENHANCED**: [rust_core/src/team_cache.rs](../rust_core/src/team_cache.rs)
- 🔄 **ENHANCED**: [rust_core/src/utils/matching.rs](../rust_core/src/utils/matching.rs)
- ➕ **ADDED**: Redis-based team matching RPC ([shared/arbees_shared/team_matching/](../shared/arbees_shared/team_matching/))

**Improvements**:
- Dynamic loading (no pre-build required)
- Fuzzy matching with confidence scores
- Opponent validation to prevent false positives
- Redis RPC for distributed matching

**Status**: ✅ **COMPLETE** (better than reference)

---

### 2. src/circuit_breakers.rs

**Reference Bot**: API failure detection with open/close thresholds

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/circuit_breaker.rs](../rust_core/src/circuit_breaker.rs)

**Improvements**:
- Configurable thresholds (`ApiCircuitBreakerConfig`)
- Per-API circuit breakers (Kalshi, Polymarket)
- Rate limits (429) bypass circuit breaker (don't count as failures)

**Key Code**:
```rust
pub struct ApiCircuitBreakerConfig {
    pub failure_threshold: u32,      // Open after N failures
    pub timeout_seconds: u64,        // Stay open for N seconds
    pub reset_timeout_seconds: u64,  // Reset after N seconds success
}
```

**Status**: ✅ **COMPLETE** (matches reference + improvements)

---

### 3. src/execution.rs

**Reference Bot**: Order execution and deduplication logic

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/execution.rs](../rust_core/src/execution.rs)
- ✅ **COMPLETE**: [services/execution_service_rust/](../services/execution_service_rust/)

**Improvements**:
- Split into library + dedicated microservice
- IOC order support (added 2026-01-27)
- Paper trading mode
- Database persistence (TimescaleDB)
- Fee calculation and tracking
- Latency measurement

**Key Features**:
- `ExecutionTracker` for deduplication
- `place_ioc_order()` for immediate-or-cancel orders
- Paper trade simulation with realistic fills
- P&L tracking in database

**Status**: ✅ **COMPLETE** (enhanced with IOC orders)

---

### 4. src/kalshi.rs

**Reference Bot**: Kalshi API client with IOC order support

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/clients/kalshi.rs](../rust_core/src/clients/kalshi.rs)

**Improvements**:
- ✅ IOC orders implemented (2026-01-27)
- ✅ Atomic order ID generation (2026-01-27)
- ✅ Exponential backoff for rate limits (2026-01-27)
- 🔄 Rate limits bypass circuit breaker
- ➕ Helper methods: `is_filled()`, `is_partial()`, `filled_count()`
- ➕ WebSocket support ([markets/kalshi/ws_client.py](../markets/kalshi/ws_client.py))

**Key Methods**:
```rust
// IOC order placement (NEW: 2026-01-27)
pub async fn place_ioc_order(&self, ticker: &str, side: &str,
                              price: f64, quantity: i32) -> Result<KalshiOrder>

// Order ID generation (NEW: 2026-01-27)
fn generate_order_id() -> String  // Format: "arb{timestamp}{counter}"

// Rate limit handling (NEW: 2026-01-27)
async fn authenticated_request(...) -> Result<Value>  // Exponential backoff
```

**Status**: ✅ **COMPLETE** (matches reference + improvements)

---

### 5. src/polymarket_clob.rs

**Reference Bot**: Polymarket CLOB (Central Limit Order Book) client

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/clients/polymarket_clob.rs](../rust_core/src/clients/polymarket_clob.rs)

**Improvements**:
- ➕ VPN integration (only polymarket_monitor needs VPN)
- ➕ WebSocket support ([markets/polymarket/ws_client.py](../markets/polymarket/ws_client.py))
- ➕ Hybrid client (CLOB + Gamma API)

**Status**: ✅ **COMPLETE** (matches reference + VPN isolation)

---

### 6. src/polymarket.rs

**Reference Bot**: Polymarket Gamma API client

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/clients/polymarket.rs](../rust_core/src/clients/polymarket.rs)

**Improvements**:
- Unified with CLOB client
- Public API (no VPN required for market discovery)
- Event filtering and pagination

**Status**: ✅ **COMPLETE** (matches reference)

---

### 7. src/position_tracker.rs

**Reference Bot**: Position and P&L tracking

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/position_tracker.rs](../rust_core/src/position_tracker.rs)
- ✅ **COMPLETE**: [services/position_tracker_rust/](../services/position_tracker_rust/)

**Improvements**:
- Split into library + dedicated microservice
- Database persistence (paper_trades, bankroll tables)
- Piggybank balance tracking
- Historical P&L analysis
- Position limits enforcement

**Key Features**:
```rust
pub struct ArbPosition {
    pub legs: Vec<PositionLeg>,       // Both sides of arbitrage
    pub realized_pnl: f64,            // Settled profit/loss
    pub unrealized_pnl: f64,          // Current mark-to-market
    pub total_fees: f64,              // Transaction costs
}
```

**Status**: ✅ **COMPLETE** (enhanced with persistence)

---

### 8. src/discovery.rs

**Reference Bot**: Market discovery and team matching

**Arbees Equivalent**:
- 🔄 **ENHANCED**: [services/market_discovery_rust/](../services/market_discovery_rust/)
- 🔄 **ENHANCED**: [services/orchestrator_rust/src/managers/kalshi_discovery.rs](../services/orchestrator_rust/src/managers/kalshi_discovery.rs)
- ➕ **ADDED**: [rust_core/src/utils/matching.rs](../rust_core/src/utils/matching.rs)

**Improvements**:
- Dedicated microservice (not inline in main process)
- Redis RPC for distributed matching
- Fuzzy team matching with confidence scores
- Sport-specific market caching (5min TTL)
- Aggressive refresh on missing series (30s)
- Opponent validation to prevent false matches

**Key Features**:
```rust
// Team matching with confidence
pub fn match_team_in_text(team: &str, text: &str, sport: &str) -> Option<(bool, f64)>

// Enhanced matching with opponent validation
pub fn match_teams_with_context(
    home_team: &str,
    away_team: &str,
    text: &str,
    sport: &str,
    expected_home_score: Option<u16>,
    expected_away_score: Option<u16>,
) -> (bool, f64)
```

**Status**: ✅ **COMPLETE** (significantly enhanced)

---

### 9. src/Cache.rs

**Reference Bot**: Lock-free in-memory price cache

**Arbees Equivalent**:
- 🔄 **ENHANCED**: [rust_core/src/atomic_orderbook.rs](../rust_core/src/atomic_orderbook.rs)
- ➕ **ADDED**: [rust_core/src/team_cache.rs](../rust_core/src/team_cache.rs)
- ➕ **ADDED**: Redis (for cross-service state)

**Improvements**:
- Atomic orderbook for lock-free price updates
- Redis pub/sub for distributed state
- Team name cache for matching
- Market ID cache with TTL

**Key Features**:
```rust
// Lock-free atomic orderbook
pub struct AtomicOrderbook {
    yes_bid: AtomicU64,
    yes_ask: AtomicU64,
    timestamp_ms: AtomicI64,
}
```

**Status**: ✅ **COMPLETE** (enhanced with Redis + atomic ops)

---

### 10. src/lib.rs

**Reference Bot**: Core library (arbitrage detection, win probability)

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/lib.rs](../rust_core/src/lib.rs)

**Improvements**:
- Python bindings via PyO3 (`arbees_core` Python module)
- SIMD-accelerated arbitrage detection
- Enhanced win probability models
- Pregame probability blending (NEW: 2026-01-27)

**Key Features**:
```rust
// Cross-market arbitrage
pub fn find_cross_market_arbitrage(...) -> Vec<ArbitrageOpportunity>

// Same-platform arbitrage
pub fn find_same_platform_arbitrage(...) -> Option<ArbitrageOpportunity>

// Win probability calculation
pub fn calculate_win_probability(state: &GameState, for_home: bool) -> f64

// SIMD batch processing
pub fn simd_batch_scan(...) -> Vec<ArbitrageOpportunity>
```

**Status**: ✅ **COMPLETE** (matches reference + SIMD + Python bindings)

---

### 11. src/main.rs

**Reference Bot**: Single binary entry point, orchestrates all components

**Arbees Equivalent**:
- 🔄 **DISTRIBUTED**: Split into 12+ microservices

**Why Different**:
Reference bot runs everything in one process (main.rs orchestrates):
- WebSocket listeners
- Arbitrage detection
- Order execution
- Position tracking

Arbees splits into services:
1. [services/orchestrator_rust/](../services/orchestrator_rust/) - Game discovery
2. [services/market_discovery_rust/](../services/market_discovery_rust/) - Market ID lookup
3. [services/game_shard_rust/](../services/game_shard_rust/) - Game state aggregation
4. [services/signal_processor_rust/](../services/signal_processor_rust/) - Signal generation
5. [services/execution_service_rust/](../services/execution_service_rust/) - Order execution
6. [services/position_tracker_rust/](../services/position_tracker_rust/) - P&L tracking
7. [services/kalshi_monitor/](../services/kalshi_monitor/) - Kalshi WebSocket
8. [services/polymarket_monitor/](../services/polymarket_monitor/) - Polymarket WebSocket
9. [services/api/](../services/api/) - REST API
10. [frontend/](../frontend/) - React dashboard

**Trade-off**:
- ❌ Higher latency (~60ms overhead from Redis hops)
- ✅ Better fault isolation (one service crash doesn't kill system)
- ✅ Horizontal scaling (multiple game shards)
- ✅ Easier debugging (service-level logs)
- ✅ Language flexibility (Rust + Python)

**Status**: 🔄 **ENHANCED** (architectural improvement)

---

### 12. src/types.rs

**Reference Bot**: Type definitions (Market, Order, Position, etc.)

**Arbees Equivalent**:
- ✅ **COMPLETE**: [rust_core/src/types.rs](../rust_core/src/types.rs)
- ✅ **COMPLETE**: [rust_core/src/models/mod.rs](../rust_core/src/models/mod.rs)

**Improvements**:
- Richer type system (GameState, SportSpecificState)
- Python bindings (PyO3)
- Database models (SQLx compatible)
- JSON serialization (serde)

**Key Types**:
```rust
pub enum Sport { NFL, NBA, NHL, MLB, NCAAF, NCAAB, MLS, Soccer, Tennis, MMA }
pub enum Platform { Kalshi, Polymarket, Sportsbook, Paper }
pub struct GameState { /* ... */ }
pub struct MarketPrice { /* ... */ }
pub struct ArbitrageOpportunity { /* ... */ }
pub struct TradingSignal { /* ... */ }
```

**Status**: ✅ **COMPLETE** (enhanced with more types)

---

## Summary Table

| Reference File | Status | Arbees Location | Notes |
|----------------|--------|-----------------|-------|
| `scripts/build_sports_cache.py` | 🔄 **ENHANCED** | [rust_core/src/team_cache.rs](../rust_core/src/team_cache.rs) | Dynamic + Redis RPC |
| `src/circuit_breakers.rs` | ✅ **COMPLETE** | [rust_core/src/circuit_breaker.rs](../rust_core/src/circuit_breaker.rs) | Rate limit bypass |
| `src/execution.rs` | ✅ **COMPLETE** | [rust_core/src/execution.rs](../rust_core/src/execution.rs) + [service](../services/execution_service_rust/) | IOC orders added |
| `src/kalshi.rs` | ✅ **COMPLETE** | [rust_core/src/clients/kalshi.rs](../rust_core/src/clients/kalshi.rs) | IOC + rate limits |
| `src/polymarket_clob.rs` | ✅ **COMPLETE** | [rust_core/src/clients/polymarket_clob.rs](../rust_core/src/clients/polymarket_clob.rs) | VPN isolation |
| `src/polymarket.rs` | ✅ **COMPLETE** | [rust_core/src/clients/polymarket.rs](../rust_core/src/clients/polymarket.rs) | Gamma API |
| `src/position_tracker.rs` | ✅ **COMPLETE** | [rust_core/src/position_tracker.rs](../rust_core/src/position_tracker.rs) + [service](../services/position_tracker_rust/) | DB persistence |
| `src/discovery.rs` | 🔄 **ENHANCED** | [services/market_discovery_rust/](../services/market_discovery_rust/) | Fuzzy matching |
| `src/Cache.rs` | 🔄 **ENHANCED** | [rust_core/src/atomic_orderbook.rs](../rust_core/src/atomic_orderbook.rs) | Lock-free + Redis |
| `src/lib.rs` | ✅ **COMPLETE** | [rust_core/src/lib.rs](../rust_core/src/lib.rs) | Python bindings |
| `src/main.rs` | 🔄 **DISTRIBUTED** | [services/](../services/) | Microservices |
| `src/types.rs` | ✅ **COMPLETE** | [rust_core/src/types.rs](../rust_core/src/types.rs) | Enhanced types |

**Score**: 12/12 files mapped (100%)

---

## Verdict

### ✅ All Reference Bot Functionality Implemented

**Critical Features**:
- ✅ IOC orders
- ✅ Rate limit handling
- ✅ Order ID generation
- ✅ Circuit breakers
- ✅ Position tracking
- ✅ Market discovery
- ✅ Team matching
- ✅ Arbitrage detection
- ✅ Win probability

**Architectural Improvements**:
- ✅ Microservices (fault isolation)
- ✅ Redis pub/sub (distributed state)
- ✅ TimescaleDB (persistence)
- ✅ FastAPI + React (dashboard)
- ✅ VPN isolation (minimal scope)
- ✅ WebSocket monitors (dedicated services)

**Trade-offs**:
- ⚠️ 60ms latency overhead (acceptable)
- ⚠️ Higher complexity (manageable)

---

## Related Documents

- 📊 [ARCHITECTURE_COMPARISON_REPORT.md](./ARCHITECTURE_COMPARISON_REPORT.md) - Detailed architectural analysis
- ✅ [OPERATIONAL_READINESS_CHECKLIST.md](./OPERATIONAL_READINESS_CHECKLIST.md) - Testing guide
- 📝 [EXECUTIVE_SUMMARY.md](./EXECUTIVE_SUMMARY.md) - High-level overview
- 🔧 [KALSHI_IMPLEMENTATION_ANALYSIS_CORRECTED.md](./KALSHI_IMPLEMENTATION_ANALYSIS_CORRECTED.md) - IOC specification

---

**Next Action**: Begin 48-hour paper trading test (see [Operational Readiness Checklist](./OPERATIONAL_READINESS_CHECKLIST.md))

**Status**: ✅ Complete Mapping
**Last Updated**: 2026-01-27
