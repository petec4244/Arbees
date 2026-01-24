# PLANNING MODE PROMPT: Futures Tracking & Game Lifecycle Management

I need you to analyze and plan the implementation of two major features for the Arbees trading system:

1. **Futures/Pre-Game Prop Tracking** - Monitor upcoming games 24-48 hours before they start
2. **Game Lifecycle Management** - Automatically archive completed games and create historical analysis page

## Context

Read the complete specification at:
```
P:\petes_code\ClaudeCode\Arbees\CLAUDE_CODE_FUTURES_AND_ARCHIVING_PROMPT.md
```

This document contains:
- Complete business justification for both features
- Detailed architecture diagrams
- Full code implementations
- Database schemas
- Frontend components
- Docker configurations
- Testing strategies

## High-Level Overview

### Feature 1: Futures Tracking

**Business Value:**
- Markets list 24-48 hours before games start
- Early pricing is often inefficient (fewer sharp bettors)
- Opening lines can shift 5-10% before game starts
- Capture edge before sharp money arrives

**Example Opportunity:**
```
Tuesday 3pm: Lakers @ Celtics (Friday 7pm) markets listed
  - Opening: Lakers 55% ($0.55)
  - Your model: Lakers 62%
  - Edge: 7% (HUGE!)
  - Buy @ $0.55

Friday 6pm (1 hour before game):
  - Line moved to 60% ($0.60)
  - You're up 9% before game even starts!
```

**Key Components:**
- `FuturesMonitor` service - Discovers upcoming games
- Database tables - Track futures games and price history
- Frontend page - Shows upcoming games with line movement
- Handoff mechanism - Transitions to GameShard when game goes live

### Feature 2: Game Lifecycle Management

**Business Value:**
- Live games page cluttered with 50+ finished games
- Need clean historical data for ML training
- Enable post-game analysis

**Key Components:**
- `GameArchiver` service - Moves completed games to historical tables
- Database tables - Store archived games with full history
- Frontend page - Browse and analyze past games
- Auto-cleanup - Keeps live games page clean

## Your Task

**Phase 1: Architecture Analysis (START HERE)**

1. **Read the full specification** from the file above

2. **Analyze the current codebase structure:**
   - How does the current GameShard work?
   - Where does game discovery happen?
   - How are WebSocket connections managed?
   - Where is the database layer?

3. **Identify integration points:**
   - How will FuturesMonitor connect to existing services?
   - How will GameArchiver detect game completion?
   - What shared code can be reused?
   - What new infrastructure is needed?

4. **Create a dependency map:**
   - What needs to be built first?
   - What can be built in parallel?
   - What are the critical path items?

**Phase 2: Detailed Planning**

For each feature, create:

### PART 1: Futures Tracking Plan

**Services to Create:**
- [ ] `services/futures_monitor/monitor.py` - Main service
- [ ] `services/futures_monitor/Dockerfile` - Container config
- [ ] `services/futures_monitor/requirements.txt` - Dependencies

**Database Changes:**
- [ ] `futures_games` table - Track upcoming games
- [ ] `futures_price_history` table - Line movement over time
- [ ] `futures_signals` table - Early opportunities detected

**Frontend Components:**
- [ ] `frontend/src/pages/FuturesPage.tsx` - Main page
- [ ] Reuse `PropChart` component for line movement
- [ ] Add countdown timers
- [ ] Highlight opportunities (edge ≥ 5%)

**API Endpoints:**
- [ ] `GET /api/futures/games` - List upcoming games
- [ ] `GET /api/futures/games/{id}/prices` - Price history
- [ ] `GET /api/futures/games/{id}/signals` - Opportunities

**Integration Points:**
- [ ] ESPN client - Get upcoming schedule
- [ ] Market discovery - Find markets early
- [ ] WebSocket clients - Subscribe to price updates
- [ ] Orchestrator - Notify when to start live tracking

**Critical Questions to Answer:**
1. How do we determine when markets are first listed?
2. How do we hand off to GameShard when game goes live?
3. Should futures trades execute automatically or just alert?
4. What's the minimum edge threshold for futures (5%? 7%?)?

### PART 2: Game Lifecycle Plan

**Services to Create:**
- [ ] `services/archiver/archiver.py` - Main service
- [ ] `services/archiver/Dockerfile` - Container config
- [ ] `services/archiver/requirements.txt` - Dependencies

**Database Changes:**
- [ ] `historical_games` table - Completed games
- [ ] `historical_signals` table - All signals from game
- [ ] `historical_trades` table - All trades from game
- [ ] `historical_price_history` table - Complete price timeline
- [ ] Add `archived_at` column to `games` table

**Frontend Components:**
- [ ] `frontend/src/pages/HistoricalGamesPage.tsx` - Main page
- [ ] Filters (sport, date range, outcome)
- [ ] Summary stats dashboard
- [ ] Game detail view
- [ ] Price chart replay

**API Endpoints:**
- [ ] `GET /api/historical/games` - List archived games
- [ ] `GET /api/historical/games/{id}` - Game details
- [ ] `GET /api/historical/games/{id}/trades` - Trade history
- [ ] `GET /api/historical/games/{id}/chart` - Price replay

