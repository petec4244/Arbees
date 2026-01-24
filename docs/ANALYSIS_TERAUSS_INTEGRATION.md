# ANALYSIS: Integrating terauss Rust Arbitrage Bot into Arbees

## Executive Summary

You have a **production-ready, battle-tested Rust arbitrage bot** that is:
- ✅ **Simple and focused** - Does ONE thing well (pure arbitrage)
- ✅ **Production hardened** - Circuit breakers, position tracking, SIMD optimization
- ✅ **Sports-focused currently** - But architecture supports ANY binary markets
- ✅ **Fast and reliable** - Rust, lock-free orderbooks, sub-millisecond latency

Your current Arbees system is:
- ⚠️ **Getting complex** - Live prob models, game state tracking, edge cases
- ⚠️ **Sports-only** - No crypto, weather, or other markets
- ⚠️ **Higher risk** - More moving parts = more failure modes

## The Big Picture: Two Complementary Systems

```
┌────────────────────────────────────────────────────────────┐
│  ARBEES (Python) - Probabilistic Sports Trading            │
│  ✓ Live game state tracking (ESPN API)                     │
│  ✓ Win probability models                                  │
│  ✓ Edge detection (model vs market)                        │
│  ✓ Signal generation on probability shifts                 │
│  ✓ Complex but powerful                                    │
└────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────────────┐
│  TERAUSS (Rust) - Pure Arbitrage Trading                   │
│  ✓ Simple math: YES + NO < $1.00 = profit                  │
│  ✓ No models needed (guaranteed profit)                    │
│  ✓ Works on ANY binary market                              │
│  ✓ Fast, reliable, battle-tested                           │
│  ✓ Circuit breakers + position tracking built-in           │
└────────────────────────────────────────────────────────────┘

          ↓ INTEGRATION STRATEGY ↓

┌────────────────────────────────────────────────────────────┐
│  COMBINED SYSTEM - Best of Both Worlds                     │
│                                                             │
│  SPORTS MARKETS:                                            │
│    ├─ Arbees: Probabilistic edge trading                   │
│    └─ terauss: Pure arb opportunities                      │
│                                                             │
│  NON-SPORTS MARKETS (crypto, weather, politics):           │
│    └─ terauss: ONLY pure arbitrage                         │
│                                                             │
│  Result: Lower risk, broader markets, higher profits       │
└────────────────────────────────────────────────────────────┘
```

---

## Key Insights from terauss Bot

### What It Does Well

1. **Pure Arbitrage Detection (SIMD-optimized)**
   ```rust
   // The math is simple:
   if yes_ask + no_ask < 100 cents {
       profit = 100 - (yes_ask + no_ask)  // Guaranteed!
   }
   ```

2. **Production-Ready Safety**
   - Circuit breakers (max position, max loss, consecutive errors)
   - Position tracking across both platforms
   - In-flight deduplication (prevents double-execution)
   - Dry run mode for testing

