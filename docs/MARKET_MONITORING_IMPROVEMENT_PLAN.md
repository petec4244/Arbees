# Market Monitoring Improvement Plan

**Goal:** Maximize profit across all market types with zero-error architecture

**Status:** Architecture defined, Sports implemented, Other markets pending

---

## Executive Summary

The Arbees codebase has **excellent abstractions** already defined:
- `MarketType` enum (Sports, Politics, Economics, Crypto, Entertainment)
- `EventProvider` trait for data sources
- `ProbabilityModel` trait for edge calculation

**Current Reality:** Only sports markets are actually implemented. The non-sports code paths are stubs.

**Opportunity:** With minimal architecture changes, we can activate 4 additional market types for ~5x market coverage.

---

## Current Architecture Analysis

### What's Working Well (Keep As-Is)

| Component | Status | Why Keep |
|-----------|--------|----------|
| **rust_core abstractions** | Excellent | Well-designed traits, minimal changes needed |
| **Redis pub/sub messaging** | Solid | Decoupled services, easy to add new producers |
| **ZMQ for latency-critical paths** | Good | Optional low-latency for time-sensitive markets |
| **Paper trading system** | Robust | Safe validation before real money |
| **Notification service** | Now rate-limited | Won't spam you during volatile periods |

### What Needs Improvement

| Component | Issue | Impact |
|-----------|-------|--------|
| **orchestrator_rust** | ESPN-only, hardcoded 7 sports | Can't discover non-sports events |
| **market_discovery_rust** | Team matching logic sports-centric | Fails for "BTC > $100k" markets |
| **game_shard_rust** | Win probability model sports-only | No edge calculation for other markets |
| **signal_processor_rust** | Assumes binary YES/NO with teams | Crypto/Economics don't have "teams" |

---

## Recommended Service Changes

### Option A: Minimal Changes (Recommended - Less Risk)

**Philosophy:** Keep existing services, add market-type routing

```
                  ┌─────────────────────────────────────┐
                  │         EVENT ROUTER (NEW)          │
                  │  Routes by market_type to provider  │
                  └─────────────────────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        ▼                  ▼                  ▼
   ┌─────────┐       ┌──────────┐       ┌──────────┐
   │  ESPN   │       │  Crypto  │       │ Politics │
   │Provider │       │ Provider │       │ Provider │
   │(exists) │       │  (NEW)   │       │  (NEW)   │
   └─────────┘       └──────────┘       └──────────┘
        │                  │                  │
        └──────────────────┼──────────────────┘
                           ▼
              ┌─────────────────────────┐
              │  market_discovery_rust  │
              │  (enhanced matching)    │
              └─────────────────────────┘
                           │
                           ▼
              ┌─────────────────────────┐
              │     game_shard_rust     │
              │  (uses ProbabilityModel │
              │   registry - exists!)   │
              └─────────────────────────┘
```

**Changes Required:**
1. Add `EventRouter` in orchestrator (20% effort)
2. Implement new `EventProvider` for each market type (60% effort)
3. Implement new `ProbabilityModel` for each market type (15% effort)
4. Enhance market_discovery matching for non-team entities (5% effort)

### Option B: Major Refactor (Not Recommended Now)

Split into market-type specific services:
- sports_orchestrator
- crypto_orchestrator
- politics_orchestrator
- etc.

**Why Not Now:** More complexity, more containers, higher ops burden. Do this later if a specific market type needs unique scaling.

---

## Implementation Plan by Market Type

### 1. CRYPTO MARKETS (Highest ROI - Implement First)

**Why First:**
- 24/7 markets (no downtime)
- High volatility = more edge opportunities
- Clear price targets ("BTC > $100k by Dec 2025")
- Real-time data easily available

**Data Sources:**
| Source | Data | API |
|--------|------|-----|
| CoinGecko | Price, volume, market cap | Free tier available |
| Binance | Real-time orderbook | WebSocket |
| Coinglass | Funding rates, OI | API key required |
| Kalshi/Polymarket | Market prices | Already integrated |

**New Files Needed:**
```
rust_core/src/providers/crypto.rs       # CryptoEventProvider
rust_core/src/clients/coingecko.rs      # CoinGecko API client
rust_core/src/probability/crypto.rs      # CryptoProbabilityModel
```

**Probability Model Logic:**
```rust
/// BTC > $100k by Dec 2025
fn calculate_probability(current_price: f64, target: f64, days_remaining: u32, volatility: f64) -> f64 {
    // Black-Scholes inspired: probability of hitting target
    let daily_vol = volatility / 365.0_f64.sqrt();
    let expected_move = daily_vol * (days_remaining as f64).sqrt();
    let z_score = (target - current_price).ln() / expected_move;
    1.0 - normal_cdf(z_score)
}
```

