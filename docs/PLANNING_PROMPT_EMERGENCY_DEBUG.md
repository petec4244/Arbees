# EMERGENCY DEBUG PLAN - Fix Paper Trading Bugs This Weekend

**Goal:** Stop losing money on bad trades by fixing team matching and exit logic  
**Timeline:** This weekend (Saturday + Sunday)  
**Expected Time:** 12-16 hours  
**Success Criteria:** 24-hour paper trading run with positive P&L and no immediate exits

---

## Executive Summary

**The Problem:**
Your architecture is excellent, but you have **logic bugs** causing:
- Immediate position exits (entry price = exit price)
- Wrong entry prices (90%+ probabilities)
- Repeated losses on same teams

**Root Causes:**
1. **Team Matching Bug** - SignalProcessor can select wrong team's price
2. **Exit Validation Bug** - PositionTracker compares prices from different teams
3. **Missing Logging** - Can't see what's happening in production

**The Fix:**
Add comprehensive logging + fix team matching + validate exits = working system

---

## Phase 1: Pre-Flight Checks (30 minutes)

### Step 1.1: Stop All Trading Services

**Why:** Prevent further losses while debugging

```bash
# Stop the trading pipeline
docker-compose stop signal_processor execution_service position_tracker

# Verify they're stopped
docker-compose ps | grep -E "signal|execution|position"
```

**Validation:**
```bash
# Should show no running containers for these services
# If they show "Up", run stop again
```

---

### Step 1.2: Backup Current Database State

**Why:** Safety net in case something goes wrong

```bash
# Create backup directory
mkdir -p ./backups/$(date +%Y%m%d_%H%M%S)

# Export paper trades table
docker exec arbees-timescaledb pg_dump \
  -U arbees -d arbees \
  -t paper_trades \
  --inserts \
  > ./backups/$(date +%Y%m%d_%H%M%S)/paper_trades_backup.sql

# Export market prices (last 24 hours only)
docker exec arbees-timescaledb pg_dump \
  -U arbees -d arbees \
  -t market_prices \
  --inserts \
  > ./backups/$(date +%Y%m%d_%H%M%S)/market_prices_backup.sql
```

**Validation:**
```bash
# Check backup files exist and are not empty
ls -lh ./backups/$(date +%Y%m%d_%H%M%S)/
# Should show two .sql files, each > 1KB
```

---

### Step 1.3: Review Recent Failed Trades

**Why:** Understand the exact failure patterns

```bash
# Connect to database
docker exec -it arbees-timescaledb psql -U arbees -d arbees

# Query recent losses with immediate exits
SELECT 
    trade_id,
    game_id,
    platform,
    market_id,
    market_title,
    side,
    entry_price,
    exit_price,
    size,
    pnl,
    time as entry_time,
    exit_time,
    EXTRACT(EPOCH FROM (exit_time - time)) as hold_seconds
FROM paper_trades
WHERE status = 'closed'
  AND outcome = 'loss'
  AND time > NOW() - INTERVAL '24 hours'
ORDER BY time DESC
LIMIT 20;
```

**What to look for:**
```
❌ hold_seconds < 30 (immediate exits)
❌ entry_price > 0.85 (bad entry prices)
❌ Same market_title appearing multiple times (repeated losses)
❌ exit_price = 1.0 - entry_price (inverted team prices)
```

**Save findings:**
```bash
# Export to file for analysis
\copy (SELECT trade_id, game_id, market_title, side, entry_price, exit_price, pnl, EXTRACT(EPOCH FROM (exit_time - time)) as hold_seconds FROM paper_trades WHERE status = 'closed' AND outcome = 'loss' AND time > NOW() - INTERVAL '24 hours' ORDER BY time DESC LIMIT 50) TO '/tmp/failed_trades.csv' WITH CSV HEADER;
\q

# Copy out of container
docker cp arbees-timescaledb:/tmp/failed_trades.csv ./failed_trades_analysis.csv
```

---

## Phase 2: Add Comprehensive Logging (3-4 hours)

### Step 2.1: Add Logging to SignalProcessor

**File:** `services/signal_processor/processor.py`

**Location:** In `_get_market_price()` method (around line 250)

**Add this logging block AFTER finding a price:**

