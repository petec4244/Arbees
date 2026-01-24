# ARBEES ARCHITECTURE REVIEW & AWS DEPLOYMENT PLAN

## Current Container Architecture Analysis

### 📊 CURRENT STATE (11 Services)

```
┌─────────────────────────────────────────────────────────────┐
│  DATA LAYER (Stateful - MUST stay in same region)           │
├─────────────────────────────────────────────────────────────┤
│  1. timescaledb    - PostgreSQL + TimescaleDB (2GB RAM)     │
│  2. redis          - Message bus + cache (minimal RAM)      │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  CORE TRADING SERVICES (Latency-critical)                   │
├─────────────────────────────────────────────────────────────┤
│  3. game_shard     - Live game monitoring (2GB RAM)          │
│  4. position_manager - Trade execution + risk (1GB RAM)      │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  COORDINATION LAYER (Medium priority)                       │
├─────────────────────────────────────────────────────────────┤
│  5. orchestrator   - Game discovery + shard assignment       │
│  6. market_discovery_rust - Kalshi/Poly market matching     │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  NETWORK BYPASS (GEO-RESTRICTED ACCESS)                     │
├─────────────────────────────────────────────────────────────┤
│  7. vpn            - NordVPN container (Polymarket access)   │
│  8. polymarket_monitor - Price feed via VPN (network_mode)  │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  SUPPORTING SERVICES (Low priority)                         │
├─────────────────────────────────────────────────────────────┤
│  9. futures_monitor - Pre-game market tracking              │
│  10. archiver      - Historical data cleanup                │
│  11. ml_analyzer   - Nightly performance analysis           │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  USER INTERFACE (Can run anywhere)                          │
├─────────────────────────────────────────────────────────────┤
│  12. api          - REST API for frontend                   │
│  13. frontend     - React dashboard                         │
└─────────────────────────────────────────────────────────────┘
```

---

## 🚨 CRITICAL ISSUE: NO REAL ORDER EXECUTION

### What's Missing

**Current State:**
```python
# services/position_manager/position_manager.py
from markets.paper.engine import PaperTradingEngine  # ❌ PAPER TRADING ONLY!

self.paper_trader = PaperTradingEngine(...)
```

**What You Need:**
```python
# Real execution engines
from markets.kalshi.execution import KalshiExecutionEngine
from markets.polymarket.execution import PolymarketExecutionEngine

# Conditional: Use paper trader OR real traders
if os.getenv("PAPER_TRADING", "1") == "1":
    self.kalshi_trader = PaperTradingEngine(...)
    self.poly_trader = PaperTradingEngine(...)
else:
    self.kalshi_trader = KalshiExecutionEngine(...)   # ← DOESN'T EXIST YET!
    self.poly_trader = PolymarketExecutionEngine(...) # ← DOESN'T EXIST YET!
```

**Impact:**
- ❌ Can't place real trades on Kalshi
- ❌ Can't place real trades on Polymarket
- ✅ All signals generated correctly
- ✅ All infrastructure works
- **Just missing the final execution step!**

---

## 📐 ARCHITECTURE ISSUES

### Problem 1: Position Manager is a God Object

**Current responsibilities (TOO MANY!):**
```python
class PositionManager:
    # 1. Signal subscription ✓
    # 2. Risk evaluation ✓
    # 3. Order execution ❌ (only paper)
    # 4. Position tracking ✓
    # 5. Exit monitoring ✓
    # 6. Performance reporting ✓
    # 7. Arbitrage detection ✓
    # 8. Game end cleanup ✓
```

**Issues:**
- Single point of failure
- Can't scale order execution independently
- Hard to test individual components
- Mixing hot path (execution) with cold path (reporting)

**Recommended Split:**
```
position_manager/
├── signal_processor.py    - Subscribe to signals, validate
├── risk_evaluator.py      - Risk checks only
├── execution_engine.py    - ORDER PLACEMENT (the missing piece!)
├── position_tracker.py    - Track fills, P&L
└── exit_monitor.py        - Watch for exit conditions
```

---

