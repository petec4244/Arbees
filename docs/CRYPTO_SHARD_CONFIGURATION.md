# Crypto Shard Configuration Guide

## Overview

The crypto_shard_rust service provides comprehensive configuration options for tuning arbitrage detection, risk management, and probability models. All configuration is via environment variables.

## Configuration Parameters

### Service Identity

```env
CRYPTO_SHARD_ID=crypto_1                              # Unique identifier for this shard
```

### ZMQ Endpoints

```env
# Price subscription endpoints (comma-separated)
CRYPTO_PRICE_SUB_ENDPOINTS=tcp://kalshi_monitor:5555,tcp://vpn:5556,tcp://crypto-spot-monitor:5560

# Execution request publishing endpoint
CRYPTO_EXECUTION_PUB_ENDPOINT=tcp://*:5559            # Publishes ExecutionRequests to execution_service
```

### Risk Management

#### Position Sizing

```env
CRYPTO_MIN_EDGE_PCT=3.0                               # Minimum arbitrage edge % to trade (3% default for crypto, 7% for sports)
                                                       # Lower = more trades but lower expected profit per trade
                                                       # Higher = fewer trades but each with higher edge

CRYPTO_MAX_POSITION_SIZE=500.0                        # Maximum USD per single trade
                                                       # Typical range: $100-$1000
                                                       # Lower = safer but less capital efficiency

CRYPTO_VOLATILITY_SCALING=true                        # Enable volatility-based position sizing
                                                       # If true, reduces position size by 30% during high volatility
                                                       # Volatility factor > 1.5 triggers scaling
```

#### Exposure Limits

```env
CRYPTO_MAX_ASSET_EXPOSURE=2000.0                      # Maximum USD exposed per asset (BTC, ETH, SOL, etc.)
                                                       # Prevent over-concentration in single asset
                                                       # Should be >= CRYPTO_MAX_POSITION_SIZE

CRYPTO_MAX_TOTAL_EXPOSURE=5000.0                      # Maximum USD across all crypto trades
                                                       # Portfolio-level risk limit
                                                       # Should be >= CRYPTO_MAX_ASSET_EXPOSURE
```

#### Liquidity Requirements

```env
CRYPTO_MIN_LIQUIDITY=50.0                             # Minimum USD liquidity to trade
                                                       # Markets below this threshold are skipped
                                                       # Prevent trading illiquid positions
                                                       # Typical range: $10-$100
```

### Probability Model

#### Volatility Calculation

```env
CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS=30                # Historical volatility lookback period
                                                       # Longer window = smoother volatility estimate
                                                       # Shorter window = more responsive to recent moves
                                                       # Typical range: 7-90 days

CRYPTO_MODEL_TIME_DECAY=true                          # Enable probability time decay as expiration approaches
                                                       # If true, adjusts probability model closer to expiration
```

#### Model Confidence

```env
CRYPTO_MODEL_MIN_CONFIDENCE=0.60                      # Minimum model confidence to generate signal
                                                       # Range: 0.0-1.0
                                                       # Lower = more signals but lower reliability
                                                       # Higher = fewer signals but higher conviction
                                                       # Typical range: 0.55-0.75
```

### Monitoring & Debugging

```env
CRYPTO_POLL_INTERVAL_SECS=30                          # How often to check events for arbitrage
                                                       # Lower = more responsive but more CPU
                                                       # Higher = less responsive but more efficient
                                                       # Typical range: 5-60 seconds

CRYPTO_PRICE_STALENESS_SECS=60                        # Maximum age before price is considered stale
                                                       # If price is older than this, skip trade
                                                       # Must be longer than poll interval
                                                       # Typical range: 30-120 seconds

CRYPTO_HEARTBEAT_INTERVAL_SECS=5                      # How often to publish health heartbeat to Redis
                                                       # For orchestrator monitoring
                                                       # Typical range: 5-30 seconds
```

### Logging

```env
RUST_LOG=info                                         # Logging level (trace, debug, info, warn, error)
                                                       # info = normal operation
                                                       # debug = detailed diagnostics
                                                       # trace = very verbose
```

## Configuration Profiles

### Conservative (Low Risk)

