# Claude Code Prompt: Fix Home/Away Team Price Matching Bug

## Problem Statement

There is a critical bug in the signal generation logic where we're comparing probabilities from different teams, leading to incorrect edge calculations and wrong trade execution.

### Current Broken Behavior

**The Issue:**
```python
# Signal generation currently does:
prob_change = new_prob - old_prob  # new_prob is ALWAYS home team

if prob_change > 0:
    signal.team = home_team  # ✅ Correct
    edge = model_prob - market_prob  # ❌ BUT which market_prob?

elif prob_change < 0:
    signal.team = away_team  # ✅ Correct  
    edge = model_prob - market_prob  # ❌ BUT which market_prob?
```

**The Core Problem:**
- `model_prob` (from win probability calculation) is **always the HOME team's win probability**
- `market_prob` comes from `ctx.market_prices` which could be **either home OR away team's contract**
- When we calculate `edge = model_prob - market_prob`, we might be comparing:
  - HOME team model prob (65%) to AWAY team market price (48%) ❌ **WRONG - comparing different teams!**
  - This produces meaningless edge calculations

**Example of the Bug:**
```
Game: Lakers (home) @ Celtics (away)
Model: Lakers 65% to win (home team)
Markets available:
  - Lakers YES: $0.52 (52% implied)
  - Celtics YES: $0.48 (48% implied)

Current broken logic when Lakers probability increases:
  - prob_change = +5%
  - action = "BUY"
  - team = "Lakers" ✅
  - edge = 0.65 - market_prob

BUT: Which market_prob gets used?
  - If Lakers market: 0.65 - 0.52 = 13% edge ✅ CORRECT
  - If Celtics market: 0.65 - 0.48 = 17% edge ❌ WRONG! (comparing Lakers prob to Celtics price!)
```

---

## Required Fix

### Goal
Ensure that when calculating edge, both `model_prob` and `market_prob` refer to **the same team**.

### Solution Strategy

**Approach 1: Track Team-Specific Prices (RECOMMENDED)**

Modify the system to explicitly track which team each market price represents:

```python
@dataclass
class GameContext:
    """Enhanced to track team-specific prices."""
    game_state: GameState
    market_prices: dict[Platform, MarketPrice]  # Keep for compatibility
    
    # NEW: Organize prices by team
    home_team_prices: dict[Platform, float]  # HOME team YES prices
    away_team_prices: dict[Platform, float]  # AWAY team YES prices
```

**Approach 2: Convert Prices to Same Perspective**

When comparing, always convert to the same team's perspective:
- If comparing HOME team: use home market price OR (1.0 - away market price)
- If comparing AWAY team: use away market price OR (1.0 - home market price)

---

## Files to Modify

### 1. `services/game_shard/shard.py`

**Location of Bug:**
- `GameContext` dataclass (add team-specific price tracking)
- `_handle_ws_price_update()` method (populate team-specific prices)
- `_generate_signals()` method (use correct team prices for edge calculation)

**Required Changes:**

#### Change 1: Update GameContext
```python
@dataclass
class GameContext:
    game_state: GameState
    market_prices: dict[Platform, MarketPrice]
    
    # ADD THESE:
    home_team_prices: dict[Platform, float] = field(default_factory=dict)
    away_team_prices: dict[Platform, float] = field(default_factory=dict)
```

#### Change 2: Populate Team-Specific Prices
```python
async def _handle_ws_price_update(self, platform: Platform, price: MarketPrice):
    """Handle price update and categorize by team."""
    
    # ... existing code to find game_id and ctx ...
    
    # Store raw price (existing)
    ctx.market_prices[platform] = price
    
    # NEW: Determine which team this price is for
    if self._is_home_team_market(price.market_id, ctx.game_state.home_team):
        ctx.home_team_prices[platform] = price.mid_price
        logger.debug(f"[{ctx.game_state.game_id}] HOME team price: {price.mid_price:.3f}")
    elif self._is_away_team_market(price.market_id, ctx.game_state.away_team):
        ctx.away_team_prices[platform] = price.mid_price
        logger.debug(f"[{ctx.game_state.game_id}] AWAY team price: {price.mid_price:.3f}")
    else:
        logger.warning(f"Cannot determine team for market {price.market_id}")
```