**Edge Opportunity:**
- Market price: 0.45 (45% chance BTC > $100k)
- Model price: 0.52 (based on current $95k price + volatility)
- Edge: 7% (tradeable!)

---

### 2. ECONOMICS MARKETS (High Value - Implement Second)

**Why Second:**
- Predictable schedule (Fed meetings, CPI releases)
- Clear data source (FRED, BLS)
- Markets often mispriced around releases

**Data Sources:**
| Source | Data | API |
|--------|------|-----|
| FRED | Economic indicators | Free API |
| BLS | CPI, Jobs data | Free API |
| Investing.com | Economic calendar | Scrape or API |
| Kalshi/Polymarket | Market prices | Already integrated |

**New Files Needed:**
```
rust_core/src/providers/economics.rs     # EconomicsEventProvider
rust_core/src/clients/fred.rs            # FRED API client
rust_core/src/probability/economics.rs    # EconomicsProbabilityModel
```

**Probability Model Logic:**
```rust
/// Will CPI YoY be > 3.0% in January?
fn calculate_probability(
    forecast: f64,      // Consensus: 2.9%
    threshold: f64,     // Market: 3.0%
    forecast_std: f64,  // Historical forecast error: 0.15%
) -> f64 {
    // Z-score based on forecast distribution
    let z = (threshold - forecast) / forecast_std;
    1.0 - normal_cdf(z)
}
```

**Edge Opportunity:**
- If market prices "CPI > 3%" at 0.30 but model says 0.45, that's 15% edge

---

### 3. POLITICS MARKETS (Medium Value - Implement Third)

**Why Third:**
- Fewer events but high liquidity
- Polling data is public
- Long-running markets (elections months away)

**Data Sources:**
| Source | Data | API |
|--------|------|-----|
| FiveThirtyEight | Poll aggregates | Free (scrape) |
| RealClearPolitics | Poll averages | Free (scrape) |
| Polymarket | Market prices | Already integrated |
| PredictIt | Alternative market | API available |

**New Files Needed:**
```
rust_core/src/providers/politics.rs      # PoliticsEventProvider
rust_core/src/clients/polling.rs         # Polling aggregator client
rust_core/src/probability/politics.rs     # PoliticsProbabilityModel
```

**Probability Model:**
- Use poll aggregates with uncertainty adjustment
- Account for systematic polling errors (2016, 2020 lessons)
- Factor in time-to-event (more uncertainty further out)

---

### 4. ENTERTAINMENT MARKETS (Lower Priority)

**Why Last:**
- Sporadic events (Oscars, Grammys yearly)
- Harder to model (subjective outcomes)
- Lower liquidity

**Keep simple:** Use market-implied probabilities as baseline, only trade obvious mispricings.

---

## Service Consolidation Recommendations

### Consolidate: market_discovery_rust

**Current Issues:**
- 1000+ lines in main.rs
- Mixes team matching, caching, RPC handling

**Recommendation:** Extract into modules but keep single service:
```
market_discovery_rust/
├── src/
│   ├── main.rs              # Entry point, message loop only
│   ├── discovery/
│   │   ├── mod.rs
│   │   ├── sports.rs        # Sports market matching
│   │   ├── crypto.rs        # Crypto market matching (NEW)
│   │   ├── economics.rs     # Economics market matching (NEW)
│   │   └── politics.rs      # Politics market matching (NEW)
│   ├── matching/
│   │   ├── mod.rs
│   │   ├── team.rs          # Team name matching (existing)
│   │   └── entity.rs        # Generic entity matching (NEW)
│   └── cache.rs             # Market cache
```

**Why Single Service:** All discovery logic shares the same Kalshi/Polymarket API clients. Splitting would duplicate API connections.

### Consolidate: orchestrator_rust + event routing

**Current:** Orchestrator only polls ESPN

**Recommendation:** Add event router module:
```
orchestrator_rust/
├── src/
│   ├── main.rs
│   ├── router.rs            # Routes to appropriate provider (NEW)
│   ├── providers/
│   │   ├── mod.rs
│   │   ├── espn.rs          # Sports provider (existing)
│   │   ├── crypto.rs        # Crypto provider (NEW)
│   │   ├── economics.rs     # Economics provider (NEW)
│   │   └── politics.rs      # Politics provider (NEW)
│   └── managers/
│       └── game_manager.rs  # Rename to event_manager.rs
```

### Keep Separate: signal_processor_rust

**Why:** Already well-structured, just needs probability model registration:
```rust
// Add to signal_processor_rust initialization
let mut registry = ProbabilityModelRegistry::new();
registry.register_model(Box::new(CryptoProbabilityModel::new()));
registry.register_model(Box::new(EconomicsProbabilityModel::new()));
registry.register_model(Box::new(PoliticsProbabilityModel::new()));
```

### Keep Separate: execution_service_rust

