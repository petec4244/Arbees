# PLANNING MODE PROMPT: Game Lifecycle & ML Performance Analysis

I need you to analyze and plan the implementation of two critical features for improving the Arbees trading system based on real trading data and performance analysis:

## Context: Current Performance

**Paper Trading Results (2.5 hours):**
- Up $2,000 total
- Mix of wins and losses
- Some trades identified as "not good"
- Need systematic way to analyze what's working vs what's not

**Goal:** Build systems to automatically clean up completed games AND learn from trading performance to continuously improve.

---

## Feature 5: Game Lifecycle Management

### Business Problem

**Current State:**
- Games accumulate in the live games list even after they finish
- No easy way to review past performance by game
- Database fills with stale data
- Can't distinguish between active and completed games

**User Experience Issue:**
```
Live Games Dashboard:
- Lakers vs Celtics (FINAL - 3 hours ago)  ❌ Still showing
- Warriors vs Mavs (FINAL - 5 hours ago)   ❌ Still showing
- Heat vs Bucks (LIVE - Q3)                ✅ Actually live
- Nets vs 76ers (FINAL - yesterday)        ❌ Still showing
... 45 more finished games cluttering the view ...
```

**What We Need:**
```
Live Games Dashboard:
- Heat vs Bucks (LIVE - Q3) ✅ Only active games

Historical Games Page:
- Lakers vs Celtics (Final: 112-108) - P&L: +$450, Win Rate: 75%
- Warriors vs Mavs (Final: 98-105) - P&L: -$120, Win Rate: 50%
- Filter by: Sport, Date Range, Outcome (Profitable/Loss)
- Sortable by P&L, Win Rate, Trade Count
```

### Requirements

#### Auto-Archive Completed Games

**Trigger:** Game status changes to "final" or "completed"
**Grace Period:** Wait 30-60 minutes (for late score corrections)
**Actions:**
1. Mark game as "ended" in database
2. Close any open positions at final score
3. Calculate final P&L for the game
4. Move to historical tables
5. Remove from live GameShard tracking

#### Historical Database Schema

**Tables Needed:**
```sql
-- Archived games with summary stats
archived_games:
  - game_id, sport, teams, scores
  - ended_at, archived_at
  - total_pnl, win_rate, total_trades, signals_generated

-- All trades from archived games
archived_trades:
  - Copy of trades table structure
  - Links back to archived_games

-- All signals from archived games  
archived_signals:
  - Copy of trading_signals structure
  - Helps analyze missed opportunities

-- Complete price history (TimescaleDB)
archived_price_history:
  - Full price timeline for replay/analysis
```

#### Historical Games Page (Frontend)

**Features:**
- Table view of all completed games
- Filters:
  - Sport (NBA, NFL, NHL, etc.)
  - Date range (from/to)
  - Outcome (Profitable, Loss, Break-even)
- Sortable columns:
  - P&L (highest to lowest)
  - Win rate (best to worst)
  - Trade count
- Game detail view:
  - Final score
  - Trade-by-trade breakdown
  - Price chart replay
  - Signal history (generated vs executed)
  - What worked / what didn't

**Summary Dashboard:**
```
┌─────────────┬─────────────┬─────────────┬─────────────┐
│ Total Games │  Total P&L  │  Win Rate   │Total Trades │
│     47      │  +$2,450    │    68%      │     156     │
└─────────────┴─────────────┴─────────────┴─────────────┘
```

### Implementation Components

#### 1. Game End Detection (services/game_shard/shard.py)

**Modify GameShard to detect game completion:**
```python
async def _handle_game_state_update(self, game_id, new_state, ...):
    # ... existing logic ...
    
    # NEW: Detect game end
    if new_state.status in ["final", "completed", "closed"]:
        await self._handle_game_end(game_id, new_state)

async def _handle_game_end(self, game_id, final_state):
    """
    Handle game ending:
    1. Mark as ended in DB
    2. Close open positions
    3. Calculate final P&L
    4. Stop monitoring
    """
```

#### 2. Archiver Service (services/archiver/archiver.py)