3. **Market Discovery**
   - Intelligent caching (2-hour TTL)
   - Incremental updates (don't re-fetch everything)
   - Handles Kalshi rate limits (2 req/sec)
   - Works across 15+ sports leagues

4. **Architecture Benefits**
   - **Lock-free orderbooks** - Atomic operations, no mutex contention
   - **SIMD arb detection** - Processes multiple markets in parallel using CPU vectors
   - **Concurrent execution** - Both legs execute simultaneously
   - **Sub-millisecond latency** - Rust + optimizations

### What It Currently Doesn't Do

1. **Only sports markets** - Config has EPL, NBA, NFL, etc. but NO crypto/weather
2. **No probabilistic models** - Just pure math (which is actually good!)
3. **No ML/learning** - Doesn't improve over time

---

## The Opportunity: Expand to Non-Sports Markets

### What Markets Are Available?

**Kalshi offers:**
- 🏀 Sports (already covered)
- 💰 **Economics** (inflation, GDP, Fed rates)
- 🌦️ **Weather** (temperature, precipitation, hurricanes)
- 📊 **Finance** (stock prices, crypto, commodities)
- 🗳️ **Politics** (elections, policy outcomes - though you may want to avoid)
- 📈 **Crypto** (Bitcoin price, ETH trends)

**Polymarket offers:**
- 🏀 Sports (already covered)
- 💰 **Crypto** (BTC price predictions, ETH movements)
- 🌦️ **Weather** (similar to Kalshi)
- 🗳️ **Politics** (HUGE volume here, but risky)
- 📰 **Current events** (various predictions)

**The sweet spot for pure arbitrage:**
- ✅ **Crypto price markets** - High volume, liquid, well-defined
- ✅ **Weather markets** - Binary outcomes, clear resolution
- ✅ **Economic indicators** - Fed decisions, inflation reports
- ⚠️ **Politics** - High volume BUT resolution disputes common

---

## Integration Strategy

### Option 1: Standalone Container (RECOMMENDED)

**Run terauss bot as separate Docker container:**

```yaml
# docker-compose.yml

services:
  # Your existing Arbees services...
  orchestrator:
    ...
  game_shards:
    ...
  
  # NEW: Pure arbitrage bot (terauss)
  arb_bot_rust:
    image: terauss-arb:latest
    container_name: arbees_pure_arb
    environment:
      - KALSHI_API_KEY_ID=${KALSHI_API_KEY_ID}
      - KALSHI_PRIVATE_KEY_PATH=/secrets/kalshi_key.pem
      - POLY_PRIVATE_KEY=${POLY_PRIVATE_KEY}
      - POLY_FUNDER=${POLY_FUNDER}
      - DRY_RUN=0  # Live trading
      - ENABLED_LEAGUES=crypto,weather,economics  # NEW markets!
      - ARB_THRESHOLD=0.995  # 0.5% minimum profit
      - CB_MAX_DAILY_LOSS=1000  # $1000 daily loss limit
      - RUST_LOG=info
    volumes:
      - ./secrets:/secrets:ro
    restart: unless-stopped
    networks:
      - arbees_network
```

**Benefits:**
- ✅ **Zero integration work** - Just configure and run
- ✅ **Isolated** - Crashes don't affect Arbees
- ✅ **Easy to disable** - Just stop the container
- ✅ **Battle-tested** - Use proven code as-is

**Drawbacks:**
- ❌ **No shared intelligence** - Doesn't learn from Arbees models
- ❌ **Duplicate position tracking** - Each system tracks separately

---

### Option 2: Hybrid Integration (ADVANCED)

**Integrate key components from terauss into Arbees:**

```python
# New service: services/pure_arb_monitor/

services/pure_arb_monitor/
├── monitor.py           # Python wrapper
├── rust_bindings/       # PyO3 bindings to Rust code
│   ├── src/
│   │   ├── lib.rs       # Rust library interface
│   │   ├── simd_arb.rs  # Copy from terauss
│   │   └── circuit_breaker.rs  # Copy from terauss
└── Dockerfile

# Python calls Rust for performance-critical parts:
from rust_bindings import detect_arbitrage_simd, CircuitBreaker

# Use Rust SIMD for fast arb detection:
arbs = detect_arbitrage_simd(orderbooks)

# Use Rust circuit breaker for safety:
cb = CircuitBreaker(max_position=1000, max_daily_loss=5000)
if cb.can_trade():
    execute_trade()
```

**Benefits:**
- ✅ **Shared position tracking** - Single source of truth
- ✅ **Shared circuit breaker** - Coordinated risk management
- ✅ **SIMD performance** - Keep the fast arb detection
- ✅ **Unified monitoring** - One dashboard

**Drawbacks:**
- ❌ **Complex integration** - PyO3 bindings, build system
- ❌ **Maintenance burden** - Keep Rust + Python in sync
- ❌ **Testing complexity** - Two languages to debug

---

### Option 3: Extract Just the Config (QUICK WIN)

**Copy market definitions from terauss to expand Arbees:**

```python
# arbees_shared/config/non_sports_markets.py

NON_SPORTS_LEAGUES = [
    # Crypto markets
    LeagueConfig(
        league_code="crypto_btc",
        poly_prefix="btc",
        kalshi_series_game="KXBTCPRICE",
        description="Bitcoin price predictions"
    ),
    # Weather markets  
    LeagueConfig(
        league_code="weather_temp",
        poly_prefix="weather",
        kalshi_series_game="KXWEATHERTEMP",
        description="Temperature predictions"
    ),
    # Economic indicators
    LeagueConfig(
        league_code="econ_inflation",
        poly_prefix="inflation",
        kalshi_series_game="KXCPI",
        description="CPI/inflation reports"
    ),
]
```

**Benefits:**
- ✅ **Quick to implement** - Just config changes
- ✅ **Leverages existing Arbees** - No new services
- ✅ **Can use models OR pure arb** - Your choice

**Drawbacks:**
- ❌ **Arbees complexity remains** - Still have edge cases
- ❌ **No SIMD optimization** - Slower than Rust

---

## Recommended Approach

### Phase 1: Standalone Container (Week 1)

**Goal:** Get crypto/weather arbitrage running FAST with zero risk

1. **Extract terauss bot to own repo:**
   ```bash
   cp -r Polymarket-Kalshi-Arbitrage-bot/ ../arbees-pure-arb/
   cd ../arbees-pure-arb/
   ```

2. **Add crypto/weather market configs:**
   ```rust
   // src/config.rs
   
   // Add to get_league_configs():
   LeagueConfig {
       league_code: "crypto_btc",
       poly_prefix: "btc",
       kalshi_series_game: "KXBTC",
       kalshi_series_spread: None,
       kalshi_series_total: None,
       kalshi_series_btts: None,
   },
   LeagueConfig {
       league_code: "weather",
       poly_prefix: "weather",
       kalshi_series_game: "KXWEATHER",
       kalshi_series_spread: None,
       kalshi_series_total: None,
       kalshi_series_btts: None,
   },
   ```

3. **Build Docker image:**
   ```dockerfile
   FROM rust:1.75 as builder
   WORKDIR /app
   COPY . .
   RUN cargo build --release
   
   FROM debian:bookworm-slim
   COPY --from=builder /app/target/release/prediction-market-arbitrage /usr/local/bin/arb-bot
   CMD ["arb-bot"]
   ```

4. **Add to Arbees docker-compose.yml**

5. **Test in DRY_RUN mode for 24 hours**

6. **Go live with small position limits**

**Timeline:** 2-3 days

---

### Phase 2: Unified Monitoring Dashboard (Week 2)

**Goal:** See both systems in one place

1. **Add Rust bot metrics endpoint:**
   ```rust
   // src/metrics.rs
   
   #[derive(Serialize)]
   struct BotMetrics {
       total_arbs_found: u64,
       total_trades: u64,
       total_pnl: f64,
       active_positions: HashMap<String, i64>,
       circuit_breaker_status: String,
   }
   
   // HTTP endpoint on port 9091
   async fn metrics_handler() -> Json<BotMetrics> { ... }
   ```

2. **Poll from Arbees API:**
   ```python
   # services/api/unified_metrics.py
   
   @router.get("/api/metrics/unified")
   async def get_unified_metrics():
       # Fetch from Arbees
       arbees_metrics = get_arbees_metrics()
       
       # Fetch from Rust bot
       rust_metrics = await fetch_rust_bot_metrics("http://arb_bot_rust:9091/metrics")
       
       return {
           "arbees": arbees_metrics,
           "pure_arb": rust_metrics,
           "combined_pnl": arbees_metrics.pnl + rust_metrics.pnl
       }
   ```

3. **Update frontend dashboard:**
   ```typescript
   // frontend/src/pages/UnifiedDashboard.tsx
   
   <div className="grid grid-cols-2 gap-4">
     <MetricsCard title="Arbees (Probabilistic)" data={arbees} />
     <MetricsCard title="Pure Arbitrage (Rust)" data={rust} />
   </div>
   <MetricsCard title="Combined Performance" data={combined} />
   ```

**Timeline:** 3-4 days

---

### Phase 3: Shared Circuit Breaker (Week 3-4)

**Goal:** Coordinated risk management across both systems

1. **Central circuit breaker service:**
   ```python
   # services/circuit_breaker_central/
   
   class CentralCircuitBreaker:
       """
       Tracks positions and P&L across BOTH systems.
       Provides API for checking if trading allowed.
       """
       
       def __init__(self):
           self.arbees_positions = {}
           self.rust_positions = {}
       
       async def can_trade(self, system: str, market: str, size: int):
           total_position = self._get_total_position(market)
           total_pnl = self._get_total_pnl()
           
           # Check limits
           if total_position + size > MAX_POSITION:
               return False, "Max position exceeded"
           
           if total_pnl < -MAX_DAILY_LOSS:
               return False, "Max daily loss exceeded"
           
           return True, "OK"
   ```

2. **Modify Rust bot to check central CB:**
   ```rust
   // Before executing trade:
   let response = check_circuit_breaker(market, size).await?;
   if !response.allowed {
       warn!("Circuit breaker: {}", response.reason);
       return;
   }
   ```

3. **Modify Arbees to check central CB:**
   ```python
   # Before signal execution:
   allowed, reason = await central_cb.can_trade("arbees", game_id, size)
   if not allowed:
       logger.warning(f"Circuit breaker: {reason}")
       return
   ```

**Timeline:** 5-7 days

---

## Risk Analysis

### Risks of Running terauss Bot

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|-----------|
| **Duplicate positions** | Medium | High | Shared position tracking (Phase 3) |
| **Combined loss exceeds limits** | Low | High | Central circuit breaker (Phase 3) |
| **Rust bot crashes** | Low | Low | Docker restart policy + monitoring |
| **Market discovery conflicts** | Low | Low | Different league codes |
| **API rate limits hit** | Low | Medium | terauss has built-in rate limiting |

### Benefits vs Current Approach

**Current Arbees Risks:**
- ❌ Complex probability models can have bugs
- ❌ Game state tracking edge cases
- ❌ Home/away team matching issues (just fixed!)
- ❌ Sports-only (missing opportunities)

**terauss Bot Benefits:**
- ✅ Simple math - can't get it wrong
- ✅ Battle-tested code - already works
- ✅ Expands to crypto/weather - MORE opportunities
- ✅ Circuit breakers built-in - safer

---

## Implementation Recommendation

### My Strong Recommendation: Phase 1 Only (For Now)

**Why:**
1. **You made $2,000 in 2.5 hours** - Arbees is WORKING
2. **Just fixed home/away bug** - Arbees about to get even better
3. **terauss is proven** - Don't mess with success
4. **Expansion > Integration** - Get crypto/weather markets FIRST

**The Plan:**
```
Week 1: Run terauss bot standalone
├─ Add crypto/weather markets to config
├─ Deploy as separate Docker container
├─ Test in DRY_RUN for 24 hours
├─ Go live with conservative limits ($100 max position)
└─ Monitor for 7 days

Week 2: Evaluate results
├─ How many arbs found in crypto/weather?
├─ What's the P&L?
├─ Any issues?
└─ Decide if worth keeping

Only if successful:
Week 3-4: Add unified monitoring
```

**Don't integrate yet because:**
- Integration adds complexity
- Arbees is working well
- terauss is working well
- Keep them separate = lower risk

**Integrate later if:**
- Both systems profitable for 1+ month
- You want shared circuit breaker
- You want unified dashboard
- You have time to build PyO3 bindings

---

## Next Steps

1. **I'll create a Claude Code prompt to:**
   - Extract terauss bot to standalone service
   - Add crypto/weather/economics market configs
   - Create Docker setup
   - Add to Arbees docker-compose.yml
   - Set up monitoring endpoint

2. **You review and approve the plan**

3. **Claude Code implements it** (2-3 hours of work)

4. **You test in DRY_RUN** (24 hours)

5. **Go live with small positions** (week 1)

6. **Evaluate results** (week 2)

Ready for the implementation prompt?
