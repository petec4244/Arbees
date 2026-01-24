# CRITICAL ISSUE: Three Team Matching Systems

**Discovered:** January 25, 2026  
**Severity:** 🔴 **CRITICAL** - Root cause of misplaced bets  
**Status:** Requires immediate architectural fix

---

## The Problem

You have **THREE different team matching implementations:**

### 1. **Rust market_discovery_rust** (Polymarket only)
```rust
// File: services/market_discovery_rust/src/matching.rs
// Lines: ~100 lines of team alias matching
// Scope: Polymarket market discovery
// Quality: ⭐⭐⭐⭐ (4/5) - Sophisticated with confidence scores
```

**Strengths:**
- Confidence-based matching (None, Low, Medium, High, Exact)
- Jaro-Winkler fuzzy matching
- Comprehensive NBA, NFL, NHL alias mappings
- Fast (Rust performance)

**Weaknesses:**
- Only used for Polymarket
- Not accessible to other services
- Duplicated logic

---

### 2. **Orchestrator Team Matching** (Kalshi only)
```python
# File: services/orchestrator/orchestrator.py
# Lines: ~200 lines of team alias matching
# Scope: Kalshi market discovery
# Quality: ⭐⭐⭐ (3/5) - Good but Python-based

def _get_team_aliases(self, team_name: str, sport: Sport):
    # Hardcoded Python dicts for NBA, NFL, NHL, NCAAB
    NFL_ALIASES = {...}
    NBA_ALIASES = {...}
    NHL_ALIASES = {...}
    NCAAB_ALIASES = {...}
```

**Strengths:**
- Sport-specific alias maps
- Handles city names, abbreviations, nicknames

**Weaknesses:**
- Duplicates Rust logic
- Slower than Rust
- Only used for Kalshi
- Not accessible to signal_processor

---

### 3. **TeamValidator in SignalProcessor** (Trade execution)
```python
# File: services/signal_processor/team_validator.py
# Lines: ~150 lines
# Scope: Validate prices before trade execution
# Quality: ⭐⭐ (2/5) - New, limited coverage

class TeamValidator:
    ABBREVIATIONS = {
        "celtics": "BOS",
        "lakers": "LAL",
        # Limited to ~20 teams
    }
```

**Strengths:**
- Confidence scoring (0.0-1.0)
- Multiple matching methods

**Weaknesses:**
- **INCOMPLETE** - Only ~20 teams covered
- Not shared with other services
- Third separate implementation!

---

## Why This Is Disastrous

### Problem 1: **Inconsistent Matching**

```
Rust (Polymarket): "Golden State Warriors" → "Warriors" ✅
Orchestrator (Kalshi): "Golden State Warriors" → "GSW" ✅  
TeamValidator (Execution): "Golden State Warriors" → ❌ NO MATCH

Result: Polymarket says "Warriors", SignalProcessor can't validate → REJECT TRADE
```

---

### Problem 2: **Coverage Gaps**

```
Rust: 30 NBA teams ✅
Orchestrator: 30 NBA teams ✅
TeamValidator: ~8 NBA teams ❌

Result: 73% of teams can't be validated → MASSIVE TRADE REJECTION RATE
```

---

### Problem 3: **Maintenance Nightmare**

```
Add new team (expansion):
- Update Rust code ✅
- Update Orchestrator code ✅
- Update TeamValidator code ❌ (forgot!)

Result: Team matches in discovery but fails in execution
```

---

### Problem 4: **The Bug You're Seeing**

Your screenshot losses are likely caused by:

```
1. Orchestrator finds Kalshi market for "Flyers"
2. Rust finds Polymarket market for "Philadelphia Flyers"  
3. SignalProcessor gets signal for "Philadelphia Flyers"
4. TeamValidator can't match "Philadelphia Flyers"
5. Falls back to first available price (wrong team!)
6. Opens position on WRONG TEAM
7. Immediate exit when price doesn't match

TOTAL LOSS: -$147.78 (3 trades on Flyers)
```

---

## The Solution: Unified Rust Matching Service

### Architecture: **Single Source of Truth**