```python
async def _get_market_price(self, signal: TradingSignal) -> Optional[MarketPrice]:
    """Get current market price for the signal."""
    pool = await get_pool()
    target_team = (signal.team or "").strip()

    # ... existing team matching logic ...

    # Find price logic here (existing code)
    # ...
    
    # ========== ADD THIS LOGGING BLOCK ==========
    if row:
        found_price = MarketPrice(
            market_id=row["market_id"],
            platform=Platform(row["platform"]),
            market_title=row["market_title"],
            contract_team=row.get("contract_team"),
            yes_bid=float(row["yes_bid"]),
            yes_ask=float(row["yes_ask"]),
            yes_bid_size=float(row.get("yes_bid_size") or 0),
            yes_ask_size=float(row.get("yes_ask_size") or 0),
            volume=float(row["volume"] or 0),
            liquidity=float(row.get("liquidity") or 0),
        )
        
        # CRITICAL LOGGING: Price selection decision
        logger.critical(
            "🔍 PRICE SELECTION DECISION:\n"
            f"  Signal ID: {signal.signal_id}\n"
            f"  Game ID: {signal.game_id}\n"
            f"  Target Team: '{target_team}'\n"
            f"  Signal Direction: {signal.direction.value}\n"
            f"  Signal Model Prob: {signal.model_prob:.3f}\n"
            f"  Signal Market Prob: {signal.market_prob:.3f if signal.market_prob else 'None'}\n"
            f"  ----\n"
            f"  Found Price:\n"
            f"    Platform: {found_price.platform.value}\n"
            f"    Market ID: {found_price.market_id}\n"
            f"    Market Title: '{found_price.market_title}'\n"
            f"    Contract Team: '{found_price.contract_team}'\n"
            f"    Yes Bid: {found_price.yes_bid:.3f}\n"
            f"    Yes Ask: {found_price.yes_ask:.3f}\n"
            f"    Mid: {found_price.mid_price:.3f}\n"
            f"  ----\n"
            f"  Team Match Analysis:\n"
            f"    Exact Match: {target_team.lower() == (found_price.contract_team or '').lower()}\n"
            f"    Contains Target: {target_team.lower() in (found_price.contract_team or '').lower()}\n"
            f"    Contains Found: {(found_price.contract_team or '').lower() in target_team.lower()}\n"
            f"    DECISION: {'✅ APPROVED' if found_price.contract_team else '⚠️ NO CONTRACT_TEAM'}"
        )
        
        return found_price
    # ========== END LOGGING BLOCK ==========

    return None
```

**Validation:**
```bash
# Test the logging works
docker-compose up signal_processor
# Should see PRICE SELECTION DECISION logs with emoji markers
# Press Ctrl+C to stop
```

---

### Step 2.2: Add Logging to ExecutionService

**File:** `services/execution_service/service.py`

**Location:** In `_handle_execution_request()` method, AFTER trade execution

**Add this logging block:**

```python
async def _handle_execution_request(self, data: dict) -> None:
    """Handle incoming execution request."""
    try:
        exec_request = ExecutionRequest(**data)
        
        # ... existing execution logic ...
        
        # After successful execution, add this:
        if trade and self.paper_engine:
            # ========== ADD THIS LOGGING BLOCK ==========
            logger.critical(
                "💰 TRADE EXECUTED:\n"
                f"  Request ID: {exec_request.request_id}\n"
                f"  Trade ID: {trade.trade_id}\n"
                f"  Game ID: {exec_request.game_id}\n"
                f"  ----\n"
                f"  Execution Details:\n"
                f"    Platform: {exec_request.platform.value}\n"
                f"    Market ID: {exec_request.market_id}\n"
                f"    Contract Team: '{exec_request.contract_team}'\n"
                f"    Market Title: '{trade.market_title}'\n"
                f"  ----\n"
                f"  Position Details:\n"
                f"    Side: {exec_request.side.value}\n"
                f"    Entry Price: {trade.entry_price:.3f}\n"
                f"    Size: ${trade.size:.2f}\n"
                f"    Contracts: {trade.size / trade.entry_price:.2f}\n"
                f"  ----\n"
                f"  Signal Context:\n"
                f"    Model Prob: {exec_request.model_prob:.3f}\n"
                f"    Market Prob: {exec_request.market_prob:.3f if exec_request.market_prob else 'None'}\n"
                f"    Edge: {exec_request.edge_pct:.1f}%\n"
                f"    Limit Price: {exec_request.limit_price:.3f}\n"
                f"  ----\n"
                f"  Bankroll: ${self.paper_engine._bankroll.current_balance:.2f}"
            )
            # ========== END LOGGING BLOCK ==========
```

**Validation:**
```bash
# Test execution service logging
docker-compose up execution_service
# Should see TRADE EXECUTED logs
# Press Ctrl+C to stop
```

---

### Step 2.3: Add Logging to PositionTracker

**File:** `services/position_tracker/tracker.py`

**Location:** In `_evaluate_exit()` method AND `_get_current_price()` method

**Add to _get_current_price():**

