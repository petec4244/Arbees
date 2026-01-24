# PLANNING MODE PROMPT: terauss Pure Arbitrage Bot Integration

I need you to analyze and plan the integration of a battle-tested Rust arbitrage bot (terauss) into the Arbees ecosystem to expand market coverage beyond sports into crypto, weather, and economic markets.

## Context

Read the complete analysis at:
```
P:\petes_code\ClaudeCode\Arbees\ANALYSIS_TERAUSS_INTEGRATION.md
```

**Current Situation:**
- **Arbees (Python)**: Probabilistic sports trading with live models - Made $2,000 in 2.5 hours!
- **terauss (Rust)**: Pure arbitrage bot (YES + NO < $1.00 = profit) - Battle-tested, production-ready
- **Opportunity**: terauss currently only does sports, but can easily expand to crypto/weather/economics

**The Plan:**
Run terauss as a **standalone Docker container** alongside Arbees to capture pure arbitrage opportunities in non-sports markets.

---

## Source Material

You have access to the complete terauss bot codebase at:
```
P:\petes_code\ClaudeCode\Arbees\Polymarket-Kalshi-Arbitrage-bot\
```

**Key Files:**
- `src/main.rs` - Entry point, WebSocket orchestration
- `src/config.rs` - **League configurations** (THIS IS WHAT WE'LL MODIFY)
- `src/discovery.rs` - Market discovery and matching
- `src/execution.rs` - Order execution engine
- `src/circuit_breaker.rs` - Risk management
- `src/position_tracker.rs` - Position and P&L tracking
- `Cargo.toml` - Rust dependencies
- `README.md` - Documentation

---

## What terauss Does (Summary)

### Pure Arbitrage Strategy

**The Math:**
```
In prediction markets: YES + NO = $1.00 (always!)

Arbitrage opportunity when:
YES_ask (Platform A) + NO_ask (Platform B) < $1.00

Example:
Kalshi YES ask:  42¢
Poly NO ask:     56¢
Total cost:      98¢
Guaranteed:     100¢
Profit:           2¢ per contract (guaranteed!)
```

### Architecture Highlights

✅ **Lock-free orderbooks** - Atomic operations, no mutex contention
✅ **SIMD-accelerated arb detection** - Sub-millisecond latency
✅ **Concurrent order execution** - Both legs execute simultaneously
✅ **Circuit breaker protection** - Max position, max loss, consecutive errors
✅ **Position tracking** - Real-time P&L across both platforms
✅ **Market discovery with caching** - Intelligent 2-hour TTL, incremental updates
✅ **Production hardened** - Dry run mode, comprehensive logging

### Current Market Support

**Currently configured for sports only:**
- Soccer: EPL, Bundesliga, La Liga, Serie A, Ligue 1, UCL, UEL
- Basketball: NBA
- Football: NFL
- Hockey: NHL
- Baseball: MLB

**What we want to add:**
- 💰 Crypto (Bitcoin price, ETH predictions)
- 🌦️ Weather (temperature, precipitation)
- 📊 Economics (inflation, Fed rates, GDP)

---

## Your Task: Planning the Integration

### Phase 1: Create Standalone Service (START HERE)

**Goal:** Run terauss bot as separate Docker container to capture crypto/weather/economics arbitrage opportunities.

**Requirements:**
1. **Create new service directory structure:**
   ```
   services/pure_arb_bot/
   ├── Dockerfile
   ├── docker-compose.yml (fragment)
   ├── README.md
   └── src/ (copied from terauss repo)
       ├── main.rs
       ├── config.rs (MODIFIED - add new markets)
       ├── discovery.rs
       ├── execution.rs
       ├── circuit_breaker.rs
       ├── position_tracker.rs
       ├── ... (all other files)
   ```

2. **Modify config.rs to add new markets:**
   ```rust
   // src/config.rs
   
   pub fn get_league_configs() -> Vec<LeagueConfig> {
       vec![
           // CRYPTO MARKETS (NEW!)
           LeagueConfig {
               league_code: "crypto_btc",
               poly_prefix: "btc",
               kalshi_series_game: "KXBTC",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           LeagueConfig {
               league_code: "crypto_eth",
               poly_prefix: "eth",
               kalshi_series_game: "KXETH",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           
           // WEATHER MARKETS (NEW!)
           LeagueConfig {
               league_code: "weather_temp",
               poly_prefix: "weather",
               kalshi_series_game: "KXWEATHERTEMP",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           LeagueConfig {
               league_code: "weather_precip",
               poly_prefix: "weather",
               kalshi_series_game: "KXWEATHERPRECIP",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           
           // ECONOMICS MARKETS (NEW!)
           LeagueConfig {
               league_code: "econ_inflation",
               poly_prefix: "inflation",
               kalshi_series_game: "KXCPI",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           LeagueConfig {
               league_code: "econ_fed",
               poly_prefix: "fed",
               kalshi_series_game: "KXFED",
               kalshi_series_spread: None,
               kalshi_series_total: None,
               kalshi_series_btts: None,
           },
           
           // Keep existing sports markets...
           // (or comment them out to focus on new markets only)
       ]
   }
   ```

3. **Create production-ready Dockerfile:**
   ```dockerfile
   # Multi-stage build for small image size
   FROM rust:1.75 as builder
   WORKDIR /app
   COPY Cargo.toml Cargo.lock ./
   COPY src ./src
   RUN cargo build --release
   
   FROM debian:bookworm-slim
   RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
   COPY --from=builder /app/target/release/prediction-market-arbitrage /usr/local/bin/arb-bot
   CMD ["arb-bot"]
   ```

4. **Add to main docker-compose.yml:**
   ```yaml
   services:
     # Existing Arbees services...
     
     pure_arb_bot:
       build:
         context: ./services/pure_arb_bot
         dockerfile: Dockerfile
       container_name: arbees_pure_arb_bot
       environment:
         # Credentials (shared with Arbees)
         - KALSHI_API_KEY_ID=${KALSHI_API_KEY_ID}
         - KALSHI_PRIVATE_KEY_PATH=/secrets/kalshi_key.pem
         - POLY_PRIVATE_KEY=${POLY_PRIVATE_KEY}
         - POLY_FUNDER=${POLY_FUNDER}
         
         # Bot configuration
         - DRY_RUN=1  # Start in paper trading mode
         - RUST_LOG=info
         - FORCE_DISCOVERY=0
         
         # NEW: Enable only non-sports markets
         - ENABLED_LEAGUES=crypto_btc,crypto_eth,weather_temp,econ_inflation
         
         # Circuit breaker (conservative for new markets)
         - CB_ENABLED=true
         - CB_MAX_POSITION_PER_MARKET=50  # $50 max per market
         - CB_MAX_TOTAL_POSITION=200      # $200 total max
         - CB_MAX_DAILY_LOSS=500          # $500 daily loss limit
         - CB_MAX_CONSECUTIVE_ERRORS=3
         - CB_COOLDOWN_SECS=300           # 5 min cooldown
         
         # Arbitrage threshold
         - ARB_THRESHOLD=0.995  # 0.5% minimum profit
       
       volumes:
         - ./secrets:/secrets:ro
         - ./logs/pure_arb:/var/log/arb_bot
       
       restart: unless-stopped
       
       networks:
         - arbees_network
       
       # Health check
       healthcheck:
         test: ["CMD", "pgrep", "-f", "arb-bot"]
         interval: 30s
         timeout: 10s
         retries: 3
   ```

5. **Create monitoring metrics endpoint** (optional but recommended):
   ```rust
   // src/metrics.rs (NEW FILE)
   
   use serde::{Serialize, Deserialize};
   use std::collections::HashMap;
   
   #[derive(Serialize)]
   pub struct BotMetrics {
       pub uptime_secs: u64,
       pub total_arbs_found: u64,
       pub total_trades_executed: u64,
       pub total_pnl_cents: i64,
       pub active_positions: HashMap<String, i64>,
       pub circuit_breaker_status: CircuitBreakerStatus,
       pub markets_monitored: usize,
   }
   
   #[derive(Serialize)]
   pub struct CircuitBreakerStatus {
       pub enabled: bool,
       pub tripped: bool,
       pub reason: Option<String>,
       pub cooldown_remaining_secs: u64,
   }
   
   // Expose HTTP endpoint on port 9091
   pub async fn serve_metrics(state: Arc<GlobalState>) {
       // Serve metrics as JSON
   }
   ```

### Phase 2: Research Market Discovery (CRITICAL)

**Before implementing, you need to research:**

1. **What are the actual Kalshi series codes for crypto/weather/economics?**
   - The config above uses placeholder codes like "KXBTC", "KXWEATHERTEMP"
   - Need to find the REAL series codes by:
     - Browsing Kalshi API documentation
     - Calling Kalshi `/events` or `/series` endpoint
     - Looking at actual market tickers on Kalshi website

2. **What are the Polymarket prefixes/slugs for these markets?**
   - Need to research Polymarket Gamma API
   - Find market slugs for crypto predictions
   - Find market slugs for weather predictions
   - Find market slugs for economic indicators

3. **Are there enough markets to make this worthwhile?**
   - How many crypto markets exist on both platforms?
   - How many weather markets exist on both platforms?
   - Are they liquid enough?

**Research tasks:**
```
[ ] Call Kalshi API to list all available series
[ ] Filter for non-sports series (crypto, weather, economics)
[ ] For each series, note the series_ticker
[ ] Call Polymarket Gamma API to list markets
[ ] Find matching markets between platforms
[ ] Estimate opportunity size (how many arb-able markets?)
```

### Phase 3: Testing Strategy

1. **Local testing:**
   ```bash
   # Build the bot
   cd services/pure_arb_bot
   cargo build --release
   
   # Test in dry run mode
   DRY_RUN=1 \
   ENABLED_LEAGUES=crypto_btc,weather_temp \
   RUST_LOG=debug \
   cargo run --release
   ```

2. **Docker testing:**
   ```bash
   # Build image
   docker-compose build pure_arb_bot
   
   # Run in dry run mode
   docker-compose up pure_arb_bot
   
   # Check logs
   docker logs -f arbees_pure_arb_bot
   ```

3. **Validation checklist:**
   ```
   [ ] Bot starts successfully
   [ ] Discovers crypto/weather markets
   [ ] Finds matching markets on both platforms
   [ ] Detects arbitrage opportunities (if any)
   [ ] Circuit breaker works (test by simulating loss)
   [ ] Dry run mode prevents real trades
   [ ] Logs are clear and informative
   [ ] No crashes for 24 hours
   ```

4. **Go-live checklist:**
   ```
   [ ] Tested in DRY_RUN for 24+ hours
   [ ] No errors in logs
   [ ] Found at least 1 arbitrage opportunity
   [ ] Circuit breaker tested and working
   [ ] Position limits set conservatively
   [ ] Monitoring in place
   [ ] Set DRY_RUN=0
   [ ] Start with tiny positions ($10-20)
   [ ] Monitor closely for first hour
   [ ] Gradually increase limits if successful
   ```

---

## Critical Questions to Answer

**Before Implementation:**

1. **Market Discovery:**
   - What are the actual Kalshi series codes for crypto/weather/economics?
   - What are the Polymarket market slugs?
   - How do we discover markets that don't follow team-based patterns?
   - Do we need to modify the discovery logic?

2. **Market Matching:**
   - How do we match crypto markets between platforms?
   - Crypto: "Will BTC be > $100k on Feb 1?" needs exact date/price matching
   - Weather: "Will NYC temp be > 50°F on Jan 25?" needs location/date/threshold matching
   - Are the current matching algorithms sufficient?

3. **Trading Mechanics:**
   - Do crypto/weather markets have the same fee structure?
   - Are they as liquid as sports markets?
   - Do they resolve as reliably?
   - Any special considerations?

4. **Risk Management:**
   - Should we use different position limits for different market types?
   - Should crypto have higher limits (more liquid)?
   - Should weather have lower limits (less liquid)?
   - What's the right ARB_THRESHOLD for each market type?

**Architecture Decisions:**

1. **Code Reuse vs. Fork:**
   - Option A: Copy entire terauss repo into services/pure_arb_bot/
   - Option B: Keep as separate repo, use git submodule
   - Option C: Fork terauss repo, add as dependency
   - **Recommendation:** Option A (full copy) for independence

2. **Market Configuration:**
   - Should we keep sports markets enabled?
     - PRO: More opportunities
     - CON: Duplicate with Arbees (both systems trading same markets)
   - **Recommendation:** Disable sports initially, focus on new markets only

3. **Shared vs. Separate Credentials:**
   - Same API keys as Arbees?
     - PRO: Simple setup
     - CON: Combined rate limits
   - **Recommendation:** Same credentials, but terauss has rate limiting built-in

4. **Position Coordination:**
   - Track positions separately or shared?
   - If BTC market has position in both Arbees AND terauss, is that OK?
   - **Recommendation:** Start separate, combine later if needed

---

## Success Metrics

**Week 1 Goals:**
✅ Bot runs without crashes for 24+ hours
✅ Discovers crypto/weather markets successfully
✅ Finds at least 1 arbitrage opportunity
✅ Circuit breaker works correctly
✅ No errors in logs

**Week 2 Goals:**
✅ Executes first live trade successfully
✅ Position tracking accurate
✅ P&L calculation correct
✅ No circuit breaker trips (or only on valid conditions)

**Month 1 Goals:**
✅ Profitable (even if small)
✅ No major issues or bugs
✅ Consistent with Arbees performance
✅ Ready to scale up

---

## Your Planning Deliverables

Create a comprehensive plan that includes:

1. **Architecture Diagram:**
   - How pure_arb_bot fits into Arbees ecosystem
   - Data flow
   - Network topology

2. **File Structure:**
   - Complete directory layout
   - Which files to copy from terauss
   - Which files to modify
   - Which files to create new

3. **Market Discovery Research Plan:**
   - How to find Kalshi series codes
   - How to find Polymarket slugs
   - How to test market matching
   - Fallback if markets don't exist

4. **Testing Strategy:**
   - Local testing steps
   - Docker testing steps
   - Validation checklist
   - Go-live checklist

5. **Risk Analysis:**
   - What could go wrong?
   - How to mitigate each risk?
   - Monitoring strategy
   - Rollback plan

6. **Timeline:**
   - Research phase (market discovery)
   - Implementation phase (code changes)
   - Testing phase (dry run)
   - Go-live phase (small positions)

---

## Constraints & Guidelines

**Must-haves:**
- ✅ Start in DRY_RUN mode (no real trades until validated)
- ✅ Conservative circuit breaker limits initially
- ✅ Comprehensive logging
- ✅ Health checks and monitoring
- ✅ Graceful degradation (if markets not found, log and continue)

**Should-haves:**
- ✅ Metrics endpoint for monitoring
- ✅ README.md with setup instructions
- ✅ Environment variable documentation
- ✅ Clear separation from Arbees (no shared state initially)

**Nice-to-haves:**
- 🎯 Unified dashboard showing both systems
- 🎯 Shared circuit breaker (Phase 2)
- 🎯 Auto-scaling based on opportunity volume

**Must-nots:**
- ❌ Don't modify Arbees code (keep isolated)
- ❌ Don't share state between systems initially
- ❌ Don't go live without 24h dry run
- ❌ Don't use aggressive position limits initially

---

## Start Here

Begin your planning by:

1. **Reading the terauss codebase:**
   - Understand how market discovery works
   - Understand how leagues are configured
   - Understand the discovery caching mechanism
   - Understand the arbitrage detection logic

2. **Researching available markets:**
   - What crypto markets exist on Kalshi?
   - What crypto markets exist on Polymarket?
   - What weather markets exist on both?
   - What economic markets exist on both?

3. **Creating the implementation plan:**
   - Step-by-step guide to copy and modify code
   - Market configuration details
   - Docker setup
   - Testing procedure
   - Go-live checklist

4. **Asking critical questions:**
   - Are there enough markets to justify this?
   - Will the existing discovery logic work?
   - Do we need to modify matching algorithms?
   - What are the risks?

**Important:** This is PLANNING MODE. Create a comprehensive research and implementation plan FIRST. The plan should include a research phase to validate that there are actually enough crypto/weather/economics markets on both platforms to make this worthwhile.

What's your analysis and implementation plan?