### Problem 2: VPN + Polymarket Monitor Coupling

**Current Design:**
```yaml
vpn:
  image: qmcgaw/gluetun
  # VPN to bypass Polymarket geo-restrictions

polymarket_monitor:
  network_mode: "service:vpn"  # ← TIGHTLY COUPLED!
  # Shares VPN's network stack
```

**Issues:**
- VPN restarts = monitor restarts
- Can't scale monitor independently
- All Polymarket traffic goes through VPN (slow)
- Network debugging is painful

**Better Design:**
```
Option A: Dedicated Polymarket Shard
polymarket_shard:
  network_mode: "service:vpn"
  # ONLY Polymarket price fetching via VPN
  # Pushes to Redis for others to consume

All other services:
  # Access Polymarket prices via Redis (no VPN needed!)
```

---

### Problem 3: Orchestrator + Market Discovery Redundancy

**Current:**
```
orchestrator:
  - Discovers live games (ESPN API)
  - Assigns games to shards
  - Monitors shard health

market_discovery_rust:
  - Discovers Kalshi/Poly markets
  - Matches markets between platforms
  - Caches team mappings
```

**Issue:** Two services doing discovery!

**Better:**
```
orchestrator:
  - ONLY: Shard assignment, health monitoring

discovery_service:  # ← Combine both discoveries
  - ESPN game discovery
  - Kalshi/Poly market matching
  - Publish to Redis
```

---

### Problem 4: GameShard Does Everything

**Current GameShard:**
```python
class GameShard:
    # 1. ESPN game state polling ✓
    # 2. Kalshi price monitoring ✓
    # 3. Polymarket price monitoring ✓
    # 4. Win probability calculation ✓
    # 5. Signal generation ✓
    # 6. Multi-sport support ✓
```

**Issue:** Monolithic, hard to optimize per-platform

**Consideration:**
- Is this actually fine? (Maybe!)
- Each game is independent
- Sharing connections is good
- Could spawn multiple shards for load balancing

**Verdict:** GameShard is OKAY as-is. Don't split.

---

## 🎯 RECOMMENDED ARCHITECTURE

### Tier 1: Combine for Simplicity

**Merge these services:**

```
1. orchestrator + market_discovery_rust → discovery_coordinator
   Why: Both do discovery, reduce duplication
   Latency impact: None (discovery is infrequent)

2. archiver + ml_analyzer → analytics_service
   Why: Both are batch jobs (run nightly/hourly)
   Latency impact: None (not in hot path)

3. futures_monitor → Keep standalone OR merge into discovery_coordinator
   Why: Pre-game markets are separate concern
   Latency impact: None (futures are 24-48h ahead)
```

**Result: 11 services → 8 services**

---

### Tier 2: Split for Performance

**Extract execution engine from position_manager:**

```
Before:
  position_manager (does everything)

After:
  signal_processor     - Validates signals, checks risk
  execution_service    - ONLY places orders (Kalshi + Polymarket)
  position_tracker     - Tracks fills, calculates P&L
```

**Why:**
- Execution is latency-critical (need sub-second)
- Position tracking can be slower (1-2 second delay OK)
- Can scale execution independently
- Easier to add terauss-style Rust execution later

**Result: 8 services → 10 services**

---

### Tier 3: Create Dedicated Polymarket Price Shard

**Current problem:**
```
game_shard → Kalshi prices (direct, fast)
game_shard → Polymarket prices (via Redis, slower)
polymarket_monitor → VPN → Polymarket API → Redis
```

**Better:**
```
polymarket_price_shard:
  network_mode: "service:vpn"
  Responsibilities:
    - Subscribe to Polymarket WebSocket (via VPN)
    - Publish prices to Redis
    - Handle reconnections
    - ONLY Polymarket, nothing else

game_shard:
  - Reads Polymarket prices from Redis (fast!)
  - Reads Kalshi prices direct (fast!)
  - No VPN coupling
```

**Result: 10 services → 11 services**

---

## 🏗️ PROPOSED FINAL ARCHITECTURE (11 Services)