```python
async def _get_current_price(self, trade: PaperTrade) -> Optional[float]:
    """Get current executable market price for an open trade."""
    pool = await get_pool()

    # Extract team hint from trade
    team_hint: Optional[str] = None
    title = (trade.market_title or "").strip()
    if "[" in title and "]" in title:
        try:
            team_hint = title.rsplit("[", 1)[-1].split("]", 1)[0].strip()
        except Exception:
            team_hint = None
    
    # ========== ADD THIS LOGGING BLOCK ==========
    logger.debug(
        f"🔎 FETCHING EXIT PRICE:\n"
        f"  Trade ID: {trade.trade_id}\n"
        f"  Game ID: {trade.game_id}\n"
        f"  Entry Market Title: '{trade.market_title}'\n"
        f"  Entry Contract Team: '{getattr(trade, 'contract_team', 'N/A')}'\n"
        f"  Extracted Team Hint: '{team_hint}'\n"
        f"  Trade Side: {trade.side.value}\n"
        f"  Entry Price: {trade.entry_price:.3f}"
    )
    # ========== END LOGGING BLOCK ==========

    # Query for price matching the team
    row = None
    if team_hint:
        row = await pool.fetchrow(
            """
            SELECT yes_bid, yes_ask, market_title, contract_team, time
            FROM market_prices
            WHERE platform = $1 AND market_id = $2
              AND (market_title ILIKE $3 OR market_title ILIKE $4 OR contract_team ILIKE $5)
            ORDER BY time DESC
            LIMIT 1
            """,
            trade.platform.value,
            trade.market_id,
            f"%[{team_hint}]%",
            f"%{team_hint}%",
            f"%{team_hint}%",
        )

    if not row:
        # Fallback: any recent row
        row = await pool.fetchrow(
            """
            SELECT yes_bid, yes_ask, market_title, contract_team, time
            FROM market_prices
            WHERE platform = $1 AND market_id = $2
            ORDER BY time DESC
            LIMIT 1
            """,
            trade.platform.value,
            trade.market_id,
        )
    
    if not row:
        logger.warning(f"❌ No current price found for trade {trade.trade_id}")
        return None

    bid = float(row["yes_bid"])
    ask = float(row["yes_ask"])
    mid = (bid + ask) / 2.0

    # Executable price (bid for BUY, ask for SELL)
    chosen = bid if trade.side == TradeSide.BUY else ask
    chosen_kind = "yes_bid" if trade.side == TradeSide.BUY else "yes_ask"

    # ========== ADD THIS LOGGING BLOCK ==========
    found_team = row.get("contract_team") or "N/A"
    found_title = row["market_title"]
    teams_match = (
        team_hint and found_team and 
        (team_hint.lower() in found_team.lower() or found_team.lower() in team_hint.lower())
    )
    
    logger.critical(
        f"📊 EXIT PRICE LOOKUP RESULT:\n"
        f"  Trade ID: {trade.trade_id}\n"
        f"  Entry Team Hint: '{team_hint}'\n"
        f"  Found Market Title: '{found_title}'\n"
        f"  Found Contract Team: '{found_team}'\n"
        f"  ----\n"
        f"  Price Data:\n"
        f"    Yes Bid: {bid:.3f}\n"
        f"    Yes Ask: {ask:.3f}\n"
        f"    Mid: {mid:.3f}\n"
        f"    Trade Side: {trade.side.value}\n"
        f"    Chosen Exit ({chosen_kind}): {chosen:.3f}\n"
        f"  ----\n"
        f"  Team Match Validation:\n"
        f"    Teams Match: {'✅ YES' if teams_match else '❌ NO'}\n"
        f"    Confidence: {'HIGH' if teams_match else 'LOW'}\n"
        f"    WARNING: {'⚠️ DIFFERENT TEAMS!' if not teams_match and team_hint and found_team else 'OK'}"
    )
    # ========== END LOGGING BLOCK ==========

    return chosen
```

**Add to _evaluate_exit():**

```python
def _evaluate_exit(
    self,
    trade: PaperTrade,
    current_price: float,
    sport: str
) -> tuple[bool, str]:
    """Evaluate if position should be exited."""
    entry_price = trade.entry_price
    stop_loss_pct = self._get_stop_loss_for_sport(sport)

    if trade.side == TradeSide.BUY:
        price_move = current_price - entry_price
        tp_trigger = price_move >= self.take_profit_pct / 100
        sl_trigger = price_move <= -stop_loss_pct / 100
    else:
        price_move = entry_price - current_price
        tp_trigger = price_move >= self.take_profit_pct / 100
        sl_trigger = price_move <= -stop_loss_pct / 100

    # ========== ADD THIS LOGGING BLOCK ==========
    logger.info(
        f"🎯 EXIT EVALUATION:\n"
        f"  Trade ID: {trade.trade_id}\n"
        f"  Side: {trade.side.value}\n"
        f"  Entry: {entry_price:.3f}\n"
        f"  Current: {current_price:.3f}\n"
        f"  Price Move: {price_move:+.3f} ({price_move*100:+.1f}%)\n"
        f"  ----\n"
        f"  Thresholds:\n"
        f"    Take Profit: +{self.take_profit_pct:.1f}%\n"
        f"    Stop Loss: -{stop_loss_pct:.1f}%\n"
        f"  ----\n"
        f"  Triggers:\n"
        f"    TP Triggered: {'✅ YES' if tp_trigger else '❌ NO'}\n"
        f"    SL Triggered: {'✅ YES' if sl_trigger else '❌ NO'}\n"
        f"  Decision: {'🚪 EXIT' if (tp_trigger or sl_trigger) else '✋ HOLD'}"
    )
    # ========== END LOGGING BLOCK ==========

    if tp_trigger:
        return True, f"take_profit: +{price_move*100:.1f}%"
    if sl_trigger:
        return True, f"stop_loss: {price_move*100:.1f}% (limit={stop_loss_pct}%)"

    return False, ""
```

**Validation:**
```bash
# Test position tracker logging
docker-compose up position_tracker
# Should see EXIT PRICE LOOKUP and EXIT EVALUATION logs
# Press Ctrl+C to stop
```

---

## Phase 3: Fix Team Matching Logic (4-5 hours)

### Step 3.1: Understand Current Team Matching

**File:** `services/signal_processor/processor.py`

**Current logic (lines ~250-350):**

```python
async def _get_market_price(self, signal: TradingSignal):
    # Tries multiple strategies:
    # 1. Match by contract_team using team_candidates()
    # 2. Fallback to ANY price if no team match found
```

**Problem:** Fallback can select wrong team's price!

---

### Step 3.2: Add Team Matching Validator

**Create new file:** `services/signal_processor/team_validator.py`