**Why:** Execution logic is already platform-agnostic (Kalshi/Polymarket/Paper). Works for all market types.

---

## Risk Management Enhancements

### Market-Type Specific Limits

```rust
// Add to risk controller config
pub struct MarketTypeLimits {
    pub max_exposure: f64,
    pub max_position_pct: f64,
    pub min_edge_pct: f64,
    pub cooldown_after_loss: Duration,
}

// Suggested defaults
SPORTS:       { max_exposure: 1000, max_position_pct: 10, min_edge_pct: 4.0, cooldown: 30s }
CRYPTO:       { max_exposure: 500,  max_position_pct: 5,  min_edge_pct: 8.0, cooldown: 60s }
ECONOMICS:    { max_exposure: 300,  max_position_pct: 5,  min_edge_pct: 10.0, cooldown: 0s }
POLITICS:     { max_exposure: 200,  max_position_pct: 3,  min_edge_pct: 5.0, cooldown: 0s }
ENTERTAINMENT:{ max_exposure: 100,  max_position_pct: 2,  min_edge_pct: 15.0, cooldown: 0s }
```

**Rationale:**
- **Crypto:** Higher volatility → smaller positions, higher edge required
- **Economics:** Event-driven, one-shot → no cooldown needed
- **Politics:** Long-duration → small positions to manage exposure
- **Entertainment:** Hardest to model → highest edge threshold

### Volatility-Adjusted Position Sizing

```rust
/// Kelly fraction adjusted for market type volatility
fn adjusted_kelly(base_kelly: f64, market_type: &MarketType) -> f64 {
    let volatility_multiplier = match market_type {
        MarketType::Sport { .. } => 1.0,      // Baseline
        MarketType::Crypto { .. } => 0.5,     // Half size due to volatility
        MarketType::Economics { .. } => 1.2,  // Larger (binary, predictable)
        MarketType::Politics { .. } => 0.8,   // Reduced (long duration risk)
        MarketType::Entertainment { .. } => 0.3, // Minimal (hard to model)
    };
    base_kelly * volatility_multiplier
}
```

---

## Implementation Priority & Timeline

### Phase 1: Foundation (Week 1-2)
- [ ] Refactor orchestrator with EventRouter
- [ ] Add entity matching (not just team matching)
- [ ] Register probability models in signal_processor

### Phase 2: Crypto Markets (Week 3-4)
- [ ] Implement CoinGecko client
- [ ] Implement CryptoEventProvider
- [ ] Implement CryptoProbabilityModel
- [ ] Test with paper trading

### Phase 3: Economics Markets (Week 5-6)
- [ ] Implement FRED client
- [ ] Implement EconomicsEventProvider
- [ ] Implement EconomicsProbabilityModel
- [ ] Test with paper trading

### Phase 4: Politics Markets (Week 7-8)
- [ ] Implement polling aggregator client
- [ ] Implement PoliticsEventProvider
- [ ] Implement PoliticsProbabilityModel
- [ ] Test with paper trading

### Phase 5: Full Integration (Week 9-10)
- [ ] Enable all market types in production
- [ ] Monitor for 2 weeks
- [ ] Tune parameters based on results

---

## "Make No Mistakes" Checklist

### Before Any Code Change:
- [ ] Write tests FIRST (probability model tests, matching tests)
- [ ] Paper trade for minimum 48 hours per market type
- [ ] Set conservative limits initially (halve all max_exposure values)
- [ ] Monitor notification frequency (rate limiter should prevent spam)

### Before Going Live:
- [ ] Review all probability models with historical data
- [ ] Verify market matching accuracy > 99%
- [ ] Confirm risk limits are set per market type
- [ ] Test notification system works
- [ ] Have manual kill switch ready

### Ongoing:
- [ ] Daily P&L review per market type
- [ ] Weekly model accuracy review
- [ ] Monthly parameter tuning

---

## Expected ROI

| Market Type | Events/Day | Avg Edge | Est. Daily Profit |
|-------------|------------|----------|-------------------|
| Sports | 20-50 | 5-15% | $50-150 |
| Crypto | 5-20 | 8-20% | $30-100 |
| Economics | 2-5 | 10-25% | $20-50 |
| Politics | 1-3 | 5-10% | $10-30 |
| **TOTAL** | 28-78 | - | **$110-330/day** |

*Conservative estimates based on current sports performance extrapolated*

---

## Summary

**Key Insight:** The architecture is already designed for multi-market. We just need to implement the providers and probability models.

**Recommended Approach:**
1. Keep existing services (no major refactors)
2. Add event routing in orchestrator
3. Implement one market type at a time (Crypto → Economics → Politics)
4. Paper trade each before enabling

**Expected Outcome:** 3-5x more trading opportunities with same infrastructure.

💰 Let's make those bags! 💰