**New background service:**
```python
class GameArchiver:
    """
    Polls for completed games and archives them.
    
    Workflow:
    1. Find games with status='ended' and ended_at > 1 hour ago
    2. Copy game + trades + signals + prices to historical tables
    3. Mark original game as archived
    4. Optionally: Delete from live tables (or keep with archived flag)
    """
    
    async def run(self):
        while True:
            await self._archive_completed_games()
            await asyncio.sleep(300)  # Every 5 minutes
```

#### 3. Archive Trigger (services/orchestrator/orchestrator.py)

**Orchestrator coordination:**
- GameShard marks game "ended"
- Orchestrator notifies Archiver
- Archiver waits grace period then archives
- Orchestrator stops GameShard for that game

#### 4. Frontend (frontend/src/pages/HistoricalGames.tsx)

**React component with:**
- Filter controls (sport, date, outcome)
- Games table (sortable, paginated)
- Summary stats
- Detail modal/page per game

---

## Feature 6: ML Performance Analysis System

### Business Problem

**Current Situation:**
- Made $2,000 in 2.5 hours ✅
- But some trades were "not good" ❌
- No systematic way to identify:
  - Which strategies work best?
  - Which market types are most profitable?
  - What edge threshold is optimal?
  - When to trade vs when to wait?
  - Why did we miss opportunities?
  - What caused losing trades?

**Manual Analysis is Not Scalable:**
- Can't review every trade manually
- Patterns hard to see without data science
- Parameter tuning is guesswork
- No way to track improvement over time

### What We Need: Self-Improving Trading System

**Goal:** Build an ML system that:
1. **Analyzes** every trade, signal, and opportunity
2. **Learns** patterns in wins vs losses
3. **Suggests** concrete parameter improvements
4. **Generates** nightly performance reports
5. **Predicts** optimal settings for tomorrow

### Requirements

#### ML Analysis Capabilities

**Trade Analysis:**
- Success rate by:
  - Sport (NBA vs NFL vs NHL)
  - Market type (winner vs spread vs total)
  - Edge size (1-2% vs 2-3% vs 3%+)
  - Time of game (Q1 vs Q4)
  - Day of week
  - Time of day (afternoon vs evening)
- Identify:
  - Best performing conditions
  - Worst performing conditions
  - Edge threshold sweet spot
  - Position sizing optimization

**Opportunity Analysis:**
- Signals generated but not executed (why?)
- Compare executed vs missed:
  - What was the edge on missed opportunities?
  - What was the outcome if we had traded?
  - Pattern recognition: "We always miss X type of signal"

**Loss Analysis:**
- Categorize losing trades:
  - Edge too small (false signal)
  - Market moved against us (bad luck)
  - Execution delay (too slow)
  - Wrong side (home/away bug - now fixed!)
  - Model error (game state misread)

#### Nightly "Hot Wash" Report

**Daily Performance Summary:**
```markdown
# Arbees Trading Report - January 22, 2026

## Executive Summary
- **Daily P&L:** +$2,450 (🔥 Best day this week!)
- **Win Rate:** 68% (34 wins / 50 total trades)
- **Avg Edge:** 2.8%
- **Opportunities:** 127 detected, 50 executed (39% capture rate)

## What Went Well ✅
1. **NBA spread bets crushed it:** 85% win rate (17/20 trades)
2. **Edge threshold 2.5%+ had 92% success rate**
3. **Q3/Q4 trades outperformed early game** (75% vs 58%)

## What Needs Improvement ⚠️
1. **NFL totals struggled:** 40% win rate (4/10 trades) - AVOID THESE
2. **Missed 77 opportunities** due to:
   - 45 (58%) → Execution too slow (need faster WebSocket?)
   - 22 (29%) → Edge threshold too high (lower from 2% to 1.5%?)
   - 10 (13%) → Circuit breaker activated (review limits)

## Losing Trades Analysis 💸
**Total Losses:** -$680 (16 trades)

Top 3 Biggest Losses:
1. Warriors vs Mavs Q2 (-$180) → Edge was only 1.2%, too thin
2. Heat vs Nets Q4 (-$150) → Late game volatility, market moved fast
3. Celtics vs Bucks Q1 (-$120) → Execution delay (5 second lag)

Pattern: **Small edge trades (<2%) lost 71% of the time**

## Recommended Changes 🎯
1. **Increase minimum edge threshold:**
   - Current: 2.0%
   - Recommended: 2.5% (would eliminate 12 losing trades)
   - Backtest shows: Win rate improves from 68% → 76%

2. **Avoid NFL totals markets:**
   - Win rate: 40% (too unpredictable)
   - Suggestion: Disable totals for NFL

3. **Increase position size on NBA spreads:**
   - Current: $100
   - Recommended: $150 (85% win rate justifies it)

4. **Lower capture threshold in Q4:**
   - Currently miss 60% of Q4 opportunities
   - Recommendation: Lower edge threshold to 1.8% in Q4 only

## Tomorrow's Forecast 📊
- **15 games scheduled** (12 NBA, 3 NHL)
- **Expected opportunities:** 80-120
- **Projected P&L:** +$1,800 - $2,200 (based on parameter updates)
- **Confidence:** High (NBA-heavy schedule = favorable)

## 7-Day Performance Trend 📈
```
Jan 15: +$1,200 (62% win rate)
Jan 16: +$890   (59% win rate)
Jan 17: -$340   (48% win rate) ⚠️ Bad day
Jan 18: +$1,450 (71% win rate)
Jan 19: +$1,680 (69% win rate)
Jan 20: +$2,100 (73% win rate) 🔥
Jan 21: +$2,450 (68% win rate) 🔥