```env
CRYPTO_MIN_EDGE_PCT=7.0                               # Higher bar for trades
CRYPTO_MAX_POSITION_SIZE=100.0                        # Small positions
CRYPTO_MAX_ASSET_EXPOSURE=500.0                       # Strict per-asset limits
CRYPTO_MAX_TOTAL_EXPOSURE=2000.0                      # Conservative portfolio limit
CRYPTO_VOLATILITY_SCALING=true                        # Reduce size in volatile markets
CRYPTO_MODEL_MIN_CONFIDENCE=0.75                      # Only high-confidence signals
```

### Moderate (Balanced)

```env
CRYPTO_MIN_EDGE_PCT=3.0                               # Reasonable edge threshold
CRYPTO_MAX_POSITION_SIZE=500.0                        # Medium positions
CRYPTO_MAX_ASSET_EXPOSURE=2000.0                      # Reasonable asset limits
CRYPTO_MAX_TOTAL_EXPOSURE=5000.0                      # Balanced portfolio limit
CRYPTO_VOLATILITY_SCALING=true                        # Adapt to market conditions
CRYPTO_MODEL_MIN_CONFIDENCE=0.60                      # Good-quality signals
```

### Aggressive (High Volume)

```env
CRYPTO_MIN_EDGE_PCT=1.5                               # Lower edge threshold
CRYPTO_MAX_POSITION_SIZE=1000.0                       # Large positions
CRYPTO_MAX_ASSET_EXPOSURE=5000.0                      # Generous asset limits
CRYPTO_MAX_TOTAL_EXPOSURE=15000.0                     # Large portfolio allocation
CRYPTO_VOLATILITY_SCALING=false                       # Don't scale down in volatility
CRYPTO_MODEL_MIN_CONFIDENCE=0.50                      # Include marginal signals
```

### Testing/Paper Trading

```env
CRYPTO_MIN_EDGE_PCT=1.0                               # Very low threshold to generate signals
CRYPTO_MAX_POSITION_SIZE=10000.0                      # No position limits in paper mode
CRYPTO_MAX_ASSET_EXPOSURE=50000.0                     # No asset limits in paper mode
CRYPTO_MAX_TOTAL_EXPOSURE=100000.0                    # No portfolio limits
CRYPTO_VOLATILITY_SCALING=true                        # Still test scaling logic
CRYPTO_MODEL_MIN_CONFIDENCE=0.40                      # Generate many signals to test
```

## Tuning Guidelines

### For Maximum Profitability

1. **Lower CRYPTO_MIN_EDGE_PCT** (2-4% instead of 7%)
   - Catch more opportunities
   - But ensure they're still profitable after fees

2. **Increase CRYPTO_MODEL_MIN_CONFIDENCE** (0.70-0.80)
   - Only trade high-conviction signals
   - Reduce false positives

3. **Enable CRYPTO_VOLATILITY_SCALING**
   - Automatically reduce size when market conditions worsen

4. **Use longer CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS** (30-60)
   - Smoother volatility estimates
   - Avoid overreacting to single spikes

### For Maximum Safety

1. **Raise CRYPTO_MIN_EDGE_PCT** (5-10%)
   - Only trade obvious opportunities
   - Lower win rate but higher profit per trade

2. **Lower CRYPTO_MODEL_MIN_CONFIDENCE** (0.40-0.55)
   - Include more signals for diversification

3. **Set Conservative Exposure Limits**
   - CRYPTO_MAX_POSITION_SIZE: $50-$200
   - CRYPTO_MAX_ASSET_EXPOSURE: $500-$1000
   - CRYPTO_MAX_TOTAL_EXPOSURE: $1000-$2000

4. **Increase CRYPTO_PRICE_STALENESS_SECS** (90-120)
   - Only trade with fresh price data
   - Reduce stale price risk

### For Real-Time Responsiveness

1. **Lower CRYPTO_POLL_INTERVAL_SECS** (5-10 seconds)
   - Check for opportunities more frequently
   - May increase CPU usage

2. **Lower CRYPTO_PRICE_STALENESS_SECS** (30-45 seconds)
   - Use newer price data
   - But risk missing some opportunities