```python
"""
Team name matching and validation logic.
"""
from typing import Optional, Tuple
import re


class TeamMatchResult:
    """Result of team matching validation."""
    
    def __init__(
        self,
        is_match: bool,
        confidence: float,
        method: str,
        reason: str,
    ):
        self.is_match = is_match
        self.confidence = confidence  # 0.0 to 1.0
        self.method = method
        self.reason = reason
    
    def __repr__(self):
        return (
            f"TeamMatchResult(match={self.is_match}, "
            f"confidence={self.confidence:.2f}, "
            f"method='{self.method}')"
        )


class TeamValidator:
    """Validates team name matching across different formats."""
    
    # Common team name abbreviations
    ABBREVIATIONS = {
        # NBA
        "celtics": "BOS", "boston": "BOS",
        "lakers": "LAL", "los angeles lakers": "LAL",
        "warriors": "GSW", "golden state": "GSW",
        "heat": "MIA", "miami": "MIA",
        "knicks": "NYK", "new york knicks": "NYK",
        # NHL
        "bruins": "BOS", "boston bruins": "BOS",
        "flyers": "PHI", "philadelphia": "PHI",
        "capitals": "WSH", "washington": "WSH",
        "penguins": "PIT", "pittsburgh": "PIT",
        # Add more as needed
    }
    
    @staticmethod
    def normalize(team: str) -> str:
        """Normalize team name for comparison."""
        if not team:
            return ""
        # Remove special chars, lowercase, strip
        normalized = re.sub(r'[^\w\s]', '', team.lower()).strip()
        # Remove common words
        for word in ['the', 'of', 'at']:
            normalized = normalized.replace(f' {word} ', ' ')
        return normalized
    
    @staticmethod
    def extract_nickname(team: str) -> str:
        """Extract team nickname (last word)."""
        if not team:
            return ""
        parts = TeamValidator.normalize(team).split()
        return parts[-1] if parts else ""
    
    def validate_match(
        self,
        target_team: str,
        contract_team: str,
    ) -> TeamMatchResult:
        """
        Validate if contract_team matches target_team.
        
        Returns TeamMatchResult with confidence score.
        """
        if not target_team or not contract_team:
            return TeamMatchResult(
                is_match=False,
                confidence=0.0,
                method="empty_input",
                reason="Target or contract team is empty"
            )
        
        target_norm = self.normalize(target_team)
        contract_norm = self.normalize(contract_team)
        
        # Method 1: Exact match (100% confidence)
        if target_norm == contract_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=1.0,
                method="exact_match",
                reason=f"Exact: '{target_team}' == '{contract_team}'"
            )
        
        # Method 2: Nickname match (90% confidence)
        target_nick = self.extract_nickname(target_team)
        contract_nick = self.extract_nickname(contract_team)
        if target_nick and contract_nick and target_nick == contract_nick:
            return TeamMatchResult(
                is_match=True,
                confidence=0.9,
                method="nickname_match",
                reason=f"Nickname: '{target_nick}' == '{contract_nick}'"
            )
        
        # Method 3: One contains the other (80% confidence)
        if target_norm in contract_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=0.8,
                method="contains_target",
                reason=f"'{target_norm}' in '{contract_norm}'"
            )
        if contract_norm in target_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=0.8,
                method="contains_contract",
                reason=f"'{contract_norm}' in '{target_norm}'"
            )
        
        # Method 4: Abbreviation match (70% confidence)
        target_abbr = self.ABBREVIATIONS.get(target_norm)
        contract_abbr = self.ABBREVIATIONS.get(contract_norm)
        if target_abbr and contract_abbr and target_abbr == contract_abbr:
            return TeamMatchResult(
                is_match=True,
                confidence=0.7,
                method="abbreviation_match",
                reason=f"Abbr: {target_abbr} == {contract_abbr}"
            )
        
        # No match
        return TeamMatchResult(
            is_match=False,
            confidence=0.0,
            method="no_match",
            reason=f"No match: '{target_team}' vs '{contract_team}'"
        )
```

**Validation:**
```bash
# Create test file
cat > test_team_validator.py << 'EOF'
import sys
sys.path.insert(0, 'services/signal_processor')
from team_validator import TeamValidator

validator = TeamValidator()

# Test cases
tests = [
    ("Boston Celtics", "Celtics"),
    ("Philadelphia Flyers", "Flyers"),
    ("Flyers", "Philadelphia Flyers"),
    ("Los Angeles Lakers", "Lakers"),
    ("Celtics", "Flyers"),  # Should NOT match
]

for target, contract in tests:
    result = validator.validate_match(target, contract)
    print(f"{target:25s} vs {contract:25s} -> {result}")
EOF

python3 test_team_validator.py
# Should show matches with confidence scores
```

---

### Step 3.3: Integrate TeamValidator into SignalProcessor

**File:** `services/signal_processor/processor.py`

**At top of file, add import:**

```python
from .team_validator import TeamValidator, TeamMatchResult
```

**In __init__ method:**

```python
def __init__(self, ...):
    # ... existing code ...
    
    # Add team validator
    self.team_validator = TeamValidator()
```

**Replace _get_market_price() method with this improved version:**