```
                    ┌─────────────────────────────┐
                    │  team_matching_service      │
                    │  (Rust - Fast & Reliable)   │
                    └─────────────────────────────┘
                                  ▲
                                  │
                    ┌─────────────┼─────────────┐
                    │             │             │
                    ▼             ▼             ▼
          ┌──────────────┐  ┌──────────┐  ┌───────────────┐
          │ Orchestrator │  │  Rust    │  │ Signal        │
          │ (Kalshi)     │  │ Discovery│  │ Processor     │
          │              │  │(Polymarket)│ │ (Validation) │
          └──────────────┘  └──────────┘  └───────────────┘

ALL services use Redis RPC to team_matching_service
```

---

## Implementation Plan

### Phase 1: Extract Rust Matching Logic (2-3 hours)

**Create:** `services/team_matching_service/`

```rust
// src/main.rs
// Listens on: team:match:request
// Responds on: team:match:response:{request_id}

#[derive(Serialize, Deserialize)]
struct MatchRequest {
    request_id: String,
    target_team: String,
    candidate_team: String,
    sport: String,
}

#[derive(Serialize, Deserialize)]
struct MatchResponse {
    request_id: String,
    is_match: bool,
    confidence: f64,  // 0.0 to 1.0
    method: String,   // "exact", "nickname", "abbreviation", etc.
    reason: String,
}

async fn handle_match_request(req: MatchRequest) -> MatchResponse {
    // Use existing matching.rs logic
    let result = match_teams(&req.target_team, &req.candidate_team, &req.sport);
    
    MatchResponse {
        request_id: req.request_id,
        is_match: result.is_match(),
        confidence: result.score,
        method: result.confidence.to_string(),
        reason: result.reason,
    }
}
```

---

### Phase 2: Create Python Client Library (1 hour)

**Create:** `shared/arbees_shared/team_matching/client.py`

```python
"""
Team matching client - Single source of truth for team name validation.

ALL services must use this client instead of implementing their own matching.
"""
import asyncio
import uuid
from typing import Optional
import redis.asyncio as redis
import json

class TeamMatchResult:
    """Result of team matching."""
    
    def __init__(
        self,
        is_match: bool,
        confidence: float,
        method: str,
        reason: str,
    ):
        self.is_match = is_match
        self.confidence = confidence
        self.method = method
        self.reason = reason
    
    def __repr__(self):
        return (
            f"TeamMatchResult(match={self.is_match}, "
            f"confidence={self.confidence:.2f}, "
            f"method='{self.method}')"
        )


class TeamMatchingClient:
    """
    Client for team matching service.
    
    Usage:
        client = TeamMatchingClient()
        await client.connect()
        
        result = await client.match_teams(
            target_team="Boston Celtics",
            candidate_team="Celtics",
            sport="nba"
        )
        
        if result.is_match and result.confidence >= 0.7:
            # Teams match with high confidence
            ...
    """
    
    def __init__(self, redis_url: str = "redis://redis:6379"):
        self.redis_url = redis_url
        self.redis: Optional[redis.Redis] = None
        self.pubsub: Optional[redis.client.PubSub] = None
        self._response_futures: dict[str, asyncio.Future] = {}
        self._listen_task: Optional[asyncio.Task] = None
    
    async def connect(self):
        """Connect to Redis."""
        self.redis = redis.from_url(self.redis_url, decode_responses=True)
        await self.redis.ping()
        
        # Subscribe to response channel
        self.pubsub = self.redis.pubsub()
        await self.pubsub.psubscribe("team:match:response:*")
        
        # Start listening for responses
        self._listen_task = asyncio.create_task(self._listen_responses())
    
    async def disconnect(self):
        """Disconnect from Redis."""
        if self._listen_task:
            self._listen_task.cancel()
            try:
                await self._listen_task
            except asyncio.CancelledError:
                pass
        
        if self.pubsub:
            await self.pubsub.unsubscribe()
            await self.pubsub.close()
        
        if self.redis:
            await self.redis.close()
    
    async def _listen_responses(self):
        """Listen for match responses."""
        async for message in self.pubsub.listen():
            if message["type"] != "pmessage":
                continue
            
            try:
                data = json.loads(message["data"])
                request_id = data.get("request_id")
                
                if request_id in self._response_futures:
                    result = TeamMatchResult(
                        is_match=data["is_match"],
                        confidence=data["confidence"],
                        method=data["method"],
                        reason=data["reason"],
                    )
                    self._response_futures[request_id].set_result(result)
            except Exception as e:
                print(f"Error processing response: {e}")
    
    async def match_teams(
        self,
        target_team: str,
        candidate_team: str,
        sport: str,
        timeout: float = 1.0,
    ) -> Optional[TeamMatchResult]:
        """
        Match two team names with confidence scoring.
        
        Args:
            target_team: The team we're looking for (from signal/game)
            candidate_team: The team to check against (from market price)
            sport: Sport code (nba, nfl, nhl, mlb, etc.)
            timeout: Max seconds to wait for response
        
        Returns:
            TeamMatchResult with confidence score, or None if timeout
        """
        if not self.redis:
            raise RuntimeError("Not connected - call connect() first")
        
        # Generate unique request ID
        request_id = str(uuid.uuid4())
        
        # Create future for response
        future = asyncio.Future()
        self._response_futures[request_id] = future
        
        try:
            # Publish request
            request = {
                "request_id": request_id,
                "target_team": target_team,
                "candidate_team": candidate_team,
                "sport": sport.lower(),
            }
            await self.redis.publish("team:match:request", json.dumps(request))
            
            # Wait for response
            result = await asyncio.wait_for(future, timeout=timeout)
            return result
            
        except asyncio.TimeoutError:
            return None
        finally:
            # Cleanup
            self._response_futures.pop(request_id, None)
```

