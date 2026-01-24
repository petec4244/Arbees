# PLANNING MODE PROMPT: Real Execution Engine - GO LIVE THIS WEEKEND

## Mission Critical

**Goal:** Build real order execution for Kalshi and Polymarket so we can START TRADING WITH REAL MONEY THIS WEEKEND.

**Context:** 
- You're 95% done - everything works except actual order placement
- Paper trading showed $2K profit (even with edge bugs - we'll fix those)
- This is THE ONLY blocker to going live
- Need to fund HiWave browser development and eventually quit day job
- **Weekend deadline: Must be testable by Sunday!**

**Success Criteria:**
✅ Can place real limit orders on Kalshi
✅ Can place real limit orders on Polymarket
✅ Orders execute, fills confirmed, positions tracked
✅ Paper trading toggle works (test before going live)
✅ Ready to test with $10 positions on Sunday

---

## What You're Building

### Two New Execution Engines

**1. KalshiExecutionEngine** (`markets/kalshi/execution.py`)
- Place limit orders on Kalshi markets
- Check order status
- Confirm fills
- Handle errors and retries

**2. PolymarketExecutionEngine** (`markets/polymarket/execution.py`)
- Place limit orders on Polymarket CLOB
- Check order status  
- Confirm fills
- Handle authentication

**3. Integration** (`services/position_manager/position_manager.py`)
- Toggle between paper trading and real execution
- Wire up both engines
- Track fills in database
- Update positions

**Total Code:** ~300-400 lines across 3 files

---

## Reference Implementations You Have

### Kalshi: Working Demo Code

**Location:** `P:\petes_code\ClaudeCode\Arbees\kalshi_advanced_limit_demo.py`

**What it does:**
```python
# Already working Kalshi limit order placement!
async def place_limit_order(
    market_ticker: str,
    side: str,  # "yes" or "no"
    price_cents: int,  # 1-99
    count: int,  # number of contracts
):
    # 1. Sign request with RSA private key
    # 2. POST to /v2/portfolio/orders
    # 3. Get order_id back
    # 4. Poll /v2/portfolio/orders/{order_id} for fill
    # 5. Return fill confirmation
```

**Your task:** Extract this into clean class

---

### Polymarket: Reference Implementation

**Location:** `P:\petes_code\ClaudeCode\Arbees\Polymarket-Kalshi-Arbitrage-bot\src\polymarket_clob.rs`

**What it does:**
```rust
// Polymarket CLOB order placement (Rust)
async fn place_order(
    token_id: String,
    side: Side,  // BUY or SELL
    price: f64,  // 0.0 - 1.0
    size: f64,   // dollar amount
) -> OrderResult
```

**Available Python library:**
```bash
pip install py-clob-client
```

**Documentation:** https://github.com/Polymarket/py-clob-client

**Your task:** Wrap py-clob-client library

---

## Implementation Plan

### Phase 1: Kalshi Execution Engine (2-3 hours)

Create `markets/kalshi/execution.py` with:
- `KalshiExecutionEngine` class
- `_place_limit_order()` method (extract from demo)
- `_wait_for_fill()` method (poll for execution)
- Error handling and retry logic
- Logging for debugging

### Phase 2: Polymarket Execution Engine (2-3 hours)

Create `markets/polymarket/execution.py` with:
- Install `py-clob-client` library
- `PolymarketExecutionEngine` class
- `_place_limit_order()` using py-clob-client
- `_check_order_status()` for fills
- Error handling and retry logic

### Phase 3: Integration (1-2 hours)

Modify `services/position_manager/position_manager.py`:
- Add `PAPER_TRADING` environment variable check
- Initialize real or paper executors based on mode
- Route signals to correct executor
- Add clear logging (PAPER vs LIVE mode)
- Safety warnings for live trading

---

## Testing Strategy

### Stage 1: Paper Trading Validation (Friday Night)
```bash
PAPER_TRADING=1
docker-compose up
```
Verify everything works as before.

### Stage 2: Build Engines (Saturday Morning)
Extract Kalshi demo → `KalshiExecutionEngine`
Implement Polymarket → `PolymarketExecutionEngine`

### Stage 3: Integration (Saturday Afternoon)
Wire into position_manager
Test mode switching
Verify logging

### Stage 4: Live Test with $10 (Saturday Evening)
```bash
PAPER_TRADING=0  # 🚨 LIVE TRADING!
MAX_DAILY_LOSS=50.0
MAX_GAME_EXPOSURE=10.0
```
Place ONE $10 order
Verify fill and tracking
Monitor closely

---

## Weekend Schedule

### Friday Night (6pm - 10pm)
- Review this prompt
- Validate paper trading works
- Read reference code

### Saturday (8am - 8pm)
- Build execution engines (morning)
- Integration + tests (afternoon)
- First $10 live test (evening)

### Sunday
- Monitor system
- Validate trades
- Fix any bugs
- Prepare to scale!

---

## Critical Safety Checks

**Before EVERY order:**
1. Verify PAPER_TRADING mode is correct
2. Sanity check order size (< 100 contracts)
3. Sanity check price (1-99 cents)
4. Check circuit breaker status

**Logging requirements:**
```python
logger.info(f"🎯 [{mode}] Placing order: {ticker} @ {price}¢")
logger.info(f"✅ [{mode}] Order filled: {order_id}")
logger.error(f"❌ [{mode}] Order failed: {error}")
```

---

## Environment Variables

Add to `.env`:
```bash
# TRADING MODE
PAPER_TRADING=1  # Set to 0 for LIVE!

# KALSHI CREDENTIALS
KALSHI_API_KEY_ID=your_key
KALSHI_PRIVATE_KEY_PATH=/path/to/key.pem

# POLYMARKET CREDENTIALS  
POLY_PRIVATE_KEY=0xYOUR_PRIVATE_KEY

# RISK LIMITS
MAX_DAILY_LOSS=500.0
MAX_GAME_EXPOSURE=100.0
MIN_EDGE_PCT=2.0
```

---

## Success Metrics

**By Sunday night:**
✅ Real execution working on both platforms
✅ Made 1+ real trades with $10  
✅ Fills confirmed and tracked
✅ P&L calculated correctly
✅ No critical bugs

**Next week:**
Scale to $50-100 positions
Monitor daily
Build confidence

**Month 1-3:**
Build runway ($10K-15K)
Fund HiWave development

**Month 6:**
QUIT DAY JOB! 🎉
Work on HiWave + Arbees full-time

---

## The Dream

This weekend is the first step to:
- ✅ Fund HiWave browser development
- ✅ Quit crappy day job
- ✅ Work on YOUR projects full-time
- ✅ YOUR DREAM JOB

**You're 95% done. Just 200 lines of code between you and your dreams.**

**LET'S GO LIVE THIS WEEKEND! 🚀💰**

---

## For Claude Code Planning Mode

When you paste this into Claude Code, ask it to:

1. **Analyze** the existing code:
   - `kalshi_advanced_limit_demo.py` (working Kalshi orders)
   - `services/position_manager/position_manager.py` (integration point)
   - `markets/paper/engine.py` (interface to match)

2. **Create detailed plan** for:
   - Extracting Kalshi execution into clean class
   - Implementing Polymarket execution with py-clob-client
   - Integrating with position_manager
   - Testing strategy with $10 orders

3. **Identify risks** and mitigation:
   - What could go wrong?
   - How to prevent it?
   - How to detect it?

4. **Provide timeline**:
   - Hour-by-hour breakdown
   - Milestones and checkpoints
   - Success criteria

**MOST IMPORTANT:** A plan you can EXECUTE THIS WEEKEND!