```python
async def _get_market_price(self, signal: TradingSignal) -> Optional[MarketPrice]:
    """Get current market price for the signal with strict team validation."""
    pool = await get_pool()
    target_team = (signal.team or "").strip()
    
    if not target_team:
        logger.warning(f"Signal {signal.signal_id} has no team specified")
        return None
    
    # Strategy 1: Find price with matching contract_team
    rows = await pool.fetch(
        """
        SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
               yes_bid_size, yes_ask_size, volume, liquidity, time, platform
        FROM market_prices
        WHERE game_id = $1
          AND contract_team IS NOT NULL
          AND time > NOW() - INTERVAL '2 minutes'
        ORDER BY time DESC
        LIMIT 10
        """,
        signal.game_id,
    )
    
    # Validate each price and pick best match
    best_match = None
    best_confidence = 0.0
    best_result = None
    
    for row in rows:
        contract_team = row["contract_team"]
        match_result = self.team_validator.validate_match(target_team, contract_team)
        
        if match_result.is_match and match_result.confidence > best_confidence:
            best_confidence = match_result.confidence
            best_result = match_result
            best_match = row
    
    # Only accept matches with confidence >= 0.7
    if best_match and best_confidence >= 0.7:
        found_price = MarketPrice(
            market_id=best_match["market_id"],
            platform=Platform(best_match["platform"]),
            market_title=best_match["market_title"],
            contract_team=best_match["contract_team"],
            yes_bid=float(best_match["yes_bid"]),
            yes_ask=float(best_match["yes_ask"]),
            yes_bid_size=float(best_match.get("yes_bid_size") or 0),
            yes_ask_size=float(best_match.get("yes_ask_size") or 0),
            volume=float(best_match["volume"] or 0),
            liquidity=float(best_match.get("liquidity") or 0),
        )
        
        logger.critical(
            f"✅ TEAM MATCH VALIDATED:\n"
            f"  Signal Team: '{target_team}'\n"
            f"  Contract Team: '{found_price.contract_team}'\n"
            f"  Confidence: {best_confidence:.0%}\n"
            f"  Method: {best_result.method}\n"
            f"  Reason: {best_result.reason}\n"
            f"  Price: bid={found_price.yes_bid:.3f} ask={found_price.yes_ask:.3f}"
        )
        
        return found_price
    
    # No confident match found
    logger.warning(
        f"❌ NO CONFIDENT TEAM MATCH:\n"
        f"  Signal Team: '{target_team}'\n"
        f"  Game ID: {signal.game_id}\n"
        f"  Best Confidence: {best_confidence:.0%}\n"
        f"  Searched {len(rows)} prices\n"
        f"  REJECTING SIGNAL - will not trade with uncertain team match"
    )
    
    return None
```

**Validation:**
```bash
# Rebuild signal_processor container
docker-compose build signal_processor

# Test with logs
docker-compose up signal_processor
# Should see TEAM MATCH VALIDATED logs with confidence scores
```

---

## Phase 4: Fix Exit Monitoring Logic (2-3 hours)

### Step 4.1: Add Team Validation to Position Tracker

**File:** `services/position_tracker/tracker.py`

**At top of file, add imports:**

```python
# Add to existing imports
import sys
sys.path.insert(0, '/app/services/signal_processor')
from team_validator import TeamValidator, TeamMatchResult
```

**In __init__ method:**

```python
def __init__(self, ...):
    # ... existing code ...
    
    # Add team validator
    self.team_validator = TeamValidator()
```

---

### Step 4.2: Update _get_current_price() with Strict Validation

**File:** `services/position_tracker/tracker.py`

**Replace _get_current_price() method:**

```python
async def _get_current_price(self, trade: PaperTrade) -> Optional[float]:
    """
    Get current executable market price for an open trade.
    
    CRITICAL: Must return price for the SAME team as entry!
    """
    pool = await get_pool()

    # Extract team from trade
    # Priority: 1. contract_team attribute, 2. parse from market_title
    entry_team = getattr(trade, 'contract_team', None)
    
    if not entry_team:
        # Parse from market_title
        title = (trade.market_title or "").strip()
        if "[" in title and "]" in title:
            try:
                entry_team = title.rsplit("[", 1)[-1].split("]", 1)[0].strip()
            except Exception:
                pass
    
    if not entry_team:
        logger.error(
            f"❌ CANNOT DETERMINE ENTRY TEAM:\n"
            f"  Trade ID: {trade.trade_id}\n"
            f"  Market Title: '{trade.market_title}'\n"
            f"  No contract_team or parseable team in title\n"
            f"  CANNOT SAFELY EVALUATE EXIT - HOLDING POSITION"
        )
        return None
    
    # Get recent prices for this market
    rows = await pool.fetch(
        """
        SELECT yes_bid, yes_ask, market_title, contract_team, time
        FROM market_prices
        WHERE platform = $1 
          AND market_id = $2
          AND contract_team IS NOT NULL
          AND time > NOW() - INTERVAL '2 minutes'
        ORDER BY time DESC
        LIMIT 5
        """,
        trade.platform.value,
        trade.market_id,
    )
    
    # Validate team match for each price
    best_match = None
    best_confidence = 0.0
    best_result = None
    
    for row in rows:
        contract_team = row["contract_team"]
        match_result = self.team_validator.validate_match(entry_team, contract_team)
        
        if match_result.is_match and match_result.confidence > best_confidence:
            best_confidence = match_result.confidence
            best_result = match_result
            best_match = row
    
    # Require minimum 0.7 confidence
    if not best_match or best_confidence < 0.7:
        logger.warning(
            f"⚠️ NO CONFIDENT EXIT PRICE MATCH:\n"
            f"  Trade ID: {trade.trade_id}\n"
            f"  Entry Team: '{entry_team}'\n"
            f"  Best Confidence: {best_confidence:.0%}\n"
            f"  Searched {len(rows)} prices\n"
            f"  HOLDING POSITION - will not exit without confident team match"
        )
        return None
    
    # Extract prices
    bid = float(best_match["yes_bid"])
    ask = float(best_match["yes_ask"])
    
    # Executable price (bid for BUY, ask for SELL)
    chosen = bid if trade.side == TradeSide.BUY else ask
    chosen_kind = "yes_bid" if trade.side == TradeSide.BUY else "yes_ask"
    
    logger.info(
        f"✅ EXIT PRICE VALIDATED:\n"
        f"  Trade ID: {trade.trade_id}\n"
        f"  Entry Team: '{entry_team}'\n"
        f"  Exit Team: '{best_match['contract_team']}'\n"
        f"  Confidence: {best_confidence:.0%}\n"
        f"  Method: {best_result.method}\n"
        f"  Executable Price ({chosen_kind}): {chosen:.3f}"
    )
    
    return chosen
```