#### Change 3: Fix Edge Calculation in Signal Generation
```python
async def _generate_signals(self, ctx, old_state, new_state, old_prob, new_prob, new_plays):
    """Generate signals with CORRECT team-specific price matching."""
    
    prob_change = new_prob - old_prob
    
    # Only signal on significant changes
    if abs(prob_change) < 0.02:
        return
    
    # Determine which team to bet on and get CORRECT market price
    if prob_change > 0:
        # HOME team probability increased → BUY HOME team
        action = "BUY"
        team = new_state.home_team
        model_prob_for_team = new_prob  # Already home team prob
        
        # Get HOME team market price
        market_prob = self._get_best_price(ctx.home_team_prices)
        if market_prob is None:
            logger.warning(f"No market price for HOME team {team}")
            return
        
        edge = model_prob_for_team - market_prob  # ✅ Both HOME team
        
    else:  # prob_change < 0
        # HOME team probability decreased → BUY AWAY team
        action = "BUY"
        team = new_state.away_team
        model_prob_for_team = 1.0 - new_prob  # Convert to away team prob
        
        # Get AWAY team market price
        market_prob = self._get_best_price(ctx.away_team_prices)
        if market_prob is None:
            logger.warning(f"No market price for AWAY team {team}")
            return
        
        edge = model_prob_for_team - market_prob  # ✅ Both AWAY team
    
    # Check minimum edge threshold
    if abs(edge) < 0.01:  # 1% minimum edge
        return
    
    # ... rest of signal generation ...
    signal = TradingSignal(
        game_id=new_state.game_id,
        team=team,
        action=action,
        edge=edge,
        model_prob=model_prob_for_team,  # Probability for the team we're betting
        market_prob=market_prob,
        confidence=self._calculate_confidence(edge, new_state),
        size=self._calculate_position_size(edge),
        # ...
    )
```

#### Change 4: Add Helper Methods

**Add these new methods to GameShard class:**

```python
def _is_home_team_market(self, market_id: str, home_team: str) -> bool:
    """
    Determine if market_id represents the home team's contract.
    
    Handles:
    - Kalshi: Parse ticker for team codes
    - Polymarket: Use cached team mapping
    """
    # Kalshi markets: KXNBA-LAL-BOS-20260122
    if market_id.startswith("KX"):
        parts = market_id.split("-")
        if len(parts) >= 3:
            team1_code = parts[1]
            home_code = self._get_team_code(home_team)
            return team1_code == home_code
    
    # Polymarket: Check cached mapping
    # (Assumes you store team info when discovering markets)
    market_info = self._market_metadata.get(market_id)
    if market_info:
        return market_info.get("team") == home_team
    
    logger.warning(f"Cannot determine if {market_id} is home team market")
    return False

def _is_away_team_market(self, market_id: str, away_team: str) -> bool:
    """
    Determine if market_id represents the away team's contract.
    """
    if market_id.startswith("KX"):
        parts = market_id.split("-")
        if len(parts) >= 3:
            team2_code = parts[2]
            away_code = self._get_team_code(away_team)
            return team2_code == away_code
    
    market_info = self._market_metadata.get(market_id)
    if market_info:
        return market_info.get("team") == away_team
    
    logger.warning(f"Cannot determine if {market_id} is away team market")
    return False

def _get_team_code(self, team_name: str) -> Optional[str]:
    """
    Convert team name to market code.
    
    Examples:
    - "Los Angeles Lakers" → "LAL"
    - "Boston Celtics" → "BOS"
    """
    # Simple mapping (you may have this elsewhere)
    team_codes = {
        "Los Angeles Lakers": "LAL",
        "Boston Celtics": "BOS",
        "Golden State Warriors": "GSW",
        # ... add more as needed
    }
    return team_codes.get(team_name)

def _get_best_price(self, team_prices: dict[Platform, float]) -> Optional[float]:
    """
    Get the best available price from available platforms.
    
    Priority: Kalshi > Polymarket (or whatever your preference)
    """
    # Prefer Kalshi if available
    if Platform.KALSHI in team_prices:
        return team_prices[Platform.KALSHI]
    
    # Fallback to Polymarket
    if Platform.POLYMARKET in team_prices:
        return team_prices[Platform.POLYMARKET]
    
    # Return first available
    if team_prices:
        return next(iter(team_prices.values()))
    
    return None
```

---

### 2. `services/paper_trader/trader.py`

**Location of Bug:**
The paper trader needs to execute trades on the **correct team's contract**.

**Required Changes:**

