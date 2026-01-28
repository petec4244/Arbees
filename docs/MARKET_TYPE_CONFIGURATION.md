# Market Type Configuration

## Overview

Arbees now supports multiple market types beyond sports: **Politics**, **Economics**, **Cryptocurrency**, and **Entertainment**. This document describes how to enable/disable specific market types.

## Environment Variables

Add the following to your `.env` file:

```bash
# Market type enablement (Phase 4-5: Multi-market expansion)
# Set to "true" to enable, "false" to disable
# Only enabled markets will be discovered and traded
ENABLE_MARKET_SPORT=true
ENABLE_MARKET_POLITICS=false
ENABLE_MARKET_ECONOMICS=false
ENABLE_MARKET_CRYPTO=false
ENABLE_MARKET_ENTERTAINMENT=false
```

## Market Types

### 1. **Sport** (Enabled by default)
- **Provider**: ESPN API
- **Probability Model**: Win probability calculation
- **Entity Matcher**: Team matching with aliases
- **Sports**: NBA, NFL, NHL, MLB, NCAAB, NCAAF, MLS, Soccer, Tennis, MMA
- **Status**: ✅ Fully implemented

### 2. **Politics** (Coming soon)
- **Provider**: Polling aggregators (FiveThirtyEight, RealClearPolitics)
- **Probability Model**: Polling average models
- **Entity Matcher**: Candidate name matching
- **Markets**: Elections, confirmation votes, policy votes
- **Status**: 🚧 Trait infrastructure ready, providers pending

### 3. **Economics** (Coming soon)
- **Provider**: Economic calendars (FRED, BLS, Bloomberg)
- **Probability Model**: Consensus forecast models
- **Entity Matcher**: Indicator name matching
- **Markets**: CPI, unemployment, Fed decisions, GDP
- **Status**: 🚧 Trait infrastructure ready, providers pending

### 4. **Cryptocurrency** (Coming soon)
- **Provider**: Price feeds (CoinGecko, Binance, Messari)
- **Probability Model**: Technical analysis / price target models
- **Entity Matcher**: Asset/token name matching
- **Markets**: Price targets, protocol events, TVL predictions
- **Status**: 🚧 Trait infrastructure ready, providers pending

### 5. **Entertainment** (Coming soon)
- **Provider**: Entertainment news APIs
- **Probability Model**: Event-based probability models
- **Entity Matcher**: Event/category matching
- **Markets**: Awards shows, box office, streaming metrics
- **Status**: 🚧 Trait infrastructure ready, providers pending

## Architecture

### Phase 1-3: Foundation (✅ Complete)
- **MarketType enum**: Universal market discriminator
- **EventProvider trait**: Pluggable data sources
- **ProbabilityModel trait**: Pluggable probability calculations
- **Database migration**: Multi-market schema support

### Phase 4-5: Entity Matching & Config (✅ Complete)
- **EntityMatcher trait**: Pluggable entity matching
- **EntityMatcherRegistry**: Automatic matcher selection
- **TeamMatcher**: Sports team matching implementation
- **Configuration**: Market type enablement flags

### Phase 6-8: Provider Implementation (🚧 Planned)
- Implement concrete providers for each market type
- Build probability models for non-sports markets
- Create entity matchers for each domain
- Integration testing across all market types

## Usage

### Enabling a Market Type

1. **Update `.env`**:
   ```bash
   ENABLE_MARKET_POLITICS=true
   ```

2. **Restart orchestrator**:
   ```bash
   docker compose --profile full restart orchestrator
   ```

3. **Verify**: Check logs for market discovery messages:
   ```bash
   docker compose logs -f orchestrator | grep "market_type"
   ```

### Disabling a Market Type

1. **Update `.env`**:
   ```bash
   ENABLE_MARKET_SPORT=false
   ```

2. **Restart orchestrator**:
   ```bash
   docker compose --profile full restart orchestrator
   ```

## Testing

Run the test suite to verify market type abstractions:

```bash
cd services
cargo test --package arbees_rust_core --lib matching
cargo test --package arbees_rust_core --lib probability
cargo test --package arbees_rust_core --lib providers
```

## Backward Compatibility

All changes maintain **100% backward compatibility** with existing sports-only functionality:

- Legacy `GameState` struct still supported
- Existing `calculate_win_probability()` function unchanged
- Team matching logic wrapped in `TeamMatcher`
- ESPN provider wraps existing ESPN client

Setting `ENABLE_MARKET_SPORT=false` will disable sports trading, but all sports infrastructure remains available for future use.

## Next Steps

To add a new market type provider:

1. Implement `EventProvider` trait for your data source
2. Implement `ProbabilityModel` trait for your probability calculations
3. Implement `EntityMatcher` trait for entity matching
4. Register your implementations in the respective registries
5. Update documentation with supported markets

See `rust_core/src/providers/espn.rs` for a complete example implementation.