**Validation:**
```bash
# Rebuild position_tracker
docker-compose build position_tracker

# Test logs
docker-compose up position_tracker
# Should see EXIT PRICE VALIDATED logs
```

---

## Phase 5: Add Minimum Hold Time (30 minutes)

**Why:** Prevent exits within first 10 seconds (likely bad data)

**File:** `services/position_tracker/tracker.py`

**In __init__:**

```python
def __init__(
    self,
    # ... existing params ...
    min_hold_seconds: float = 10.0,  # Already exists
):
    # ... existing code ...
    self.min_hold_seconds = min_hold_seconds
```

**In _evaluate_exit():**

```python
def _evaluate_exit(
    self,
    trade: PaperTrade,
    current_price: float,
    sport: str
) -> tuple[bool, str]:
    """Evaluate if position should be exited."""
    
    # Check minimum hold time
    hold_time = (datetime.utcnow() - trade.time).total_seconds()
    if hold_time < self.min_hold_seconds:
        logger.debug(
            f"⏳ MINIMUM HOLD TIME NOT MET:\n"
            f"  Trade ID: {trade.trade_id}\n"
            f"  Hold Time: {hold_time:.1f}s\n"
            f"  Minimum: {self.min_hold_seconds:.1f}s\n"
            f"  Decision: HOLD (too soon to exit)"
        )
        return False, ""
    
    # ... rest of existing exit logic ...
```

---

## Phase 6: End-to-End Validation (4-6 hours)

### Step 6.1: Create Test Scenario

**Create file:** `test_validation.py`

```python
"""
End-to-end validation of bug fixes.

Tests:
1. Team matching works correctly
2. Exits only happen with team validation
3. No immediate exits
4. Positions held for minimum time
"""
import asyncio
import asyncpg
from datetime import datetime, timedelta

async def validate_fixes():
    """Run validation checks on recent trades."""
    
    # Connect to DB
    conn = await asyncpg.connect(
        host='localhost',
        port=5432,
        user='arbees',
        password='your_password',  # From .env
        database='arbees'
    )
    
    print("=" * 80)
    print("BUG FIX VALIDATION REPORT")
    print("=" * 80)
    print()
    
    # Test 1: Check for immediate exits
    print("Test 1: Checking for immediate exits...")
    immediate_exits = await conn.fetch("""
        SELECT 
            trade_id,
            game_id,
            market_title,
            side,
            entry_price,
            exit_price,
            pnl,
            EXTRACT(EPOCH FROM (exit_time - time)) as hold_seconds
        FROM paper_trades
        WHERE status = 'closed'
          AND time > NOW() - INTERVAL '1 hour'
          AND EXTRACT(EPOCH FROM (exit_time - time)) < 10
        ORDER BY time DESC
    """)
    
    if immediate_exits:
        print(f"  ❌ FAILED: Found {len(immediate_exits)} immediate exits (< 10s)")
        for trade in immediate_exits[:5]:
            print(f"     Trade {trade['trade_id']}: held {trade['hold_seconds']:.1f}s")
    else:
        print("  ✅ PASSED: No immediate exits found")
    print()
    
    # Test 2: Check entry prices
    print("Test 2: Checking for bad entry prices (>85%)...")
    bad_entries = await conn.fetch("""
        SELECT 
            trade_id,
            game_id,
            market_title,
            side,
            entry_price,
            size
        FROM paper_trades
        WHERE time > NOW() - INTERVAL '1 hour'
          AND entry_price > 0.85
        ORDER BY entry_price DESC
    """)
    
    if bad_entries:
        print(f"  ❌ FAILED: Found {len(bad_entries)} trades with entry > 85%")
        for trade in bad_entries[:5]:
            print(f"     Trade {trade['trade_id']}: entry={trade['entry_price']:.3f}")
    else:
        print("  ✅ PASSED: No bad entry prices found")
    print()
    
    # Test 3: Check for repeated losses on same teams
    print("Test 3: Checking for repeated losses on same teams...")
    repeated_losses = await conn.fetch("""
        SELECT 
            market_title,
            COUNT(*) as loss_count,
            SUM(pnl) as total_loss,
            MIN(time) as first_loss,
            MAX(time) as last_loss
        FROM paper_trades
        WHERE status = 'closed'
          AND outcome = 'loss'
          AND time > NOW() - INTERVAL '2 hours'
        GROUP BY market_title
        HAVING COUNT(*) >= 2
        ORDER BY loss_count DESC
    """)
    
    if repeated_losses:
        print(f"  ⚠️  WARNING: Found {len(repeated_losses)} teams with multiple losses")
        for team in repeated_losses:
            print(f"     {team['market_title']}: {team['loss_count']} losses, ${team['total_loss']:.2f}")
    else:
        print("  ✅ PASSED: No repeated losses on same teams")
    print()
    
    # Test 4: Overall P&L
    print("Test 4: Checking overall P&L...")
    pnl_stats = await conn.fetchrow("""
        SELECT 
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            COUNT(*) FILTER (WHERE outcome = 'loss') as losses,
            SUM(pnl) FILTER (WHERE outcome = 'win') as win_total,
            SUM(pnl) FILTER (WHERE outcome = 'loss') as loss_total,
            SUM(pnl) as net_pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND time > NOW() - INTERVAL '2 hours'
    """)
    
    if pnl_stats:
        wins = pnl_stats['wins'] or 0
        losses = pnl_stats['losses'] or 0
        win_total = float(pnl_stats['win_total'] or 0)
        loss_total = float(pnl_stats['loss_total'] or 0)
        net_pnl = float(pnl_stats['net_pnl'] or 0)
        
        print(f"  Wins: {wins} (${win_total:.2f})")
        print(f"  Losses: {losses} (${loss_total:.2f})")
        print(f"  Net P&L: ${net_pnl:.2f}")
        
        if net_pnl > 0:
            print("  ✅ PASSED: Positive P&L!")
        else:
            print("  ⚠️  WARNING: Negative P&L")
    print()
    
    print("=" * 80)
    print("VALIDATION COMPLETE")
    print("=" * 80)
    
    await conn.close()

if __name__ == "__main__":
    asyncio.run(validate_fixes())
```