**Integration Points:**
- [ ] GameShard - Detect when game ends
- [ ] Database - Copy data to historical tables
- [ ] Orchestrator - Notify to stop tracking

**Critical Questions to Answer:**
1. When exactly should we mark a game as "ended"?
2. How long is the grace period (1 hour? 30 minutes?)?
3. Should we delete from live tables or just mark archived?
4. How do we handle position closing at game end?

**Phase 3: Implementation Strategy**

Create a phased rollout plan:

### Week 1: Futures Foundation
- [ ] Day 1-2: Database schema and migrations
- [ ] Day 3-4: FuturesMonitor service (basic polling)
- [ ] Day 5: Test with upcoming games

### Week 2: Futures Frontend
- [ ] Day 1-2: API endpoints
- [ ] Day 3-4: FuturesPage frontend
- [ ] Day 5: Integration testing

### Week 3: Archiving Backend
- [ ] Day 1-2: Database schema for historical tables
- [ ] Day 3-4: GameArchiver service
- [ ] Day 5: Test archival flow

### Week 4: Archiving Frontend
- [ ] Day 1-2: API endpoints for historical data
- [ ] Day 3-4: HistoricalGamesPage frontend
- [ ] Day 5: End-to-end testing

**Phase 4: Risk Analysis**

Identify potential issues:

### Futures Tracking Risks:
1. **Market discovery timing**
   - Risk: Markets not available when we check
   - Mitigation: Poll frequently, store discovery timestamp

2. **Handoff coordination**
   - Risk: Game starts but not picked up by GameShard
   - Mitigation: Notification system + 5min buffer

3. **False signals**
   - Risk: Too many low-quality futures signals
   - Mitigation: Higher edge threshold (5%+ vs 2% for live)

### Game Archiving Risks:
1. **Data loss during archival**
   - Risk: Transaction fails, data lost
   - Mitigation: Use database transactions (all-or-nothing)

2. **Game end detection**
   - Risk: Miss game end, never archive
   - Mitigation: Multiple status checks, fallback cleanup

3. **Performance degradation**
   - Risk: Historical queries slow down system
   - Mitigation: Indexes, pagination, separate tables

**Phase 5: Testing Plan**

### Futures Testing:
1. **Discovery Test:**
   - Find game 48h ahead
   - Verify markets discovered
   - Check opening line recorded

2. **Price Tracking Test:**
   - Monitor line movement
   - Verify history stored correctly
   - Check charts render

3. **Handoff Test:**
   - Wait for game start
   - Verify transition to live
   - Check no gaps in coverage

### Archiving Testing:
1. **Game End Test:**
   - Wait for game to finish
   - Verify status marked "ended"
   - Check positions closed

2. **Archival Test:**
   - Wait for grace period
   - Verify data copied
   - Check cleanup completed

3. **Historical Page Test:**
   - Browse archived games
   - Test all filters
   - Verify P&L calculations

## Key Architecture Decisions

You need to make decisions on:

1. **Should Futures and Archiving be separate services or combined?**
   - Recommendation: Separate (different concerns, different timing)

2. **How do we handle the game lifecycle states?**
   ```
   UPCOMING → LIVE → ENDED → ARCHIVED
      ↓        ↓      ↓         ↓
   Futures  GameShard  →   Historical
   ```

3. **Do we need a central orchestrator for state transitions?**
   - Current: Orchestrator manages GameShards
   - Future: Also manage Futures and Archiver?

4. **Database strategy:**
   - Option A: Separate tables for each stage
   - Option B: Single table with status field
   - Recommendation: Separate for performance

5. **WebSocket management:**
   - Futures: Subscribe to upcoming markets
   - Live: GameShard has active subscriptions
   - Archive: Unsubscribe after game ends

## Success Criteria

Your plan should result in:

### Futures Feature:
✅ Games discovered 24-48h before start
✅ Markets found as soon as they list
✅ Line movement tracked continuously
✅ Futures page displays upcoming games
✅ Handoff to live works smoothly
✅ No missed opportunities

### Archiving Feature:
✅ Games auto-archived 1h after completion
✅ Live games page only shows active games
✅ Historical page has all past games
✅ Filters work (sport/date/outcome)
✅ P&L calculations accurate
✅ Data integrity maintained

## Start Here

Begin by:

1. **Reading both features completely** from the specification file
2. **Analyzing the current codebase:**
   - `services/game_shard/shard.py` - How games are tracked
   - `services/orchestrator/orchestrator.py` - How services coordinate
   - `data_providers/espn/client.py` - How we get game schedules
   - Database schema - Current table structure

3. **Creating a detailed plan that addresses:**
   - What files need to be created
   - What files need to be modified
   - What the dependencies are
   - What the timeline looks like
   - What the risks are
   - How to test each component

4. **Asking clarifying questions** about:
   - Current system architecture
   - Existing patterns to follow
   - Preferred technology choices
   - Performance requirements

**Important:** This is PLANNING MODE. Create a comprehensive plan FIRST. Don't start implementing until we've reviewed and approved the plan.

What's your analysis and implementation plan for both features?