Trend: ↗️ Improving! Win rate up 10% week-over-week
```
```

#### ML Model Architecture

**Feature Engineering:**
```python
features = {
    # Trade context
    'sport': categorical,  # NBA, NFL, NHL
    'market_type': categorical,  # winner, spread, total
    'edge_size': continuous,  # 0.01 to 0.15
    'time_of_game': categorical,  # Q1, Q2, Q3, Q4
    'score_differential': continuous,  # -20 to +20
    
    # Timing features
    'time_of_day': categorical,  # afternoon, evening, night
    'day_of_week': categorical,  # Mon-Sun
    'games_today': continuous,  # 0-15
    
    # Market features
    'market_volume': continuous,
    'price_volatility': continuous,
    'time_since_market_open': continuous,
    
    # Position features
    'position_size': continuous,
    'home_or_away': categorical,
}

targets = {
    'trade_success': binary,  # 0 or 1
    'pnl_amount': regression,  # dollars
    'edge_accuracy': regression,  # model edge vs actual edge
}
```

**Models to Train:**
1. **Trade Success Classifier** (Random Forest)
   - Input: Features above
   - Output: Probability of winning trade
   - Use: Filter out low-probability trades

2. **P&L Predictor** (XGBoost Regression)
   - Input: Features + predicted success
   - Output: Expected P&L
   - Use: Position sizing (bet more on high-expected-value)

3. **Opportunity Ranker** (LightGBM)
   - Input: Signal features
   - Output: Priority score
   - Use: Which signals to execute first

4. **Parameter Optimizer** (Bayesian Optimization)
   - Input: Historical performance + parameter settings
   - Output: Optimal edge threshold, position sizes
   - Use: Continuous parameter tuning

### Implementation Components

#### 1. ML Analyzer Service (services/ml_analyzer/analyzer.py)

```python
class MLAnalyzer:
    """
    Analyzes trading performance and generates insights.
    
    Runs:
    - Nightly at 11pm (after markets close)
    - On-demand via API
    """
    
    async def run_nightly_analysis(self):
        """
        Full analysis pipeline:
        1. Load today's trades from DB
        2. Extract features
        3. Update ML models
        4. Generate insights
        5. Create hot wash report
        6. Deliver report (email/Slack/save)
        """
        
        # Load data
        trades = await self._load_todays_trades()
        signals = await self._load_todays_signals()
        
        # Analyze
        insights = await self._analyze_performance(trades, signals)
        
        # Generate report
        report = await self._generate_hot_wash(insights)
        
        # Deliver
        await self._deliver_report(report)
```

#### 2. ML Models (services/ml_analyzer/models.py)