**Update with your password:**
```bash
# Edit the password in test_validation.py
grep POSTGRES_PASSWORD .env
# Copy the password into test_validation.py
```

---

### Step 6.2: Run 24-Hour Paper Trading Test

**Saturday Evening Setup:**

```bash
# 1. Start all services with new logging
docker-compose --profile full up -d

# 2. Verify all services running
docker-compose ps

# 3. Monitor logs in separate terminals
# Terminal 1: Signal processor
docker-compose logs -f signal_processor | grep -E "🔍|✅|❌"

# Terminal 2: Execution service  
docker-compose logs -f execution_service | grep -E "💰"

# Terminal 3: Position tracker
docker-compose logs -f position_tracker | grep -E "📊|🎯|🚪"

# 4. Let it run overnight
```

**Sunday Morning - Check Results:**

```bash
# Run validation script
python3 test_validation.py

# Check detailed logs
docker-compose logs signal_processor > logs_signal_processor.txt
docker-compose logs execution_service > logs_execution_service.txt
docker-compose logs position_tracker > logs_position_tracker.txt

# Search for issues
grep "❌" logs_*.txt
grep "⚠️" logs_*.txt
```

---

### Step 6.3: Analyze Results

**Create analysis script:** `analyze_trades.sql`

```sql
-- Connect: docker exec -it arbees-timescaledb psql -U arbees -d arbees

-- Summary statistics
SELECT 
    COUNT(*) FILTER (WHERE outcome = 'win') as wins,
    COUNT(*) FILTER (WHERE outcome = 'loss') as losses,
    COUNT(*) FILTER (WHERE outcome = 'push') as pushes,
    AVG(EXTRACT(EPOCH FROM (exit_time - time))) as avg_hold_seconds,
    SUM(pnl) as total_pnl,
    AVG(pnl) FILTER (WHERE outcome = 'win') as avg_win,
    AVG(pnl) FILTER (WHERE outcome = 'loss') as avg_loss,
    AVG(entry_price) as avg_entry_price
FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours';

-- Check for problems
-- Immediate exits (< 15 seconds)
SELECT COUNT(*) as immediate_exits
FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours'
  AND EXTRACT(EPOCH FROM (exit_time - time)) < 15;

-- Bad entry prices (> 85%)
SELECT COUNT(*) as bad_entries
FROM paper_trades
WHERE time > NOW() - INTERVAL '24 hours'
  AND entry_price > 0.85;

-- Best and worst trades
SELECT 
    trade_id,
    market_title,
    side,
    entry_price,
    exit_price,
    pnl,
    outcome,
    EXTRACT(EPOCH FROM (exit_time - time)) as hold_seconds
FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours'
ORDER BY pnl DESC
LIMIT 10;
```

---

## Phase 7: Success Criteria Validation

### Must Pass ALL of These:

**✅ Criterion 1: No Immediate Exits**
```sql
SELECT COUNT(*) FROM paper_trades 
WHERE status = 'closed' 
  AND time > NOW() - INTERVAL '24 hours'
  AND EXTRACT(EPOCH FROM (exit_time - time)) < 10;
-- Must be 0
```