```
┌─────────────────────────────────────────────────────────────┐
│  DATA LAYER (AWS RDS + ElastiCache)                         │
├─────────────────────────────────────────────────────────────┤
│  1. timescaledb    - RDS PostgreSQL + TimescaleDB extension │
│  2. redis          - ElastiCache Redis cluster              │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  DISCOVERY & COORDINATION (ECS)                             │
├─────────────────────────────────────────────────────────────┤
│  3. discovery_coordinator - Game + market discovery         │
│     ├─ ESPN game polling                                    │
│     ├─ Kalshi/Poly market matching (Rust)                   │
│     ├─ Shard assignment logic                               │
│     └─ Health monitoring                                    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  MARKET DATA (ECS with VPN sidecar)                         │
├─────────────────────────────────────────────────────────────┤
│  4. polymarket_price_shard - Dedicated Polymarket feed      │
│     ├─ VPN sidecar container (gluetun)                      │
│     ├─ WebSocket to Polymarket (via VPN)                    │
│     └─ Publish to Redis                                     │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  CORE TRADING (ECS - Multi-instance)                        │
├─────────────────────────────────────────────────────────────┤
│  5. game_shard (x2-3 instances)                             │
│     ├─ Monitor 10-20 games each                             │
│     ├─ ESPN state polling                                   │
│     ├─ Kalshi prices (direct)                               │
│     ├─ Polymarket prices (from Redis)                       │
│     └─ Signal generation                                    │
│                                                              │
│  6. signal_processor                                        │
│     ├─ Subscribe to signals from shards                     │
│     ├─ Risk evaluation (RiskController)                     │
│     ├─ Position limits checking                             │
│     └─ Send approved signals to execution                   │
│                                                              │
│  7. execution_service ⭐ NEW - THE MISSING PIECE!            │
│     ├─ KalshiExecutionEngine (real orders!)                 │
│     ├─ PolymarketExecutionEngine (real orders!)             │
│     ├─ Concurrent leg execution                             │
│     ├─ Fill confirmation                                    │
│     └─ Retry logic                                          │
│                                                              │
│  8. position_tracker                                        │
│     ├─ Subscribe to fill confirmations                      │
│     ├─ Track open positions                                 │
│     ├─ Monitor for exits                                    │
│     ├─ Calculate P&L                                        │
│     └─ Close positions on game end                          │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  ANALYTICS & SUPPORT (ECS - Low priority)                   │
├─────────────────────────────────────────────────────────────┤
│  9. analytics_service (merged archiver + ml_analyzer)       │
│     ├─ Nightly archival (11pm)                              │
│     ├─ ML training (midnight)                               │
│     ├─ Hot wash report generation                           │
│     └─ Performance metrics                                  │
│                                                              │
│  10. futures_monitor                                        │
│      ├─ Pre-game market tracking                            │
│      ├─ Early line movement detection                       │
│      └─ Handoff to game_shard when live                     │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  USER INTERFACE (Can run locally OR on AWS)                 │
├─────────────────────────────────────────────────────────────┤
│  11. api + frontend (combined in nginx)                     │
│      ├─ FastAPI backend                                     │
│      ├─ React frontend (static files)                       │
│      └─ Read-only access to DB + Redis                      │
└─────────────────────────────────────────────────────────────┘
```

---

## 🌐 AWS vs LOCAL: What Goes Where?

### Deploy to AWS (ECS Fargate)

**MUST be in AWS:**
```
✅ timescaledb (RDS)        - Managed database, automatic backups
✅ redis (ElastiCache)      - Managed cache, high availability
✅ game_shard (x2-3)        - Need fast network to markets
✅ execution_service        - Latency-critical (order placement)
✅ position_tracker         - Near execution service
✅ polymarket_price_shard   - VPN sidecar works on ECS
✅ discovery_coordinator    - Needs to reach shards quickly
```

**SHOULD be in AWS:**
```
⚠️ analytics_service       - Can access DB easily, batch jobs
⚠️ futures_monitor         - Pre-game tracking, low urgency
```

