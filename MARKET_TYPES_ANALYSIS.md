# Market Type Analysis: Why Moneyline, Spread, and Totals Matter

## Current Problem

Your current implementation only looks at **generic "team to win" markets**, which are:
- ❌ Too broad (doesn't match specific bet types)
- ❌ Hard to compare across platforms (different market structures)
- ❌ Missing the most liquid arbitrage opportunities
- ❌ Not accounting for different bet types (spread, totals, props)

## Solution: Specific Market Types

The `terauss/Polymarket-Kalshi-Arbitrage-bot` focuses on specific, comparable bet types:

### 1. **Moneyline** (Team to Win)
- **Kalshi:** "Will [Team] win?"
- **Polymarket:** "[Team] to win"
- **Example:** "Will the Lakers beat the Celtics?"
- **Why it works:** Direct 1:1 comparison

### 2. **Spread** (Point Spread)
- **Kalshi:** "Will [Team] cover the spread of [X] points?"
- **Polymarket:** "[Team] to cover -[X]"
- **Example:** "Will the Chiefs cover -7.5?"
- **Why it works:** Same outcome, direct arbitrage

### 3. **Totals** (Over/Under)
- **Kalshi:** "Will the total score be over [X]?"
- **Polymarket:** "Total points over [X]"
- **Example:** "Will total score exceed 215.5 points?"
- **Why it works:** Same outcome, high liquidity

### 4. **Player Props** (Advanced)
- **Kalshi:** "Will [Player] score over [X] points?"
- **Polymarket:** "[Player] over [X] points"
- **Example:** "Will LeBron score over 25.5 points?"
- **Why it works:** Very specific, easy to match

---

## Current vs Better Approach

### Current Approach (Generic)
```python
# Looking for ANY market mentioning "Lakers"
markets = await kalshi.get_markets(series_ticker="KXNBA")
for market in markets:
    if "lakers" in market.title.lower():
        return market  # Too broad!
```

**Problems:**
- Might match "Lakers to make playoffs" (wrong bet type)
- Might match "Lakers to win championship" (wrong timeframe)
- Might match "Lakers over 50.5 wins" (wrong type)
- Can't compare to Polymarket's specific spreads/totals

### Better Approach (Specific)
```python
# Looking for SPECIFIC bet types
bet_types = {
    "moneyline": {
        "kalshi_pattern": "Will {team} win",
        "polymarket_pattern": "{team} to win"
    },
    "spread": {
        "kalshi_pattern": "Will {team} cover {spread}",
        "polymarket_pattern": "{team} {spread}"
    },
    "total": {
        "kalshi_pattern": "Will total score be over {points}",
        "polymarket_pattern": "Total points over {points}"
    }
}
```

**Benefits:**
- Exact matching across platforms
- Can verify spreads/totals match (e.g., both -7.5)
- Higher liquidity markets
- More arbitrage opportunities

---

## Real-World Example

### Game: Lakers vs Celtics (Jan 20, 2026)

#### ❌ Generic Approach (Current)
```
Kalshi: "Will Lakers win?" (0.52/0.54)
Polymarket: "Lakers to make playoffs?" (0.85/0.87)
→ NOT COMPARABLE! Different markets!
```

#### ✅ Specific Approach (Better)
```
Bet Type: Moneyline
  Kalshi: "Will Lakers win vs Celtics on 1/20?" (0.52/0.54)
  Polymarket: "Lakers to win vs Celtics 1/20" (0.48/0.50)
  → ARBITRAGE: Buy Poly YES 0.50 + Buy Kalshi NO 0.48 = 0.98 < 1.00

Bet Type: Spread (Lakers -5.5)
  Kalshi: "Will Lakers cover -5.5 vs Celtics?" (0.45/0.47)
  Polymarket: "Lakers -5.5" (0.46/0.48)
  → No arbitrage (spread too tight)

Bet Type: Total (Over 220.5)
  Kalshi: "Will total score exceed 220.5?" (0.51/0.53)
  Polymarket: "Over 220.5 points" (0.49/0.51)
  → ARBITRAGE: Buy Poly YES 0.51 + Buy Kalshi NO 0.49 = 1.00 (breakeven)
```

---

## What terauss Bot Does Better

Looking at the terauss arbitrage bot architecture:

### 1. Market Type Classification
```python
class MarketType(Enum):
    MONEYLINE = "moneyline"
    SPREAD = "spread"
    TOTAL = "total"
    PLAYER_PROP = "player_prop"
    FIRST_BASKET = "first_basket"
    # etc...
```

### 2. Market Matching Logic
```python
def match_markets(kalshi_market, poly_market):
    # Extract bet type
    kalshi_type = parse_market_type(kalshi_market.title)
    poly_type = parse_market_type(poly_market.question)
    
    if kalshi_type != poly_type:
        return None  # Different bet types, can't compare
    
    # For spreads/totals, verify the line matches
    if kalshi_type == MarketType.SPREAD:
        if kalshi_spread != poly_spread:
            return None  # Different spreads, not arbitrage
    
    # Match found!
    return MarketPair(kalshi_market, poly_market, kalshi_type)
```

### 3. Why This Finds More Arbitrage
- **More markets:** 3-5 markets per game (moneyline + spread + total)
- **Higher liquidity:** Spread/total markets have more volume
- **Clearer matching:** No ambiguity about which markets to compare
- **Better odds:** Spreads often have better pricing than moneylines

---

## Recommended Changes

### Priority 1: Add Market Type Enum
```python
# In arbees_shared/models/market.py
class MarketType(str, Enum):
    MONEYLINE = "moneyline"
    SPREAD = "spread"
    TOTAL = "total"
    PLAYER_PROP = "player_prop"
    FIRST_BASKET = "first_basket"
    FIRST_TD = "first_touchdown"
```

### Priority 2: Update Market Discovery
Add parsing logic to extract:
- Bet type (moneyline, spread, total)
- Line/spread (e.g., -7.5, 220.5)
- Team/player involved
- Game date

### Priority 3: Match Only Compatible Markets
```python
def can_arbitrage(market_a, market_b):
    # Must be same bet type
    if market_a.market_type != market_b.market_type:
        return False
    
    # For spreads/totals, lines must match
    if market_a.market_type in (MarketType.SPREAD, MarketType.TOTAL):
        if market_a.line != market_b.line:
            return False
    
    return True
```

### Priority 4: Multi-Market Monitoring
```python
# Instead of one market per game
game_markets = {
    "moneyline": {"kalshi": "MARKET-1", "poly": "COND-1"},
    "spread": {"kalshi": "MARKET-2", "poly": "COND-2"},
    "total": {"kalshi": "MARKET-3", "poly": "COND-3"},
}
```

---

## Expected Improvement

### Before (Generic Markets Only)
```
Games monitored: 5
Markets per game: 1 (just "team to win")
Total markets: 5
Arbitrage opportunities found: 0-1 per day
```

### After (Specific Market Types)
```
Games monitored: 5
Markets per game: 3-5 (moneyline + spread + total + props)
Total markets: 15-25
Arbitrage opportunities found: 3-8 per day
```

**3-8x more opportunities!**

---

## Next Steps

1. **Add market type parsing** to `MarketDiscoveryService`
2. **Update arbitrage detection** to only compare compatible types
3. **Monitor multiple markets per game** (moneyline + spread + total)
4. **Add line/spread matching** for totals and spreads
5. **Test with real markets** to verify parsing works

Should I implement these changes now?
