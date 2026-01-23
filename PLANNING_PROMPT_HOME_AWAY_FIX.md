# PLANNING MODE PROMPT: Fix Home/Away Team Price Matching Bug

I need you to analyze and fix a critical bug in the Arbees trading system where we're comparing probabilities from different teams, causing incorrect edge calculations and wrong trade execution.

## Context

Read the complete specification at:
```
P:\petes_code\ClaudeCode\Arbees\CLAUDE_CODE_HOME_AWAY_FIX_PROMPT.md
```

This document contains:
- Detailed problem explanation with examples
- The exact bug (comparing home team model prob to either team's market price)
- Complete solution strategy
- Code examples for all required changes
- Test cases to verify the fix
- Success criteria

## The Core Issue

**Current Broken Behavior:**
```python
# We calculate edge as:
edge = model_prob - market_prob

# BUT:
# - model_prob is ALWAYS the HOME team's win probability
# - market_prob could be EITHER home OR away team's contract price
# - This means we might be comparing Lakers 65% to Celtics 48% price! ❌
```

**Example Bug:**
```
Game: Lakers (home) @ Celtics (away)
Model: Lakers 65% to win
Markets: Lakers YES $0.52, Celtics YES $0.48

Current bug:
- If we get Celtics price: edge = 0.65 - 0.48 = 17% ❌ WRONG!
  (Comparing Lakers win prob to Celtics win price - nonsense!)

Correct calculation:
- Should use Lakers price: edge = 0.65 - 0.52 = 13% ✅
  (Both are Lakers probabilities)
```

## Your Task

**Phase 1: Planning & Analysis (START HERE)**

1. **Read the full specification** from the file above
2. **Locate the affected code:**
   - `services/game_shard/shard.py` - Main bug location
   - `services/paper_trader/trader.py` - Execution bug
   - `arbees_shared/models/game.py` - May need model updates

3. **Identify the specific methods that need changes:**
   - `GameContext` dataclass
   - `_handle_ws_price_update()` method
   - `_generate_signals()` method
   - Paper trader execution methods

4. **Create a detailed plan** that includes:
   - Which files will be modified
   - Which new methods will be added
   - What the change flow will be
   - How you'll verify the fix works

**Phase 2: Implementation Strategy**

The fix has two main approaches (choose APPROACH 1):

**APPROACH 1: Track Team-Specific Prices (RECOMMENDED)**
```python
# Add to GameContext:
home_team_prices: dict[Platform, float]  # HOME team YES prices
away_team_prices: dict[Platform, float]  # AWAY team YES prices

# Then in signal generation:
if prob_change > 0:  # Home team increased
    market_prob = home_team_prices[platform]  # Use HOME price
    edge = new_prob - market_prob  # ✅ Both HOME team
else:  # Home team decreased (away increased)
    market_prob = away_team_prices[platform]  # Use AWAY price
    away_prob = 1.0 - new_prob
    edge = away_prob - market_prob  # ✅ Both AWAY team
```

**Required Helper Methods:**
- `_is_home_team_market(market_id, home_team)` - Identify if market is for home team
- `_is_away_team_market(market_id, away_team)` - Identify if market is for away team
- `_get_team_code(team_name)` - Convert "Los Angeles Lakers" → "LAL"
- `_get_best_price(team_prices)` - Get best available price for team

**Phase 3: Critical Changes Needed**

1. **Update GameContext dataclass:**
   ```python
   @dataclass
   class GameContext:
       # ... existing fields ...
       home_team_prices: dict[Platform, float] = field(default_factory=dict)
       away_team_prices: dict[Platform, float] = field(default_factory=dict)
   ```

2. **Modify _handle_ws_price_update():**
   - Determine which team the market price is for
   - Populate correct team-specific price dict
   - Add logging to show which team's price was updated

3. **Fix _generate_signals():**
   - When prob_change > 0: Use home_team_prices
   - When prob_change < 0: Use away_team_prices AND convert model_prob
   - Ensure edge calculation compares same team's probs

4. **Fix paper_trader execution:**
   - Find market for SPECIFIC team (not just any market)
   - Execute on correct team's contract
   - Verify we're buying the right team's YES

**Phase 4: Testing Requirements**

Create tests for:
1. **Home team probability increases** → Should use home team price
2. **Home team probability decreases** → Should use away team price
3. **Paper trader execution** → Should buy correct team's contract

**Expected log output after fix:**
```
[GameShard] HOME team price: 0.520 (Lakers)
[GameShard] AWAY team price: 0.480 (Celtics)
[GameShard] Model: Lakers 65% (+5% change)
[GameShard] Using HOME team price for edge calc
[SIGNAL] BUY Lakers, edge: 13% (65% - 52%) ✅
[PaperTrader] Executing on Lakers market ✅
```

## Success Criteria

Your implementation is successful when:

✅ GameContext tracks home_team_prices and away_team_prices separately
✅ _handle_ws_price_update correctly categorizes prices by team
✅ _generate_signals uses correct team's price for edge calculation
✅ Paper trader executes on correct team's contract
✅ Logs clearly show which team's price is being used
✅ Edge calculations make sense (comparing same team's probs)
✅ Test cases pass

## Important Constraints

1. **Team Detection is Critical:**
   - Kalshi tickers: `KXNBA-LAL-BOS-20260122` (LAL is team 1, BOS is team 2)
   - Polymarket: Need to check cached metadata or question text
   - Must handle both platforms correctly

2. **Don't Break Existing Code:**
   - Keep `ctx.market_prices` for backward compatibility
   - Only add new fields, don't remove existing ones
   - Existing code should continue to work

3. **Logging is Essential:**
   - Log which team's price is being used
   - Log when team cannot be determined
   - Log edge calculations with team context

## Validation Steps

After implementation:

1. **Code Review:**
   - [ ] All required methods added
   - [ ] Team-specific price tracking implemented
   - [ ] Edge calculations use correct team prices
   - [ ] Paper trader uses correct team markets

2. **Log Analysis:**
   - [ ] Logs show "HOME team price: X (TeamName)"
   - [ ] Logs show "AWAY team price: Y (TeamName)"
   - [ ] Edge calculations show which price was used
   - [ ] No warnings about "cannot determine team"

3. **Behavior Verification:**
   - [ ] When Lakers prob increases → Uses Lakers price
   - [ ] When Lakers prob decreases → Uses Celtics price
   - [ ] Paper trader buys correct team's contract
   - [ ] Edge percentages make sense (1-5% typical)

## Start Here

Begin by:
1. Reading `services/game_shard/shard.py` to understand current code structure
2. Finding the `GameContext`, `_handle_ws_price_update`, and `_generate_signals` methods
3. Creating a detailed implementation plan
4. Asking clarifying questions if needed before implementation

**Important:** This is PLANNING MODE. Create a comprehensive plan first, then we'll move to implementation. Make sure you fully understand the problem before proposing solutions.

What's your analysis and implementation plan?
