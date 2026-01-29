# Warnings Analysis: arbees_rust_core and game_shard_rust

This document analyzes all compiler warnings in the `arbees_rust_core` and `game_shard_rust` packages, categorizing them by recommended action.

---

## Summary

| Category | Count | Status |
|----------|-------|--------|
| Unused code (ready for future use) | 6 | **KEEP** - Infrastructure for multi-market expansion |
| Unused code (new implementation) | 3 | **REUSE** - Wire up in future phases |
| Dead code (legacy/deprecated) | ~~4~~ 0 | ✅ **REMOVED** - `select_best_price`, `find_team_price`, `publish_signal_zmq` deleted |
| Dependency warnings | 1 | ✅ **UPDATED** - `cargo update` run, some deps cannot be updated further without Cargo.toml changes |

---

## arbees_rust_core Warnings

### 1. `SimplePriceResponse` and `SimplePriceData` never constructed
**File:** `rust_core/src/clients/coingecko.rs:55-69`

```rust
struct SimplePriceResponse { ... }
struct SimplePriceData { ... }
```

**Analysis:** These structs were created to deserialize CoinGecko's `/simple/price` endpoint response but are not currently being used.

**Recommendation:** **KEEP** - These are infrastructure for the Crypto market integration. The `CryptoEventProvider` currently uses the `/coins/markets` endpoint, but the simple price endpoint could be useful for faster, lighter-weight price checks.

**Future Use:** When implementing real-time crypto price monitoring (Phase 7+), these structures will be needed for the high-frequency simple price API.

---

### 2. Fields `default_probability` and `mean_reversion_factor` never read
**File:** `rust_core/src/probability/politics.rs:20-22`

```rust
pub struct PoliticsProbabilityModel {
    default_probability: f64,      // Never read
    mean_reversion_factor: f64,    // Never read
}
```

**Analysis:** These fields are initialized in `new()` but the current `calculate_probability()` implementation doesn't use them - it relies on the `PoliticsStateData` from the event state instead.

**Recommendation:** **REUSE** - Update the `calculate_probability_legacy()` or `calculate_probability()` methods to use these configuration values:

```rust
// In calculate_probability():
if state.current_probability.is_none() {
    return Ok(self.default_probability);
}
// Apply mean reversion
let raw_prob = state.current_probability.unwrap();
let adjusted = raw_prob + self.mean_reversion_factor * (0.5 - raw_prob);
```

---

## game_shard_rust Warnings

### 3. Unused variable `db_pool` in `monitor_event`
**File:** `services/game_shard_rust/src/event_monitor.rs:84`

```rust
pub async fn monitor_event(
    ...
    db_pool: PgPool,  // Unused
    ...
)
```

**Analysis:** The `db_pool` is passed to `monitor_event()` but we don't yet insert event states into the database for non-sports markets.

**Recommendation:** **REUSE** - Add database persistence for non-sports event states:

```rust
// In the monitoring loop:
if let Ok(state) = provider.get_event_state(&event_id).await {
    // Add: Insert state into database
    if let Err(e) = arbees_rust_core::db::insert_from_event_state(&db_pool, &state).await {
        warn!("Failed to insert event state: {}", e);
    }
    // ... rest of logic
}
```

---

### 4. Variable `last_probability` assigned but never used
**File:** `services/game_shard_rust/src/event_monitor.rs:118, 208`

```rust
let mut last_probability: Option<f64> = None;
// ... later:
last_probability = Some(probability);  // Value never read
```

**Analysis:** This was added to track probability changes for momentum signals but isn't used yet.

**Recommendation:** **REUSE** - Implement probability change detection:

```rust
// Use for detecting significant probability shifts
if let Some(last_prob) = last_probability {
    let prob_change = (probability - last_prob).abs();
    if prob_change > 0.05 {  // 5% change threshold
        info!("Significant prob change for {}: {:.1}% → {:.1}%",
              event_id, last_prob * 100.0, probability * 100.0);
        // Could emit WinProbShift signal type
    }
}
last_probability = Some(probability);
```

---

### 5. Field `market_id` in `MarketPriceData` never read
**File:** `services/game_shard_rust/src/shard.rs:181`

```rust
pub struct MarketPriceData {
    pub market_id: String,  // Never read
    ...
}
```

**Analysis:** The `market_id` is stored but only `contract_team` is used for lookups.

**Recommendation:** **KEEP** - This field is valuable for:
- Execution service needs market_id to place orders
- Debugging/logging market-specific issues
- Future order book tracking per market

Consider using it in signal generation:
```rust
// In emit_event_signal or check_and_emit_signal:
reason: format!("... market_id={}", price.market_id),
```

---

### 6. Fields `context`, `last_home_win_prob`, `opening_home_prob` never read
**File:** `services/game_shard_rust/src/shard.rs:208-213`

```rust
struct GameEntry {
    context: GameContext,                    // Never read
    task: tokio::task::JoinHandle<()>,
    last_home_win_prob: Arc<RwLock<...>>,   // Never read
    opening_home_prob: Arc<RwLock<...>>,    // Never read
}
```