**Cost estimate:**
- RDS PostgreSQL (db.t3.medium): ~$60/month
- ElastiCache Redis (cache.t3.micro): ~$15/month
- ECS Fargate (7 services, mixed sizes): ~$200-300/month
- **Total: ~$300-400/month**

---

### Keep Local (Your NH Machine)

**CAN run locally:**
```
🏠 api + frontend          - Just viewing data, latency OK
🏠 analytics_service       - Batch jobs, not time-sensitive
🏠 ml_analyzer             - Train models overnight
```

**Benefits:**
- Save AWS costs (~$50-100/month)
- Easy debugging/development
- SSH tunnel to AWS RDS/Redis

**Hybrid setup:**
```
Local Machine (NH):
├─ api (FastAPI)
├─ frontend (React dev server)
└─ SSH tunnel to AWS → RDS + Redis

AWS (ECS):
├─ Core trading services (game_shard, execution, etc.)
├─ Database (RDS)
└─ Cache (ElastiCache)
```

---

## ⚡ LATENCY OPTIMIZATION

### What Needs Low Latency (<100ms)

```
1. execution_service → Kalshi API
   - Order placement MUST be fast
   - Recommendation: ECS in us-east-1 (closest to Kalshi)

2. execution_service → Polymarket API
   - Via VPN, will be slower (150-300ms)
   - Acceptable trade-off for geo-bypass

3. game_shard → ESPN API
   - Live game data
   - Recommendation: ECS in us-east-1

4. game_shard → Kalshi API (prices)
   - WebSocket or REST polling
   - Recommendation: Same region as execution
```

### What Can Be Slow (1-5 seconds OK)

```
1. discovery_coordinator → ESPN (game discovery)
   - Runs every 5-10 minutes
   - Latency doesn't matter

2. position_tracker → Database writes
   - 1-2 second delay acceptable

3. analytics_service → Everything
   - Batch jobs, no rush

4. api + frontend → Database reads
   - User viewing, <1 second is fine
```

---

## 🔥 THE CRITICAL PATH (What Blocks Going Live)

### Current State: $2,000 Paper Profit ✅

```
✅ Signals generated correctly
✅ Risk limits working
✅ Position tracking works (paper)
✅ Architecture scales
✅ Database schema solid
```

### Blocking Issue: No Real Execution ❌

**What's missing:**
```python
# markets/kalshi/execution.py (DOESN'T EXIST!)
class KalshiExecutionEngine:
    async def place_limit_order(
        self,
        market_ticker: str,
        side: str,  # "yes" or "no"
        price_cents: int,
        quantity: int
    ) -> OrderResult:
        """
        Place limit order on Kalshi using their REST API.
        
        Reference implementation exists at:
        P:\petes_code\ClaudeCode\Arbees\kalshi_advanced_limit_demo.py
        
        Just needs to be:
        1. Extracted into clean class
        2. Added to execution_service
        3. Integrated with position_tracker
        """
        pass


# markets/polymarket/execution.py (DOESN'T EXIST!)
class PolymarketExecutionEngine:
    async def place_limit_order(
        self,
        token_id: str,
        side: str,  # "buy" or "sell"
        price: float,
        size: float
    ) -> OrderResult:
        """
        Place limit order on Polymarket using CLOB API.
        
        Reference: terauss bot has working implementation!
        See: Polymarket-Kalshi-Arbitrage-bot/src/polymarket_clob.rs
        
        Needs Python wrapper around Polymarket's CLOB API.
        """
        pass
```

---

## 📋 IMPLEMENTATION ROADMAP

### Phase 0: Extract Real Execution (1-2 days) ⭐ START HERE

**Priority: CRITICAL - This unblocks everything!**