3. **Lower CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS** (7-14)
   - Respond faster to volatility changes

## Monitoring Tuning Effectiveness

### Key Metrics to Watch

```bash
# Check signal generation rate
docker logs arbees-crypto-shard | grep "arbitrage detected" | wc -l

# Check edge distribution
docker logs arbees-crypto-shard | grep "edge" | awk '{print $(NF-1)}' | sort -n

# Check position sizes being used
docker logs arbees-crypto-shard | grep "size=" | awk -F'size=' '{print $2}' | sort -n

# Check risk blocks
docker logs arbees-crypto-shard | grep "blocked" | wc -l
```

## Dynamic Configuration (Without Restarting)

To update configuration without restarting crypto_shard, use environment file:

```bash
# Edit .env file
vi .env

# Update specific variables
docker-compose up -d --force-recreate crypto_shard
```

## Performance Characteristics

| Config | Trades/Hour | Avg Edge | Win Rate | Drawdown |
|--------|------------|---------|----------|----------|
| Conservative | 2-5 | 5-10% | 80-90% | 5-10% |
| Moderate | 5-15 | 3-5% | 70-80% | 10-20% |
| Aggressive | 15-50+ | 1-3% | 60-70% | 20-40% |

*Note: These are estimates based on typical market conditions. Actual results will vary.*

## Troubleshooting Configuration Issues

### No trades generated
- Lower CRYPTO_MIN_EDGE_PCT (too high = no opportunities pass threshold)
- Lower CRYPTO_MODEL_MIN_CONFIDENCE (too high = no signals)
- Check CRYPTO_PRICE_STALENESS_SECS (too low = prices rejected as stale)

### Too many false positives
- Raise CRYPTO_MIN_EDGE_PCT (filter out marginal trades)
- Raise CRYPTO_MODEL_MIN_CONFIDENCE (only high-conviction signals)
- Increase CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS (smoother probability estimates)

### Position sizing too small
- Increase CRYPTO_MAX_POSITION_SIZE
- Increase CRYPTO_MAX_ASSET_EXPOSURE
- Disable or adjust CRYPTO_VOLATILITY_SCALING

### Hitting exposure limits frequently
- Increase CRYPTO_MAX_ASSET_EXPOSURE
- Increase CRYPTO_MAX_TOTAL_EXPOSURE
- Lower CRYPTO_MAX_POSITION_SIZE to spread across more trades

### Missing opportunities (prices stale)
- Increase CRYPTO_POLL_INTERVAL_SECS frequency
- Decrease CRYPTO_PRICE_STALENESS_SECS
- Ensure monitors are publishing regularly

## Environment File Template

```env
# Service Identity
CRYPTO_SHARD_ID=crypto_1

# ZMQ Configuration
CRYPTO_PRICE_SUB_ENDPOINTS=tcp://kalshi_monitor:5555,tcp://vpn:5556,tcp://crypto-spot-monitor:5560
CRYPTO_EXECUTION_PUB_ENDPOINT=tcp://*:5559

# Risk Management
CRYPTO_MIN_EDGE_PCT=3.0
CRYPTO_MAX_POSITION_SIZE=500.0
CRYPTO_MAX_ASSET_EXPOSURE=2000.0
CRYPTO_MAX_TOTAL_EXPOSURE=5000.0
CRYPTO_VOLATILITY_SCALING=true
CRYPTO_MIN_LIQUIDITY=50.0

# Probability Model
CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS=30
CRYPTO_MODEL_TIME_DECAY=true
CRYPTO_MODEL_MIN_CONFIDENCE=0.60

# Monitoring
CRYPTO_POLL_INTERVAL_SECS=30
CRYPTO_PRICE_STALENESS_SECS=60
CRYPTO_HEARTBEAT_INTERVAL_SECS=5

# Database
DATABASE_URL=postgresql://user:pass@host:5432/arbees

# Logging
RUST_LOG=info
```

## See Also

- [CLAUDE.md](../CLAUDE.md) - Project architecture overview
- [Docker Compose Configuration](../docker-compose.yml) - Service definitions
- [Risk Management Guide](./RISK_MANAGEMENT.md) - Detailed risk control
