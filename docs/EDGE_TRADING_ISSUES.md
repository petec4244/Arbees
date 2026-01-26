# Edge Trading System - Issues & Discrepancies

This document lists all issues and discrepancies found during the detailed analysis of the edge trading system.

## 🔴 Critical Issues (Must Fix Before Production)

### 1. Live Trading Not Implemented
- **Location**: `services/execution_service_rust/src/engine.rs:54-110`
- **Issue**: Real execution returns `Rejected` with "Real execution not implemented yet"
- **Impact**: System only works in paper trading mode
- **Code Reference**:
  ```rust
  Platform::Kalshi => {
      // TODO: Call Kalshi client place_order
      // For now return dummy
      return Ok(ExecutionResult {
          status: ExecutionStatus::Rejected,
          rejection_reason: Some("Real execution not implemented yet".to_string()),
          ...
      });
  }
  ```
- **Fix Required**: 
  - Implement `KalshiClient::place_order()` method
  - Implement `PolymarketClient::place_order()` method
  - Handle order placement errors and retries
  - Add order status polling for fills

### 2. Hardcoded Platform Selection
- **Location**: `services/game_shard_rust/src/shard.rs:668`
- **Issue**: `platform_buy` is hardcoded to `Platform::Polymarket`
- **Impact**: Cannot trade on Kalshi even if it has better prices or lower fees
- **Code Reference**:
  ```rust
  platform_buy: Some(Platform::Polymarket),  // Hardcoded!
  ```
- **Fix Required**:
  - Compare prices across platforms
  - Select platform with best price or lowest fees
  - Consider liquidity when selecting platform
  - Add platform selection logic based on market data

### 3. Liquidity Not Checked
- **Location**: `services/game_shard_rust/src/shard.rs:672`
- **Issue**: `liquidity_available` is hardcoded to `10000.0` with TODO comment
- **Impact**: May attempt to trade on markets with insufficient liquidity, causing slippage or failed fills
- **Code Reference**:
  ```rust
  liquidity_available: 10000.0, // TODO: Get actual liquidity
  ```
- **Fix Required**:
  - Extract liquidity from market price data
  - Check liquidity before generating signal
  - Reject signals if liquidity < position size
  - Add liquidity field to `MarketPriceData` struct

### 4. Cross-Market Arbitrage Not Executed
- **Location**: `rust_core/src/lib.rs:53-121` (detection exists)
- **Issue**: Arbitrage detection exists but signals are not generated for cross-market arbitrage opportunities
- **Impact**: Missing profitable risk-free arbitrage opportunities
- **Code Reference**: Detection logic exists in `find_cross_market_arbitrage()` but not called in signal generation
- **Fix Required**:
  - Add arbitrage signal generation in `game_shard_rust`
  - Compare prices across platforms (Kalshi vs Polymarket)
  - Generate signals when `total_cost < 1.0`
  - Handle dual-leg execution (buy YES on one platform, buy NO on other)

---

## 🟡 Medium Priority Issues

### 5. Team Matching Confidence Threshold Mismatch
- **Location**: 
  - `services/signal_processor_rust/src/main.rs:687` (requires >= 0.7)
  - `services/game_shard_rust/src/shard.rs:592-608` (fuzzy matching, no threshold)
- **Issue**: Signal processor requires `>= 0.7` confidence, but game_shard uses fuzzy matching without explicit threshold
- **Impact**: May generate signals for wrong teams, leading to incorrect trades
- **Fix Required**:
  - Standardize team matching logic across services
  - Use same matching function from `arbees_rust_core/src/utils/matching.rs`
  - Apply consistent confidence threshold (0.7)
  - Log low-confidence matches for review

### 6. Price Staleness Check Inconsistent
- **Location**: 
  - `services/signal_processor_rust/src/main.rs:647` (2 minutes)
  - `services/position_tracker_rust/src/main.rs:699` (30 seconds)
- **Issue**: Different staleness thresholds for price validation
- **Impact**: May use stale prices for execution or exit decisions
- **Fix Required**:
  - Standardize to single threshold (recommend 30 seconds)
  - Add configuration parameter `PRICE_STALENESS_TTL`
  - Validate prices at both entry and exit

### 7. Exit Price Calculation Doesn't Account for Slippage
- **Location**: `services/position_tracker_rust/src/main.rs:724-728`
- **Issue**: Uses `yes_bid` for BUY exits (selling) but doesn't account for slippage or order book depth
- **Impact**: Actual exit price may be worse than calculated, reducing profits
- **Code Reference**:
  ```rust
  let exec_price = match position.side {
      TradeSide::Buy => price.yes_bid,  // Sell at bid - no slippage buffer
      TradeSide::Sell => price.yes_ask, // Cover at ask - no slippage buffer
  };
  ```
- **Fix Required**:
  - Add slippage buffer (e.g., 0.5-1% of price)
  - Check order book depth before exit
  - Use more conservative prices for large positions
  - Add slippage tracking to P&L calculation

### 8. Fee Calculation Inconsistency
- **Location**: 
  - `services/execution_service_rust/src/engine.rs:38` (returns 0.0)
  - `services/position_tracker_rust/src/main.rs:532-538` (calculates fees)