```python
class TradeSuccessModel:
    """Random Forest classifier for trade success prediction."""
    
    def train(self, historical_trades):
        X = self._extract_features(historical_trades)
        y = self._extract_labels(historical_trades)
        
        self.model = RandomForestClassifier(n_estimators=100)
        self.model.fit(X, y)
        
        # Save feature importance
        self.feature_importance = dict(zip(
            X.columns,
            self.model.feature_importances_
        ))
    
    def predict_success_probability(self, trade_features):
        return self.model.predict_proba(trade_features)[0][1]

class ParameterOptimizer:
    """Suggests optimal parameter settings."""
    
    def optimize_edge_threshold(self, historical_trades):
        """
        Find edge threshold that maximizes win rate.
        
        Method:
        1. Try thresholds from 1% to 5% in 0.1% increments
        2. For each threshold, backtest on historical trades
        3. Calculate: win_rate, total_pnl, trade_count
        4. Return threshold that maximizes Sharpe ratio
        """
```

#### 3. Report Generator (services/ml_analyzer/report_generator.py)

```python
class HotWashReportGenerator:
    """Generates nightly performance reports."""
    
    def generate_markdown_report(self, insights: PerformanceInsights) -> str:
        """Create markdown report with analysis and recommendations."""
        
        report = f"""
# Arbees Trading Report - {insights.date}

## Executive Summary
- Daily P&L: ${insights.total_pnl:,.2f}
- Win Rate: {insights.win_rate:.1%}
- Total Trades: {insights.total_trades}

## Top Performers
{self._format_top_strategies(insights.best_strategies)}

## Areas for Improvement
{self._format_improvement_areas(insights.weaknesses)}

## Recommended Changes
{self._format_recommendations(insights.recommendations)}

## Tomorrow's Forecast
{self._format_forecast(insights.forecast)}
        """
        
        return report
    
    def generate_html_report(self, insights) -> str:
        """Create HTML report with charts."""
        
    def generate_pdf_report(self, insights) -> bytes:
        """Create PDF report for archival."""
```

---

## Your Planning Task

**Phase 1: Architecture Analysis**

1. **Understand Current System:**
   - How are games currently tracked?
   - Where is game state stored?
   - How does GameShard know when a game ends?
   - What triggers game cleanup currently (if anything)?

2. **Database Assessment:**
   - Current schema for games, trades, signals
   - Do we have TimescaleDB enabled?
   - What indexes exist?
   - How much data volume are we dealing with?

3. **Integration Points:**
   - GameShard → Archiver communication
   - Orchestrator → Archiver coordination
   - ML Analyzer → Database queries
   - Report delivery mechanisms

**Phase 2: Detailed Implementation Plan**

Create a plan that addresses:

### Game Lifecycle Management

**Week 1: Backend**
- [ ] Database schema for archived tables
- [ ] GameShard modifications (detect game end)
- [ ] Archiver service implementation
- [ ] Orchestrator integration

**Week 2: Frontend**
- [ ] Historical Games page
- [ ] Filters and sorting
- [ ] Game detail view
- [ ] Summary statistics

### ML Performance Analysis

**Week 3: Data Pipeline**
- [ ] Feature extraction from trades/signals
- [ ] Database queries for historical data
- [ ] Data preprocessing and cleaning

**Week 4: ML Models**
- [ ] Train initial models on existing data
- [ ] Validate model performance
- [ ] Implement prediction endpoints

**Week 5: Reporting**
- [ ] Hot wash report generator
- [ ] Delivery mechanisms (email, Slack, file)
- [ ] Scheduling (nightly at 11pm)

**Phase 3: Critical Questions**

Answer these questions in your plan:

**Game Lifecycle:**
1. When exactly should we mark a game "ended"? (What statuses count?)
2. What's the grace period before archiving? (30 min? 1 hour?)
3. Should we delete from live tables or just mark archived?
4. How do we handle games that never finish (suspended, postponed)?
5. What happens to open positions when game ends unexpectedly?

**ML Analysis:**
1. What's the minimum amount of historical data needed to train models?
2. How often should models be retrained? (Daily? Weekly?)
3. What ML libraries should we use? (scikit-learn? XGBoost? Both?)
4. How do we validate model predictions aren't overfitting?
5. What format for reports? (Markdown? HTML? PDF? All three?)
6. How do we deliver reports? (Email? Slack? Dashboard? File save?)
7. Should recommendations auto-apply or require manual approval?

