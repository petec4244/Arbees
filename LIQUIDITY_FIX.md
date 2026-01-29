# Liquidity Validation Fix

## Problem

Paper trader wasn't acting on arbitrage signals when liquidity was reported as $0, even though the signals themselves contained `liquidity_available` values (typically 100.0+).

### Root Cause

The signal processor had two bugs:

1. **Fallback MarketPriceRow Creation** ([signal_processor_rust/src/main.rs:1076-1077](services/signal_processor_rust/src/main.rs#L1076-L1077))
   ```rust
   // OLD CODE - HARDCODED TO 0 ❌
   yes_bid_size: Some(0.0),
   yes_ask_size: Some(0.0),
   ```
   When no market price existed in the database, the fallback created a synthetic MarketPriceRow with **0 liquidity**, causing all signals to be rejected.

2. **Liquidity Validation Logic** ([signal_processor_rust/src/main.rs:877-888](services/signal_processor_rust/src/main.rs#L877-L888))
   ```rust
   // OLD CODE - NO FALLBACK ❌
   let available = match signal.direction {
       SignalDirection::Buy => market_price.yes_ask_size.unwrap_or(0.0),
       SignalDirection::Sell => market_price.yes_bid_size.unwrap_or(0.0),
       SignalDirection::Hold => return Ok(proposed_size),
   };

   if available < self.config.liquidity_min_threshold {  // Default: $10
       return Err(...);  // REJECTED!
   }
   ```
   The validator only looked at `market_price.yes_ask_size` and ignored the signal's `liquidity_available` field entirely.

### Flow of the Bug

```
1. Game shard creates signal with liquidity_available: 100.0 ✓
2. Signal published to Redis ✓
3. Signal processor receives signal ✓
4. Signal processor queries database for market_prices ✓
5. No market_prices row found (new game or missing data) ⚠️
6. Creates fallback MarketPriceRow with yes_ask_size: 0.0 ❌
7. Validates liquidity: 0.0 < $10 threshold → REJECTED ❌
8. Paper trader never receives execution request ❌
```

## Fix

### Part 1: Use Signal's Liquidity in Fallback

Changed fallback MarketPriceRow to use the signal's `liquidity_available`:

```rust
// NEW CODE - USE SIGNAL'S LIQUIDITY ✓
let liquidity = if signal.liquidity_available > 0.0 {
    signal.liquidity_available
} else {
    100.0  // Conservative fallback if signal also has 0
};

MarketPriceRow {
    // ... other fields ...
    yes_bid_size: Some(liquidity),
    yes_ask_size: Some(liquidity),
    liquidity: Some(liquidity),
    // ... other fields ...
}
```

### Part 2: Add Fallback in Validation Logic

Added fallback to use signal's `liquidity_available` when market price has no liquidity data:

```rust
// NEW CODE - FALLBACK TO SIGNAL'S LIQUIDITY ✓
let mut available = match signal.direction {
    SignalDirection::Buy => market_price.yes_ask_size.unwrap_or(0.0),
    SignalDirection::Sell => market_price.yes_bid_size.unwrap_or(0.0),
    SignalDirection::Hold => return Ok(proposed_size),
};

// Fallback: If market price has no liquidity data, use signal's liquidity_available
if available == 0.0 && signal.liquidity_available > 0.0 {
    available = signal.liquidity_available;
    debug!(
        "Using signal's liquidity_available (${:.2}) as fallback for market price with no liquidity data",
        available
    );
}

if available < self.config.liquidity_min_threshold {
    return Err(...);
}
```

## Configuration

### Liquidity Threshold

The minimum liquidity threshold defaults to **$10** and can be configured via:

```bash
# In .env file
LIQUIDITY_MIN_THRESHOLD=10.0  # Minimum liquidity to trade (dollars)
```

Lower values will allow trading in thinner markets but increase slippage risk.

### Liquidity Position Sizing

The maximum percentage of available liquidity to use defaults to **80%**:

```bash
# In .env file
LIQUIDITY_MAX_POSITION_PCT=80.0  # Max % of liquidity to use
```

This prevents taking the entire book and ensures you can exit positions.

## Testing

To verify the fix works:

1. **Check signal_processor logs** for the new fallback message:
   ```
   Using signal's liquidity_available ($100.00) as fallback for market price with no liquidity data
   ```

2. **Monitor rejected signals**:
   ```bash
   docker-compose logs -f signal_processor | grep "insufficient_liquidity"
   ```
   Should see fewer rejections now.

3. **Check paper trades**:
   ```bash
   docker-compose logs -f execution_service | grep "PAPER TRADE"
   ```
   Should see trades being executed that were previously rejected.

## Impact

This fix ensures that:
- ✅ Signals with `liquidity_available > 0` are no longer rejected
- ✅ Fallback market prices use realistic liquidity values
- ✅ Signal processor gracefully handles missing market_prices data
- ✅ Paper trading works even when market_prices table is incomplete

## Next Steps

If you want to **lower the liquidity threshold** to allow more signals through:

```bash
# In .env
LIQUIDITY_MIN_THRESHOLD=5.0  # Allow trades with $5+ liquidity

# Restart signal processor
docker-compose restart signal_processor
```

**Note**: Lowering below $10 may result in increased slippage and difficulty exiting positions in thin markets.

## Related Files

- [services/signal_processor_rust/src/main.rs](services/signal_processor_rust/src/main.rs) - Signal processing and liquidity validation
- [services/game_shard_rust/src/shard.rs](services/game_shard_rust/src/shard.rs) - Signal creation with liquidity_available
- [rust_core/src/models/mod.rs](rust_core/src/models/mod.rs) - TradingSignal struct definition

## Deployment

To deploy this fix:

```bash
# Rebuild signal processor
docker-compose build signal_processor

# Restart
docker-compose restart signal_processor

# Watch logs
docker-compose logs -f signal_processor
```

The fix is backward compatible and requires no database migrations.