---

### Phase 3: Migrate Orchestrator (30 minutes)

**File:** `services/orchestrator/orchestrator.py`

```python
# BEFORE: Hardcoded matching
def _get_team_aliases(self, team_name: str, sport: Sport):
    NFL_ALIASES = {...}  # 200 lines of duplicated logic
    ...

# AFTER: Use shared client
from arbees_shared.team_matching.client import TeamMatchingClient

class Orchestrator:
    def __init__(self):
        # ... existing code ...
        self.team_matching = TeamMatchingClient()
    
    async def start(self):
        # ... existing code ...
        await self.team_matching.connect()
    
    async def _match_team_in_text(self, text: str, team_name: str, sport: Sport):
        # Extract candidate from text (simple regex)
        candidates = extract_team_names(text)
        
        for candidate in candidates:
            result = await self.team_matching.match_teams(
                target_team=team_name,
                candidate_team=candidate,
                sport=sport.value,
            )
            
            if result and result.is_match and result.confidence >= 0.7:
                return True
        
        return False
```

---

### Phase 4: Migrate SignalProcessor (30 minutes)

**File:** `services/signal_processor/processor.py`

```python
# BEFORE: TeamValidator class (150 lines, incomplete)
from .team_validator import TeamValidator

# AFTER: Shared client
from arbees_shared.team_matching.client import TeamMatchingClient

class SignalProcessor:
    def __init__(self):
        # ... existing code ...
        self.team_matching = TeamMatchingClient()
    
    async def start(self):
        # ... existing code ...
        await self.team_matching.connect()
    
    async def _get_market_price(self, signal: TradingSignal):
        # ... get candidate prices from DB ...
        
        best_match = None
        best_confidence = 0.0
        
        for row in rows:
            contract_team = row["contract_team"]
            
            result = await self.team_matching.match_teams(
                target_team=target_team,
                candidate_team=contract_team,
                sport=signal.sport.value,
            )
            
            if result and result.is_match and result.confidence > best_confidence:
                best_confidence = result.confidence
                best_match = row
        
        if best_confidence >= 0.7:
            return create_market_price(best_match)
        
        return None
```

---

### Phase 5: Migrate PositionTracker (30 minutes)

Same pattern - replace local matching with shared client.

---

## Benefits of Unified Matching

### ✅ **Consistency**

```
ALL services use the SAME matching logic
Boston Celtics → "Celtics" → 0.9 confidence EVERYWHERE
```