```
Tasks:
1. Create markets/kalshi/execution.py
   - Extract from kalshi_advanced_limit_demo.py
   - Clean class interface
   - Add order status checking
   - Add fill confirmation

2. Create markets/polymarket/execution.py
   - Study terauss Rust implementation
   - Port to Python (or use py-clob-client library)
   - Implement limit orders
   - Handle authentication

3. Update position_manager to use real execution
   - Add PAPER_TRADING env var toggle
   - Wire up Kalshi/Poly execution engines
   - Test with $10 positions first!

4. Test end-to-end with SMALL positions
   - $10-20 per trade maximum
   - Monitor for 24 hours
   - Verify fills tracking correctly
```

**Deliverable:** Can place real orders on both platforms! 🎉

---

### Phase 1: Split Position Manager (3-5 days)

**Priority: HIGH - Needed before AWS**

```
Tasks:
1. Extract execution_service from position_manager
   services/execution_service/
   ├── execution.py (Kalshi + Poly engines)
   ├── order_manager.py (track in-flight orders)
   └── fill_handler.py (confirm fills)

2. Create signal_processor service
   services/signal_processor/
   ├── processor.py (subscribe to signals)
   ├── risk_checker.py (RiskController)
   └── redis_publisher.py (send to execution)

3. Refactor position_tracker
   services/position_tracker/
   ├── tracker.py (maintain position state)
   ├── exit_monitor.py (watch for exits)
   └── pnl_calculator.py (calculate P&L)

4. Update docker-compose.yml
   - 3 new services instead of 1 position_manager
   - Redis channels for communication
```

**Deliverable:** Modular architecture, easier to scale

---

### Phase 2: Merge Analytics (1-2 days)

**Priority: MEDIUM - Simplifies deployment**

```
Tasks:
1. Combine archiver + ml_analyzer
   services/analytics_service/
   ├── archiver.py (historical cleanup)
   ├── ml_trainer.py (model training)
   ├── report_generator.py (hot wash reports)
   └── scheduler.py (cron for both)

2. Single cron schedule
   - 11pm: Archive completed games
   - 12am: Train ML models
   - 12:30am: Generate reports

3. Update docker-compose.yml
```

**Deliverable:** 11 services → 9 services

---

### Phase 3: Extract Polymarket Price Shard (2-3 days)

**Priority: MEDIUM - Decouples VPN**

```
Tasks:
1. Create dedicated polymarket_price_shard
   services/polymarket_price_shard/
   ├── price_fetcher.py (WebSocket client)
   ├── redis_publisher.py (publish to Redis)
   └── reconnect_handler.py (handle disconnects)

2. Configure VPN sidecar
   - network_mode: "service:vpn"
   - All Polymarket traffic through VPN

3. Update game_shard
   - Remove direct Polymarket client
   - Read prices from Redis only

4. Update docker-compose.yml
```

**Deliverable:** Isolated Polymarket data pipeline

---

### Phase 4: Merge Discovery Services (2-3 days)

**Priority: LOW - Can defer**

```
Tasks:
1. Combine orchestrator + market_discovery_rust
   services/discovery_coordinator/
   ├── game_discovery.py (ESPN polling)
   ├── market_matcher/ (Rust binary)
   ├── shard_assigner.py (assign games to shards)
   └── health_monitor.py (check shard health)

2. Single service, multiple threads
   - Thread 1: Game discovery (every 5 min)
   - Thread 2: Market matching (on-demand)
   - Thread 3: Shard health (every 30 sec)

3. Update docker-compose.yml
```

**Deliverable:** 9 services → 8 services

---

### Phase 5: AWS Deployment (3-5 days)

**Priority: HIGH - After Phase 0 + 1**

```
Tasks:
1. Set up AWS infrastructure
   - RDS PostgreSQL with TimescaleDB
   - ElastiCache Redis cluster
   - ECS cluster in us-east-1
   - VPC with proper security groups

2. Create ECS task definitions
   - One per service
   - Proper CPU/memory limits
   - Environment variables from Secrets Manager

3. Deploy services in order
   Day 1: Database + Redis + discovery_coordinator
   Day 2: polymarket_price_shard (test VPN works!)
   Day 3: game_shard (test game monitoring)
   Day 4: execution_service + signal_processor (test with $10)
   Day 5: position_tracker + analytics

4. Test end-to-end
   - Small positions ($10-20)
   - Monitor for 48 hours
   - Verify fills + P&L correct

5. Gradually increase limits
   - Day 1-3: $10-20 per trade
   - Day 4-7: $50 per trade
   - Week 2: $100 per trade
   - Week 3+: Full limits
```