```python
async def execute_signal(self, signal: TradingSignal):
    """Execute paper trade on the CORRECT team's contract."""
    
    logger.info(
        f"📝 Paper trade: {signal.action} {signal.team} "
        f"(edge: {signal.edge:.2%}, size: ${signal.size:.2f})"
    )
    
    # Find the market for the SPECIFIC TEAM we're betting on
    market = await self._get_team_market(
        game_id=signal.game_id,
        team=signal.team,  # Critical: use signal.team, not just any market
        platform=signal.platform
    )
    
    if not market:
        logger.error(f"❌ No market found for team: {signal.team}")
        return
    
    # Execute based on action
    if signal.action == "BUY":
        # Buy YES on this team
        position = await self._buy_yes(
            market=market,
            team=signal.team,  # Make sure we track which team
            size=signal.size,
            price=signal.market_prob
        )
    
    elif signal.action == "SELL":
        # Sell means we're betting AGAINST this team
        # Either: (1) Buy their NO contract, or (2) Buy opponent's YES
        # For simplicity, buy opponent's YES
        position = await self._buy_yes(
            market=market,
            team=signal.team,
            size=signal.size,
            price=signal.market_prob
        )
    
    # Record position
    self.positions[signal.game_id].append(position)

async def _get_team_market(
    self,
    game_id: str,
    team: str,
    platform: Platform
) -> Optional[Market]:
    """
    Get the market for a SPECIFIC team.
    
    Critical: Must return the contract for the team we want to bet on,
    not just any market for the game.
    """
    # Use discovery service to find team-specific market
    # This might require enhancing the discovery service
    
    # For now, you might need to check market metadata
    all_markets = await self.discovery.find_markets_for_game(game_id, [platform])
    
    for market in all_markets:
        # Check if this market is for our team
        if self._is_team_market(market, team):
            return market
    
    return None

def _is_team_market(self, market: Market, team: str) -> bool:
    """Check if market is for the specified team."""
    # Similar logic to _is_home_team_market
    # Check market title, metadata, etc.
    
    # Example for Kalshi:
    if "market_ticker" in market:
        ticker = market["market_ticker"]
        team_code = self._get_team_code(team)
        return team_code in ticker
    
    # Example for Polymarket:
    if "question" in market:
        return team in market["question"]
    
    return False
```

---

### 3. `arbees_shared/models/game.py` (If needed)

**Add team metadata to MarketPrice if not already present:**

```python
@dataclass
class MarketPrice:
    market_id: str
    platform: Platform
    yes_bid: float
    yes_ask: float
    no_bid: float
    no_ask: float
    mid_price: float
    timestamp: datetime
    
    # ADD THIS if not present:
    team: Optional[str] = None  # Which team this contract is for
```

---

## Testing Strategy

### Test Case 1: Home Team Probability Increases

```python
# Setup
game = "Lakers (home) vs Celtics (away)"
old_prob = 0.60  # Lakers 60%
new_prob = 0.65  # Lakers 65% (increased)

# Market prices
lakers_market_price = 0.52  # 52%
celtics_market_price = 0.48  # 48%

# Expected behavior:
# - prob_change = +0.05 (positive)
# - action = BUY
# - team = "Lakers"
# - model_prob = 0.65
# - market_prob = 0.52 (Lakers market, NOT Celtics!)
# - edge = 0.65 - 0.52 = 0.13 (13%)

# Execute signal generation
signal = _generate_signals(...)

# Verify
assert signal.team == "Lakers"
assert signal.action == "BUY"
assert signal.model_prob == 0.65
assert signal.market_prob == 0.52  # Must be Lakers price!
assert abs(signal.edge - 0.13) < 0.001
```

### Test Case 2: Home Team Probability Decreases

```python
# Setup
game = "Lakers (home) vs Celtics (away)"
old_prob = 0.60  # Lakers 60%
new_prob = 0.55  # Lakers 55% (decreased)

# Market prices
lakers_market_price = 0.52  # 52%
celtics_market_price = 0.48  # 48%

# Expected behavior:
# - prob_change = -0.05 (negative)
# - action = "BUY"
# - team = "Celtics"
# - model_prob = 0.45 (1.0 - 0.55, converted to Celtics prob)
# - market_prob = 0.48 (Celtics market, NOT Lakers!)
# - edge = 0.45 - 0.48 = -0.03 (-3%, no trade due to negative edge)

# Execute
signal = _generate_signals(...)

# Verify
assert signal is None or signal.team == "Celtics"
if signal:
    assert signal.model_prob == 0.45  # Celtics prob
    assert signal.market_prob == 0.48  # Celtics price!
```

### Test Case 3: Paper Trader Execution