**Analysis:** These fields are stored in the entry but only `task` is actively used (for abort on removal).

**Recommendation:** **MIXED**
- `context` - **KEEP** - Useful for game listing/status API
- `last_home_win_prob` - **KEEP** - Used inside monitor_game, stored for external access
- `opening_home_prob` - **REMOVE or REUSE** - Was intended for team strength estimation but never implemented

**Future Use:** Expose via `get_game_context()` method:
```rust
pub async fn get_game_context(&self, game_id: &str) -> Option<GameContext> {
    let games = self.games.lock().await;
    games.get(game_id).map(|e| e.context.clone())
}
```

---

### 7. Field `metadata` in `ShardCommand` never read
**File:** `services/game_shard_rust/src/shard.rs:228`

```rust
struct ShardCommand {
    ...
    metadata: Option<serde_json::Value>,  // Never read
}
```

**Analysis:** Added for future extensibility but not implemented.

**Recommendation:** **KEEP** - This is forward-compatible infrastructure. Use cases:
- Passing crypto target price/date through command
- Custom polling intervals per event
- Event-specific configuration

---

### 8. Field `timestamp` in `IncomingMarketPrice` never read
**File:** `services/game_shard_rust/src/shard.rs:242`

```rust
struct IncomingMarketPrice {
    ...
    timestamp: Option<String>,  // Never read
}
```

**Analysis:** Price messages include a timestamp but we use `Utc::now()` for `MarketPriceData.timestamp` instead.

**Recommendation:** **REUSE** - Use the source timestamp for better staleness detection:

```rust
// In process_incoming_price():
let ts = incoming.timestamp
    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
    .map(|dt| dt.with_timezone(&Utc))
    .unwrap_or_else(Utc::now);
```

---

### 9. Methods `get_price_stats` and `publish_signal_zmq` never used
**File:** `services/game_shard_rust/src/shard.rs:356, 961`

```rust
pub fn get_price_stats(&self) -> PriceListenerStatsSnapshot { ... }
async fn publish_signal_zmq(&self, signal: &TradingSignal) { ... }
```

**Analysis:**
- `get_price_stats()` - Created for monitoring but not exposed
- `publish_signal_zmq()` - Instance method superseded by `publish_signal_zmq_fn()`

**Recommendation:**
- `get_price_stats()` - **KEEP** - Expose via health check endpoint or logging
- `publish_signal_zmq()` - **REMOVE** - Replaced by the free function variant

---

### 10. Functions `select_best_price` and `find_team_price` never used
**File:** `services/game_shard_rust/src/shard.rs:1502, 1549`

```rust
fn select_best_price<'a>(...) -> Option<...> { ... }
fn find_team_price<'a>(...) -> Option<&'a MarketPriceData> { ... }
```

**Analysis:** These were replaced by `find_team_prices()` which returns both Kalshi and Polymarket prices for cross-platform arbitrage detection.

**Recommendation:** **REMOVE** - These are legacy functions superseded by the newer multi-platform approach:
- `select_best_price` → Replaced by `select_best_platform_for_team()`
- `find_team_price` → Replaced by `find_team_prices()` (returns tuple)

---

## Dependency Warning

### 11. Future-incompatible code in redis v0.24.0 and sqlx-postgres v0.7.4

```
warning: the following packages contain code that will be rejected by a future version of Rust
```

**Recommendation:** **UPDATE** - Run periodic dependency updates:

```bash
cargo update
# Or specifically:
cargo update -p redis
cargo update -p sqlx
```

Check for newer versions that fix the compatibility issues.

---

## Action Summary

### Immediate Actions (Low Risk) ✅ COMPLETED
1. ~~Remove `select_best_price` and `find_team_price` functions~~ ✅
2. ~~Remove `publish_signal_zmq` instance method (keep the `_fn` variant)~~ ✅
3. ~~Update dependencies: `cargo update`~~ ✅

### Phase 7+ Actions (Requires Testing)
1. Wire up `db_pool` in `monitor_event` to persist non-sports events
2. Use `last_probability` for momentum signal detection
3. Use `metadata` field for event-specific configuration
4. Use `timestamp` from incoming price messages

### Keep As Infrastructure
1. `SimplePriceResponse` / `SimplePriceData` - CoinGecko simple price endpoint
2. `market_id` in `MarketPriceData` - For execution and debugging
3. `context` / `last_home_win_prob` in `GameEntry` - For external access
4. `get_price_stats()` - For health monitoring
5. `default_probability` / `mean_reversion_factor` - Future tuning

---

## Cleanup Status

**Deprecated code has been removed.** The following functions were deleted:
- `select_best_price()` - Superseded by `select_best_platform_for_team()`
- `find_team_price()` - Superseded by `find_team_prices()`
- `publish_signal_zmq()` instance method - Superseded by `publish_signal_zmq_fn()`

To verify current warning count:

```bash
cd "P:\petes_code\ClaudeCode\Arbees\services"
cargo check --package game_shard_rust 2>&1 | grep warning
```

Current remaining warnings: ~10 (infrastructure warnings, acceptable - marked as KEEP/REUSE above)