- **Issue**: Entry fees not calculated in execution service, only exit fees calculated in position tracker
- **Impact**: P&L calculations may be inaccurate, risk checks may allow oversized positions
- **Fix Required**:
  - Calculate entry fees at execution time
  - Store entry fees in `ExecutionResult`
  - Use actual fees for position size validation
  - Ensure fees match platform fee schedules:
    - Kalshi: ~0.7% entry + ~0.7% exit = 1.4% total
    - Polymarket: ~2% entry + ~2% exit = 4% total

---

## 🟢 Low Priority Issues

### 9. Overtime Detection May Be Incorrect
- **Location**: `services/game_shard_rust/src/shard.rs:765-777`
- **Issue**: Overtime detection logic may not work correctly for all sports
- **Impact**: May skip signals during overtime when they could be profitable
- **Code Reference**:
  ```rust
  fn is_overtime(sport: Sport, period: u8) -> bool {
      match sport {
          Sport::NHL => period > 3,       // Regular NHL: 3 periods
          Sport::NBA => period > 4,       // Regular NBA: 4 quarters
          Sport::NFL => period > 4,       // Regular NFL: 4 quarters
          // ...
      }
  }
  ```
- **Fix Required**:
  - Verify period numbers for each sport
  - Handle overtime periods correctly (e.g., NFL OT is period 5+, not just >4)
  - Consider allowing signals in overtime with tighter risk controls

### 10. Signal Expiration Too Short
- **Location**: `services/game_shard_rust/src/shard.rs:680`
- **Issue**: Signals expire after 30 seconds but processing may take longer
- **Impact**: Valid signals may be rejected as expired
- **Code Reference**:
  ```rust
  expires_at: Some(Utc::now() + chrono::Duration::seconds(30)),
  ```
- **Fix Required**:
  - Increase expiration to 60-120 seconds
  - Check expiration at execution time, not signal generation
  - Add signal age tracking

### 11. Database Connection Pool Exhaustion Risk
- **Location**: Multiple services (all create pools with `max_connections=5`)
- **Issue**: Each service creates its own pool with limited connections
- **Impact**: May exhaust database connections under load
- **Fix Required**:
  - Use connection pooler (PgBouncer) in front of TimescaleDB
  - Increase `max_connections` per service
  - Monitor connection pool usage
  - Add connection pool metrics

### 12. Error Handling Incomplete
- **Location**: Multiple services
- **Issue**: Many errors are logged but not propagated or handled gracefully
- **Impact**: Services may continue in degraded state without alerting
- **Examples**:
  - `services/game_shard_rust/src/shard.rs:467` - Database insert errors only logged
  - `services/signal_processor_rust/src/main.rs:1176` - Parse errors only logged
- **Fix Required**:
  - Add circuit breakers for external services (ESPN, Redis, DB)
  - Implement retry logic with exponential backoff
  - Add error metrics and alerting
  - Graceful degradation (e.g., skip game if ESPN fails repeatedly)

### 13. Position Size Calculation Doesn't Account for Fees
- **Location**: `services/signal_processor_rust/src/main.rs:724-733`
- **Issue**: Position size calculated from balance but doesn't reserve funds for fees
- **Impact**: May attempt to trade more than available after fees
- **Fix Required**:
  - Reserve fee amount when calculating position size
  - Use `balance - (size * fee_rate)` for available funds
  - Add fee buffer to risk checks

### 14. No Order Book Depth Check
- **Location**: `services/signal_processor_rust/src/main.rs:633-722`
- **Issue**: Market price lookup doesn't check order book depth
- **Impact**: May attempt to trade larger than available liquidity
- **Fix Required**:
  - Check `yes_bid_size` and `yes_ask_size` from market data
  - Reject signals if position size > available liquidity
  - Add liquidity check to risk limits

### 15. Cooldown Logic May Block Profitable Trades
- **Location**: `services/signal_processor_rust/src/main.rs:585-607`
- **Issue**: Cooldowns apply to entire game, not specific team/market
- **Impact**: May block profitable trades on different teams in same game
- **Fix Required**:
  - Apply cooldowns per team, not per game
  - Or reduce cooldown duration
  - Add cooldown override for high-confidence signals

---

## Recommendations for Fix Priority

1. **Phase 1 (Before Live Trading)**:
   - Fix #1: Implement live trading execution
   - Fix #2: Platform selection logic
   - Fix #3: Liquidity checking
   - Fix #8: Fee calculation consistency

2. **Phase 2 (Stability)**:
   - Fix #5: Team matching standardization
   - Fix #6: Price staleness consistency
   - Fix #7: Slippage handling
   - Fix #12: Error handling improvements

3. **Phase 3 (Optimization)**:
   - Fix #4: Cross-market arbitrage execution
   - Fix #9: Overtime detection
   - Fix #10: Signal expiration
   - Fix #11: Connection pooling
   - Fix #13-15: Additional risk controls

---

## Testing Checklist

After fixing issues, test:

- [ ] Live trading execution on both Kalshi and Polymarket
- [ ] Platform selection based on prices/fees
- [ ] Liquidity checks prevent oversized trades
- [ ] Fee calculations match platform schedules
- [ ] Team matching works correctly across services
- [ ] Price staleness prevents stale executions
- [ ] Slippage handling improves exit prices
- [ ] Error handling gracefully degrades
- [ ] Connection pooling handles load
- [ ] Cross-market arbitrage executes correctly