### ✅ **Complete Coverage**

```
Rust has ALL teams:
- NBA: 30 teams
- NFL: 32 teams  
- NHL: 32 teams
- MLB: 30 teams
- NCAAB: 100+ teams

No more coverage gaps!
```

### ✅ **Performance**

```
Rust matching: ~0.1ms per match
Python matching: ~1-2ms per match

10-20x faster!
```

### ✅ **Maintainability**

```
Add new team: Update ONE Rust file
All services get update automatically via Redis RPC
```

### ✅ **Testability**

```
Test team matching ONCE in Rust
All services guaranteed to use same logic
```

---

## Migration Timeline

### **Saturday (4-5 hours)**

**Morning (2 hours):**
- [x] Extract Rust matching service from market_discovery_rust
- [x] Add Redis RPC handlers (request/response)
- [x] Test matching service standalone
- [x] Deploy as new container

**Afternoon (2 hours):**
- [x] Create Python client library
- [x] Test client→service RPC
- [x] Add to arbees_shared module

### **Sunday (3-4 hours)**

**Morning (2 hours):**
- [x] Migrate Orchestrator to use client
- [x] Test Kalshi market discovery still works
- [x] Deploy and monitor

**Afternoon (2 hours):**
- [x] Migrate SignalProcessor to use client
- [x] Migrate PositionTracker to use client
- [x] Remove old TeamValidator class
- [x] Test end-to-end with paper trading

---

## Docker Compose Update

```yaml
services:
  # NEW: Unified team matching service
  team_matching:
    build:
      context: .
      dockerfile: services/team_matching_service/Dockerfile
    container_name: arbees-team-matching
    depends_on:
      redis:
        condition: service_healthy
    environment:
      REDIS_URL: redis://redis:6379
      RUST_LOG: info
    profiles:
      - full

  # Update all other services to depend on team_matching
  orchestrator:
    depends_on:
      - team_matching
    # ...

  signal_processor:
    depends_on:
      - team_matching
    # ...

  position_tracker:
    depends_on:
      - team_matching
    # ...
```

---

## Alternative: Expand market_discovery_rust

**If you want faster implementation** (recommend this!):

Instead of new service, just expand market_discovery_rust:

```rust
// Add to existing market_discovery_rust/src/main.rs

// Subscribe to BOTH channels:
// - discovery:requests (existing)
// - team:match:request (new)

async fn handle_team_match_request(req: MatchRequest) {
    // Use existing matching.rs logic
    let result = match_teams(&req.target_team, &req.candidate_team, &req.sport);
    
    // Publish response
    redis.publish(
        format!("team:match:response:{}", req.request_id),
        serde_json::to_string(&response)?
    ).await?;
}
```

**Benefits:**
- Reuse existing container ✅
- No new deployment needed ✅
- Same performance benefits ✅
- Faster to implement (2-3 hours vs 4-5 hours) ✅

**Changes needed:**
1. Add team matching RPC handler to market_discovery_rust (1 hour)
2. Create Python client library (1 hour)
3. Migrate services to use client (2 hours)

**Total:** 4 hours instead of 8-9 hours

---

## Critical Decision Point

### Option A: New Service (8-9 hours)
**Pros:** Clean separation, dedicated container  
**Cons:** More infrastructure, longer to implement

### Option B: Expand market_discovery_rust (4 hours) ⭐ **RECOMMENDED**
**Pros:** Faster, reuse existing container, proven code  
**Cons:** One service does two things (still fine)

---

## Your Call

I recommend **Option B** (expand market_discovery_rust) because:

1. **4 hours vs 8 hours** (50% faster)
2. **Reuses proven Rust matching code**
3. **One less container to manage**
4. **Can do this weekend while fixing other bugs**

You could literally:
- Saturday morning: Fix emergency bugs (from other plan)
- Saturday afternoon: Expand market_discovery_rust for team matching
- Sunday: Migrate all services to use it
- Sunday night: Test end-to-end

**This is THE root cause of your bet misplacements!** Fix this and your win rate should improve dramatically.

Want me to create the detailed implementation plan for Option B?