**✅ Criterion 2: No Bad Entry Prices**
```sql
SELECT COUNT(*) FROM paper_trades
WHERE time > NOW() - INTERVAL '24 hours'
  AND entry_price > 0.85;
-- Must be 0
```

**✅ Criterion 3: Positive P&L**
```sql
SELECT SUM(pnl) FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours';
-- Must be > 0
```

**✅ Criterion 4: Win Rate > 40%**
```sql
SELECT 
    COUNT(*) FILTER (WHERE outcome = 'win') * 100.0 / 
    NULLIF(COUNT(*), 0) as win_rate_pct
FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours';
-- Must be > 40
```

**✅ Criterion 5: Average Hold Time > 30 seconds**
```sql
SELECT AVG(EXTRACT(EPOCH FROM (exit_time - time))) 
FROM paper_trades
WHERE status = 'closed'
  AND time > NOW() - INTERVAL '24 hours';
-- Must be > 30
```

---

## Troubleshooting Guide

### Problem: Still seeing immediate exits

**Check:**
```bash
# 1. Verify min_hold_seconds is set
docker exec arbees-position-tracker env | grep MIN_HOLD

# 2. Check logs for hold time warnings
docker-compose logs position_tracker | grep "MINIMUM HOLD TIME"
```

**Fix:**
```bash
# Add to docker-compose.yml under position_tracker environment:
MIN_HOLD_SECONDS: "15.0"  # Increase if needed
```

---

### Problem: Still seeing bad entry prices

**Check:**
```bash
# 1. Check team matching confidence
docker-compose logs signal_processor | grep "TEAM MATCH VALIDATED"

# 2. Look for team matching failures
docker-compose logs signal_processor | grep "NO CONFIDENT TEAM MATCH"
```

**Fix:**
```python
# In signal_processor/processor.py
# Lower confidence threshold if too strict
if best_match and best_confidence >= 0.6:  # Was 0.7
```

---

### Problem: Not enough trades

**Check:**
```bash
# Check rejection reasons
docker-compose logs signal_processor | grep "rejected"
```

**Fix:**
```bash
# Adjust thresholds in docker-compose.yml:
MIN_EDGE_PCT: "1.5"  # Lower from 2.0
MAX_BUY_PROB: "0.98"  # Raise from 0.95
```

---

### Problem: Services crashing

**Check:**
```bash
docker-compose ps
docker-compose logs signal_processor | tail -50
```

**Fix:**
```bash
# Rebuild containers
docker-compose build
docker-compose up -d
```

---

## Timeline & Checklist

### Saturday Morning (3-4 hours)

- [ ] Phase 1: Pre-flight checks (30 min)
  - [ ] Stop trading services
  - [ ] Backup database
  - [ ] Analyze failed trades

- [ ] Phase 2: Add logging (3-4 hours)
  - [ ] SignalProcessor logging
  - [ ] ExecutionService logging
  - [ ] PositionTracker logging
  - [ ] Test each service

### Saturday Afternoon (4-5 hours)

- [ ] Phase 3: Fix team matching (4-5 hours)
  - [ ] Create TeamValidator class
  - [ ] Test TeamValidator
  - [ ] Integrate into SignalProcessor
  - [ ] Rebuild and test

- [ ] Phase 4: Fix exit monitoring (2-3 hours)
  - [ ] Add TeamValidator to PositionTracker
  - [ ] Update _get_current_price()
  - [ ] Rebuild and test

### Saturday Evening (1 hour)

- [ ] Phase 5: Add minimum hold time (30 min)
- [ ] Phase 6.2: Start 24-hour test
  - [ ] Start all services
  - [ ] Monitor logs
  - [ ] Let run overnight

### Sunday Morning (2-3 hours)

- [ ] Phase 6.3: Analyze results
  - [ ] Run validation script
  - [ ] Check success criteria
  - [ ] Review logs for issues

- [ ] Phase 7: Validation
  - [ ] Verify all 5 criteria pass
  - [ ] Document any remaining issues
  - [ ] Plan next steps

---

## Success Definition

**You have SUCCEEDED if:**

✅ No immediate exits (< 10s hold time)  
✅ No bad entry prices (> 85%)  
✅ Positive P&L over 24 hours  
✅ Win rate > 40%  
✅ Average hold time > 30 seconds  
✅ Logs show team validation working  
✅ Confidence in going to real money

**If you pass all criteria:** You're ready for real execution next week!

**If you don't pass:** We have detailed logs to debug the remaining issues.

---

## Next Steps After Success

1. **Monday:** Review weekend results with ChatGPT's implementation plan
2. **Tuesday-Wednesday:** Integration tests
3. **Thursday-Friday:** Move cooldowns to Redis
4. **Weekend 2:** Build real execution engines
5. **Week 3:** GO LIVE WITH REAL MONEY! 💰

---

## Emergency Contacts

**If things break:**
1. Stop all services: `docker-compose stop`
2. Check logs: `docker-compose logs [service]`
3. Restore backup: (see Phase 1.2)
4. Start fresh: `docker-compose down && docker-compose up`

**If you need help:**
- Logs are in: `docker-compose logs [service] > debug.log`
- Database queries in: `analyze_trades.sql`
- Validation script: `test_validation.py`

---

**Good luck! You've got this! The architecture is solid, we're just fixing logic bugs.** 🚀