```python
# Signal: BUY Lakers
signal = TradingSignal(
    team="Lakers",
    action="BUY",
    edge=0.13,
    market_prob=0.52,
    platform=Platform.KALSHI,
    # ...
)

# Execute
await paper_trader.execute_signal(signal)

# Verify
# - Paper trader should find Lakers YES market
# - NOT Celtics YES market
# - Should buy at Lakers price (0.52), not Celtics price (0.48)
position = paper_trader.positions[game_id][0]
assert position.team == "Lakers"
assert position.entry_price == 0.52  # Lakers price, not 0.48!
```

---

## Verification Checklist

After implementing the fix, verify:

- [ ] `GameContext` has `home_team_prices` and `away_team_prices` fields
- [ ] `_handle_ws_price_update()` correctly populates team-specific prices
- [ ] `_is_home_team_market()` and `_is_away_team_market()` correctly identify team
- [ ] `_generate_signals()` uses correct team price for edge calculation
- [ ] When `prob_change > 0`: uses home team price
- [ ] When `prob_change < 0`: uses away team price (and converts model prob)
- [ ] Paper trader executes on correct team's contract
- [ ] Logging shows which team's price is being used
- [ ] Edge calculations make sense (not comparing apples to oranges)

---

## Success Criteria

**Before Fix (Broken):**
```
[SIGNAL] BUY Lakers, edge: 17%
  model_prob: 0.65 (Lakers)
  market_prob: 0.48 (??? - could be Celtics!)
  ❌ Nonsensical edge calculation
```

**After Fix (Correct):**
```
[SIGNAL] BUY Lakers, edge: 13%
  model_prob: 0.65 (Lakers)
  market_prob: 0.52 (Lakers market - verified!)
  ✅ Meaningful edge calculation
```

---

## Expected Behavior Changes

After this fix, you should see:
1. **More accurate edge calculations** - comparing same team's model vs market
2. **Fewer false signals** - won't trigger on mismatched comparisons
3. **Correct trade execution** - paper trader buys the right team's contract
4. **Better win rate** - edges are actually meaningful now

---

## Important Notes

1. **Market Discovery**: Ensure your market discovery service stores which team each market represents
2. **Team Code Mapping**: You'll need a reliable way to convert team names to market codes (LAL, BOS, etc.)
3. **Logging**: Add extensive logging to verify correct team prices are being used
4. **Backward Compatibility**: Keep `ctx.market_prices` for any code that still uses it

---

## Implementation Plan

1. **Phase 1: Add team tracking to GameContext**
   - Update dataclass
   - Modify initialization

2. **Phase 2: Populate team-specific prices**
   - Update `_handle_ws_price_update()`
   - Add helper methods (`_is_home_team_market`, etc.)

3. **Phase 3: Fix signal generation**
   - Update `_generate_signals()` to use correct prices
   - Add extensive logging

4. **Phase 4: Fix paper trader**
   - Update `execute_signal()` to use correct market
   - Add `_get_team_market()` method

5. **Phase 5: Testing**
   - Write unit tests for edge cases
   - Test with live games
   - Verify edges make sense

6. **Phase 6: Monitoring**
   - Watch logs for a few games
   - Verify team prices are correct
   - Check paper trader execution

---

## Sample Log Output (After Fix)

```
[GameShard] [Lakers @ Celtics] Game state updated
[GameShard] [Lakers @ Celtics] HOME team price: 0.520 (Lakers)
[GameShard] [Lakers @ Celtics] AWAY team price: 0.480 (Celtics)
[GameShard] [Lakers @ Celtics] Model: Lakers 65%, was 60% (+5%)
[GameShard] [Lakers @ Celtics] Using HOME team price for comparison
[GameShard] [Lakers @ Celtics] Edge: 65% - 52% = 13%
[SIGNAL] BUY Lakers
  Team: Lakers (HOME)
  Model Prob: 0.650
  Market Prob: 0.520 (Lakers YES on Kalshi)
  Edge: 0.130 (13.0%)
  Size: $100.00

[PaperTrader] Executing: BUY Lakers @ 0.52
[PaperTrader] Found market: KXNBA-LAL-BOS-20260122 (Lakers YES)
[PaperTrader] Position opened: Lakers @ 0.52
```

---

## Begin Implementation

Start by:
1. Reading the current `services/game_shard/shard.py` code
2. Locating the `GameContext`, `_handle_ws_price_update`, and `_generate_signals` methods
3. Implementing the changes outlined above
4. Adding logging to verify correct behavior
5. Testing with a live game

This fix is critical for accurate trading - the current behavior is essentially random since we're comparing mismatched probabilities!