**Phase 4: Risk Analysis**

Identify and mitigate risks:

### Game Lifecycle Risks:
1. **Data Loss During Archival**
   - Risk: Transaction fails halfway, data corrupted
   - Mitigation: Use database transactions (atomic operations)

2. **Position Closing Logic**
   - Risk: Open positions not closed properly at game end
   - Mitigation: Comprehensive position tracking + reconciliation

3. **Database Performance**
   - Risk: Historical queries slow down live trading
   - Mitigation: Separate tables + indexes + TimescaleDB compression

### ML Analysis Risks:
1. **Overfitting**
   - Risk: Models learn noise, not signal
   - Mitigation: Cross-validation, holdout sets, simple models first

2. **Cold Start Problem**
   - Risk: Not enough data initially to train good models
   - Mitigation: Start with simple statistics, add ML as data grows

3. **Auto-Apply Recommendations**
   - Risk: ML suggests bad parameters, system auto-applies, loses money
   - Mitigation: Recommendations require manual review and approval

**Phase 5: Success Metrics**

Define how we measure success:

### Game Lifecycle:
✅ Live games page only shows active games  
✅ Games archived within 2 hours of completion  
✅ 100% data integrity (no lost trades/signals)  
✅ Historical page loads in < 2 seconds  
✅ Filters work correctly  
✅ P&L calculations match paper trader  

### ML Analysis:
✅ First hot wash report generated  
✅ Reports delivered successfully (email/Slack)  
✅ Model accuracy > 60% (better than random)  
✅ At least 3 actionable recommendations per report  
✅ Recommendations improve win rate when applied  
✅ 7-day and 30-day trend tracking working  

**Phase 6: Testing Strategy**

### Game Lifecycle Testing:
1. **Happy Path:**
   - Game finishes normally
   - Archiver picks it up
   - Data copied correctly
   - Live page updated

2. **Edge Cases:**
   - Game suspended (what happens?)
   - Score correction after "final" (grace period catches it)
   - Multiple games ending simultaneously
   - Archiver crashes mid-archive (recovery?)

3. **Performance:**
   - Archive 100 games at once
   - Query historical with 10,000 games
   - Verify no degradation to live trading

### ML Analysis Testing:
1. **Model Validation:**
   - Train on first 80% of data
   - Test on last 20%
   - Verify accuracy > baseline

2. **Report Generation:**
   - Generate report with real data
   - Verify all sections populated
   - Check calculations are correct

3. **Recommendations:**
   - Generate recommendations
   - Backtest on historical data
   - Verify they would have improved performance

---

## Deliverables

Your plan should include:

1. **Architecture Diagrams:**
   - Game lifecycle state machine
   - Data flow for archival process
   - ML analysis pipeline
   - Report generation and delivery

2. **Database Schema:**
   - All new tables with columns
   - Indexes needed for performance
   - Migration scripts

3. **API Specifications:**
   - Endpoints for historical games
   - Endpoints for ML insights
   - Request/response formats

4. **Timeline:**
   - Week-by-week breakdown
   - Dependencies (what blocks what)
   - Milestones

5. **Risk Mitigation:**
   - Top 5 risks for each feature
   - Mitigation strategies
   - Contingency plans

---

## Start Here

Begin by:

1. **Reading current codebase:**
   - `services/game_shard/shard.py` - Game tracking
   - `services/orchestrator/orchestrator.py` - Service coordination
   - Database schema - Current tables
   - `services/paper_trader/trader.py` - Trade execution

2. **Analyzing your real data:**
   - You mentioned $2,000 profit in 2.5 hours
   - Some trades were "not good"
   - What patterns can you see manually?
   - What questions would you want the ML system to answer?

3. **Creating the plan:**
   - Break down into phases
   - Identify dependencies
   - Estimate effort
   - List risks

4. **Asking questions:**
   - What's unclear about requirements?
   - What technical constraints exist?
   - What are the priorities?

**Important:** This is PLANNING MODE. Create a comprehensive plan FIRST before any implementation. The plan should be detailed enough that someone could implement it without asking questions.

What's your analysis and implementation plan for both features?