**Deliverable:** Production system on AWS! 🚀

---

## 💰 COST ANALYSIS

### AWS Costs (Monthly)

```
RDS PostgreSQL (db.t3.medium):
  - 2 vCPU, 4GB RAM
  - 100GB storage
  - Multi-AZ backup
  Cost: ~$60/month

ElastiCache Redis (cache.t3.micro):
  - 2 nodes for HA
  - 0.5GB RAM each
  Cost: ~$15/month

ECS Fargate:
  - discovery_coordinator (0.25 vCPU, 512MB): ~$10/month
  - polymarket_price_shard (0.25 vCPU, 512MB): ~$10/month
  - game_shard x3 (0.5 vCPU, 1GB each): ~$90/month
  - execution_service (0.5 vCPU, 1GB): ~$30/month
  - signal_processor (0.25 vCPU, 512MB): ~$10/month
  - position_tracker (0.25 vCPU, 512MB): ~$10/month
  - analytics_service (0.25 vCPU, 512MB): ~$5/month (runs nightly)
  Cost: ~$165/month

Data Transfer:
  - API calls to Kalshi/Polymarket
  - WebSocket data
  Cost: ~$20/month

Total: ~$260/month
```

### Local Costs (If keeping some local)

```
Local Machine (already have):
  - api + frontend
  - Development environment
Cost: $0 (electricity negligible)

VPN to AWS:
  - SSH tunnel for DB access
Cost: $0 (included in AWS)

Total: $0/month additional
```

**Hybrid Total: ~$260/month AWS + $0 local = $260/month**

---

## 🎯 RECOMMENDATIONS

### What to Do NOW (Priority Order)

1. **Phase 0: Real Execution (THIS WEEK!)**
   - Extract Kalshi execution from demo
   - Create Polymarket execution wrapper
   - Test with $10 positions
   - **This unblocks EVERYTHING!**

2. **Phase 1: Split Position Manager (NEXT WEEK)**
   - Modular before AWS
   - Easier to scale
   - Cleaner testing

3. **Phase 5: AWS Deployment (WEEK 3)**
   - After execution + refactor working locally
   - Start small ($10-20 positions)
   - Gradual scale-up

4. **Phase 2-4: Optimization (LATER)**
   - Once AWS working
   - Merge analytics
   - Extract Polymarket shard
   - Merge discovery

### What to Run Where

**AWS (us-east-1 ECS):**
- ALL core trading services
- Database (RDS)
- Redis (ElastiCache)

**Local (Your NH machine):**
- Frontend + API (optional, save $10/month)
- Development environment
- Testing

### Architecture Verdict

**Current architecture is 80% GOOD!**

✅ Game shards are well-designed
✅ Redis message bus works great
✅ Database schema is solid
✅ Orchestrator makes sense

❌ **Critical missing piece: Real execution**
⚠️ **Position manager too big** (split it)
⚠️ **VPN coupling awkward** (can fix later)

**Recommendation:** Don't over-engineer! Get real execution working FIRST, then refine architecture as you scale.

---

## 📄 NEXT STEPS

**I can create:**

1. **PLANNING_PROMPT_REAL_EXECUTION.md**
   - Claude Code prompt to build Kalshi + Polymarket execution engines
   - Extract from demo code + terauss bot
   - Wire into position_manager
   - Test with small positions

2. **PLANNING_PROMPT_POSITION_MANAGER_REFACTOR.md**
   - Split into 3 services
   - execution_service
   - signal_processor  
   - position_tracker

3. **AWS_DEPLOYMENT_GUIDE.md**
   - Step-by-step ECS setup
   - RDS + ElastiCache configuration
   - Task definitions
   - Testing checklist

**Which do you want first?** My strong recommendation: **#1 (Real Execution)** - this is the blocker!
