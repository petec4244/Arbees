# CRITICAL: Risk Management Implementation Plan

**Context:** Recent trading session achieved 97.9% win rate (+$5,415 profit) but operated at 26x leverage with ZERO risk controls. Model is excellent, infrastructure is dangerous.

**Objective:** Implement proper risk management before re-enabling trading

**Timeline:** 4-6 hours (implement ALL fixes before next trading session)

**Files to modify:** All in Rust services (signal_processor_rust, game_shard_rust)

---

## 🔴 **CRITICAL FINDINGS**

### **Issue 1: Risk Limits Not Enforced**
```
Configured: MAX_GAME_EXPOSURE = $100
Actual: $5,727 per game (57x over limit!)

Configured: MAX_SPORT_EXPOSURE = $800  
Actual: $26,343 total exposure (33x over limit!)

Root cause: Config values loaded but NEVER CHECKED
```

### **Issue 2: Signal Spam**
```
Observed: 260 signals/minute
Expected: 1-2 signals per game

Example:
[02:13:21] SIGNAL: Buy Blues - edge=16.6%
[02:13:24] SIGNAL: Buy Blues - edge=16.6%  <- DUPLICATE
[02:13:25] SIGNAL: Buy Blues - edge=16.6%  <- DUPLICATE
[02:13:26] SIGNAL: Buy Blues - edge=16.6%  <- DUPLICATE

Result: 80-145 positions per game instead of 1-2
```

### **Issue 3: Broken Duplicate Detection**
```rust
// Current (BROKEN):
idempotency_key = format!("{}_{}_{}", signal_id, game_id, team)
//                                     ^^^^^^^^^ NEW UUID EVERY TIME!

// Every signal gets new UUID -> "unique" -> no duplicates blocked
```

### **Issue 4: Bankroll Ignored**
```rust
// Current:
let size = bankroll * kelly * KELLY_FRACTION;
// No check: if size > available_balance -> REJECT

// Positions sized assuming INFINITE bankroll
```

---

## ✅ **IMPLEMENTATION PLAN**

### **Phase 1: Add Risk Check Infrastructure (2 hours)**

#### **File:** `services/signal_processor_rust/src/main.rs`

**Location:** Add after config loading (around line 90)

**Step 1.1: Add risk tracking functions**

```rust
impl SignalProcessor {
    /// Get current available balance for trading
    async fn get_available_balance(&self) -> Result<f64> {
        let row = sqlx::query!(
            "SELECT current_balance FROM paper_trading_state LIMIT 1"
        )
        .fetch_one(&self.db)
        .await?;
        
        Ok(row.current_balance)
    }
    
    /// Get total exposure for a specific game
    async fn get_game_exposure(&self, game_id: &str) -> Result<f64> {
        let row = sqlx::query!(
            r#"
            SELECT COALESCE(SUM(size), 0) as total
            FROM paper_trades
            WHERE game_id = $1
              AND status = 'open'
            "#,
            game_id
        )
        .fetch_one(&self.db)
        .await?;
        
        Ok(row.total.unwrap_or(0.0))
    }
    
    /// Get total exposure for a specific sport
    async fn get_sport_exposure(&self, sport: &str) -> Result<f64> {
        let row = sqlx::query!(
            r#"
            SELECT COALESCE(SUM(size), 0) as total
            FROM paper_trades
            WHERE sport = $1
              AND status = 'open'
            "#,
            sport
        )
        .fetch_one(&self.db)
        .await?;
        
        Ok(row.total.unwrap_or(0.0))
    }
    
    /// Get total loss today
    async fn get_daily_loss(&self) -> Result<f64> {
        let row = sqlx::query!(
            r#"
            SELECT COALESCE(SUM(pnl), 0) as total_pnl
            FROM paper_trades
            WHERE status = 'closed'
              AND time >= CURRENT_DATE
              AND pnl < 0
            "#
        )
        .fetch_one(&self.db)
        .await?;
        
        Ok(row.total_pnl.unwrap_or(0.0).abs())
    }
    
    /// Count open positions for a game
    async fn count_open_positions(&self, game_id: &str) -> Result<i64> {
        let row = sqlx::query!(
            r#"
            SELECT COUNT(*) as count
            FROM paper_trades
            WHERE game_id = $1
              AND status = 'open'
            "#,
            game_id
        )
        .fetch_one(&self.db)
        .await?;
        
        Ok(row.count.unwrap_or(0))
    }
}
```

---

#### **Step 1.2: Add comprehensive risk check function**

```rust
impl SignalProcessor {
    /// Check all risk limits before allowing a trade
    /// Returns None if all checks pass, Some(reason) if rejected
    async fn check_risk_limits(
        &self,
        signal: &TradingSignal,
        proposed_size: f64,
    ) -> Result<Option<String>> {
        // 1. BANKROLL CHECK
        let available = self.get_available_balance().await?;
        if proposed_size > available {
            return Ok(Some(format!(
                "INSUFFICIENT_FUNDS: need ${:.2}, have ${:.2}",
                proposed_size, available
            )));
        }
        
        // 2. DAILY LOSS LIMIT
        let daily_loss = self.get_daily_loss().await?;
        if daily_loss >= self.config.max_daily_loss {
            return Ok(Some(format!(
                "MAX_DAILY_LOSS: ${:.2} >= ${:.2}",
                daily_loss, self.config.max_daily_loss
            )));
        }
        
        // 3. GAME EXPOSURE LIMIT
        let game_exposure = self.get_game_exposure(&signal.game_id).await?;
        if game_exposure + proposed_size > self.config.max_game_exposure {
            return Ok(Some(format!(
                "MAX_GAME_EXPOSURE: ${:.2} + ${:.2} > ${:.2}",
                game_exposure, proposed_size, self.config.max_game_exposure
            )));
        }
        
        // 4. SPORT EXPOSURE LIMIT
        let sport = signal.sport.to_string();
        let sport_exposure = self.get_sport_exposure(&sport).await?;
        if sport_exposure + proposed_size > self.config.max_sport_exposure {
            return Ok(Some(format!(
                "MAX_SPORT_EXPOSURE: ${:.2} + ${:.2} > ${:.2}",
                sport_exposure, proposed_size, self.config.max_sport_exposure
            )));
        }
        
        // 5. POSITION COUNT LIMIT (max 2 per game: one buy, one sell)
        let position_count = self.count_open_positions(&signal.game_id).await?;
        if position_count >= 2 {
            return Ok(Some(format!(
                "MAX_POSITIONS_PER_GAME: {} >= 2",
                position_count
            )));
        }
        
        // All checks passed
        Ok(None)
    }
}
```

---

#### **Step 1.3: Integrate risk checks into signal processing**

**Find the `process_signal()` method and add checks BEFORE position sizing:**

```rust
async fn process_signal(&self, signal: TradingSignal) -> Result<()> {
    // ... existing validation code ...
    
    // Calculate proposed position size
    let kelly_size = self.calculate_kelly_size(&signal)?;
    
    // *** NEW: CHECK RISK LIMITS BEFORE PROCEEDING ***
    if let Some(rejection_reason) = self.check_risk_limits(&signal, kelly_size).await? {
        warn!(
            "🛑 RISK REJECTION: {} - Game: {}, Team: {}, Size: ${:.2}, Reason: {}",
            signal.signal_id,
            signal.game_id,
            signal.team,
            kelly_size,
            rejection_reason
        );
        
        // Optionally: publish rejection to Redis for monitoring
        self.publish_rejection(&signal, &rejection_reason).await?;
        
        return Ok(()); // Reject signal, don't trade
    }
    
    info!(
        "✅ RISK APPROVED: {} - Game: {}, Team: {}, Size: ${:.2}",
        signal.signal_id,
        signal.game_id,
        signal.team,
        kelly_size
    );
    
    // ... continue with execution ...
}
```

---

### **Phase 2: Fix Signal Spam (1 hour)**

#### **File:** `services/game_shard_rust/src/shard.rs`

**Step 2.1: Add debounce tracking to GameShard struct**

```rust
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct GameShard {
    // ... existing fields ...
    
    /// Track last signal time per (game_id, team) to prevent spam
    last_signal_time: HashMap<(String, String), Instant>,
}

impl GameShard {
    pub fn new(...) -> Self {
        Self {
            // ... existing initialization ...
            last_signal_time: HashMap::new(),
        }
    }
}
```

---

**Step 2.2: Add debounce logic**

```rust
const SIGNAL_DEBOUNCE_SECS: u64 = 30; // Minimum 30 seconds between same signals

impl GameShard {
    /// Check if we should emit a signal (debouncing)
    fn should_emit_signal(&mut self, game_id: &str, team: &str) -> bool {
        let key = (game_id.to_string(), team.to_string());
        let now = Instant::now();
        
        if let Some(last_time) = self.last_signal_time.get(&key) {
            let elapsed = now.duration_since(*last_time);
            if elapsed < Duration::from_secs(SIGNAL_DEBOUNCE_SECS) {
                debug!(
                    "⏭️  DEBOUNCE SKIP: {}/{} - Last signal {:.1}s ago (need {}s)",
                    game_id, team,
                    elapsed.as_secs_f64(),
                    SIGNAL_DEBOUNCE_SECS
                );
                return false;
            }
        }
        
        // Update last signal time
        self.last_signal_time.insert(key, now);
        true
    }
}
```

---

**Step 2.3: Call debounce check before publishing signals**

**Find where signals are published (likely in `process_game()` or similar) and add:**

```rust
// Before publishing signal:
if !self.should_emit_signal(&game_id, &team) {
    continue; // Skip this signal
}

// ... publish signal to Redis ...
info!(
    "📡 SIGNAL PUBLISHED: {}/{} - edge={:.1}%",
    game_id, team, edge_pct
);
```

---

### **Phase 3: Fix Idempotency Key (30 minutes)**

#### **File:** `services/signal_processor_rust/src/main.rs`

**Find where idempotency_key is created and change:**

```rust
// BEFORE (BROKEN - signal_id is new UUID every time):
let idempotency_key = format!(
    "{}_{}_{}", 
    signal.signal_id,  // ❌ NEW UUID = always unique
    signal.game_id,
    signal.team
);

// AFTER (FIXED - stable key per game/team/direction):
let idempotency_key = format!(
    "{}_{}_{}",
    signal.game_id,
    signal.team,
    signal.direction  // "buy" or "sell"
);
```

**Why this works:**
- Same game + same team + same direction = same key
- Prevents duplicate positions on same side
- Still allows one buy AND one sell per game

---

### **Phase 4: Add Position Size Limits (30 minutes)**

#### **File:** `services/signal_processor_rust/src/main.rs`

**Step 4.1: Update config to include max position size**

```rust
// Add to Config struct:
pub struct Config {
    // ... existing fields ...
    pub max_position_pct: f64,  // e.g., 5.0 = 5% of bankroll max
}

// Load in from_env():
max_position_pct: env::var("MAX_POSITION_PCT")
    .unwrap_or_else(|_| "5.0".to_string())
    .parse()
    .expect("MAX_POSITION_PCT must be a number"),
```

---

**Step 4.2: Cap position sizes in calculate_kelly_size()**

```rust
fn calculate_kelly_size(&self, signal: &TradingSignal) -> Result<f64> {
    let bankroll = self.get_available_balance().await?;
    
    // Calculate Kelly criterion
    let edge = signal.edge_pct / 100.0;
    let kelly_fraction = edge / signal.market_prob;
    
    // Apply Kelly fraction (conservative sizing)
    let kelly_size = bankroll * kelly_fraction * self.config.kelly_fraction;
    
    // *** NEW: CAP AT MAX_POSITION_PCT ***
    let max_size = bankroll * (self.config.max_position_pct / 100.0);
    let capped_size = kelly_size.min(max_size);
    
    if capped_size < kelly_size {
        debug!(
            "Position size capped: ${:.2} -> ${:.2} (max {}% of ${:.2})",
            kelly_size,
            capped_size,
            self.config.max_position_pct,
            bankroll
        );
    }
    
    Ok(capped_size.max(1.0)) // Minimum $1 trade
}
```

---

### **Phase 5: Update Configuration (15 minutes)**

#### **File:** `docker-compose.yml`

**Update signal_processor environment variables:**

```yaml
signal_processor:
  environment:
    # ... existing vars ...
    
    # UPDATED: More conservative risk limits
    MAX_DAILY_LOSS: "100.0"        # Was 500.0 - reduce until confident
    MAX_GAME_EXPOSURE: "50.0"      # Was 100.0 - enforce strictly  
    MAX_SPORT_EXPOSURE: "200.0"    # Was 800.0 - reduce exposure
    MIN_EDGE_PCT: "5.0"            # Was 2.0 - higher bar for signals
    KELLY_FRACTION: "0.10"         # Was 0.25 - smaller positions
    MAX_POSITION_PCT: "5.0"        # Was 10.0 - cap individual trades
```

**Add new game_shard environment variable:**

```yaml
game_shard:
  environment:
    # ... existing vars ...
    
    # NEW: Signal debouncing
    SIGNAL_DEBOUNCE_SECS: "30"    # Min 30s between duplicate signals
```

---

## 🧪 **TESTING PLAN**

### **Test 1: Risk Limits Work**

**Scenario:** Bankroll = $100, try to open $150 position

**Expected:**
```
🛑 RISK REJECTION: INSUFFICIENT_FUNDS: need $150.00, have $100.00
```

---

### **Test 2: Game Exposure Limit**

**Scenario:** Open $30 position on game, try to open another $30 (limit = $50)

**Expected:**
```
✅ First trade: APPROVED
🛑 Second trade: MAX_GAME_EXPOSURE: $30.00 + $30.00 > $50.00
```

---

### **Test 3: Signal Debouncing**

**Scenario:** Publish same signal 5 times in 10 seconds

**Expected:**
```
📡 SIGNAL PUBLISHED: game_123/Lakers - edge=8.5%
⏭️  DEBOUNCE SKIP: game_123/Lakers - Last signal 2.1s ago (need 30s)
⏭️  DEBOUNCE SKIP: game_123/Lakers - Last signal 4.3s ago (need 30s)
...
```

---

### **Test 4: Idempotency Works**

**Scenario:** Process same signal twice (same game/team/direction)

**Expected:**
```
✅ First signal: Trade executed
⏭️  Second signal: Duplicate detected (idempotency_key collision)
```

---

### **Test 5: Position Size Capping**

**Scenario:** Kelly suggests $80 position, max is 5% of $1000 = $50

**Expected:**
```
Position size capped: $80.00 -> $50.00 (max 5% of $1000.00)
```

---

## 📊 **VALIDATION CHECKLIST**

Before re-enabling trading, verify ALL of these:

### **Code Checks:**
- [ ] `check_risk_limits()` function exists
- [ ] `get_available_balance()` queries database
- [ ] `get_game_exposure()` sums open positions
- [ ] `get_sport_exposure()` filters by sport
- [ ] `get_daily_loss()` sums losses today
- [ ] Risk checks called BEFORE position sizing
- [ ] Debounce logic in game_shard
- [ ] Idempotency key uses game/team/direction (not signal_id)
- [ ] Position sizes capped at MAX_POSITION_PCT

### **Configuration Checks:**
- [ ] MAX_DAILY_LOSS = 100.0
- [ ] MAX_GAME_EXPOSURE = 50.0
- [ ] MAX_SPORT_EXPOSURE = 200.0
- [ ] MIN_EDGE_PCT = 5.0
- [ ] KELLY_FRACTION = 0.10
- [ ] MAX_POSITION_PCT = 5.0
- [ ] SIGNAL_DEBOUNCE_SECS = 30

### **Database Checks:**
- [ ] paper_trading_state table has current_balance
- [ ] paper_trades table has status, game_id, sport, time, pnl columns

### **Test Results:**
- [ ] Test 1 passed (insufficient funds rejected)
- [ ] Test 2 passed (game exposure limit enforced)
- [ ] Test 3 passed (signals debounced)
- [ ] Test 4 passed (duplicates blocked)
- [ ] Test 5 passed (position sizes capped)

---

## 🚀 **DEPLOYMENT SEQUENCE**

### **Step 1: Build Updated Services**

```bash
cd P:\petes_code\ClaudeCode\Arbees

# Rebuild Rust services
docker-compose build signal_processor_rust
docker-compose build game_shard_rust

# Verify builds successful (no errors)
```

---

### **Step 2: Deploy with Safety**

```bash
# Stop current trading
docker-compose stop signal_processor game_shard execution_service

# Start updated services
docker-compose up -d signal_processor_rust game_shard_rust

# Monitor logs
docker-compose logs -f signal_processor_rust | grep -E "RISK|APPROVED|REJECTION"
docker-compose logs -f game_shard_rust | grep -E "SIGNAL|DEBOUNCE"
```

---

### **Step 3: Validate in Paper Trading**

```bash
# Let system run for 30 minutes
# Watch for:

# 1. Risk rejections working:
docker-compose logs signal_processor_rust | grep "RISK REJECTION"

# 2. Debouncing working:
docker-compose logs game_shard_rust | grep "DEBOUNCE SKIP"

# 3. Reasonable position counts:
docker exec arbees-timescaledb psql -U arbees -d arbees -c \
  "SELECT game_id, COUNT(*) FROM paper_trades WHERE status='open' GROUP BY game_id;"
# Should see 0-2 positions per game (not 80-145!)

# 4. Exposure within limits:
docker exec arbees-timescaledb psql -U arbees -d arbees -c \
  "SELECT game_id, SUM(size) FROM paper_trades WHERE status='open' GROUP BY game_id;"
# Should see < $50 per game
```

---

## ⚠️ **CRITICAL REMINDERS**

1. **DO NOT skip any phase** - All 5 are required for safety
2. **DO NOT increase limits** until system proves stable
3. **DO NOT re-enable trading** until ALL validation checks pass
4. **DO monitor first hour closely** - be ready to kill switch if needed

---

## 📈 **EXPECTED RESULTS AFTER FIXES**

### **Before (Dangerous):**
```
Signals: 260/minute
Positions: 80-145 per game
Max exposure: $5,727 per game
Leverage: 26x
```

### **After (Safe):**
```
Signals: 1-2 per game per 30 seconds = ~4-8/minute
Positions: 0-2 per game (one buy, one sell max)
Max exposure: $50 per game (enforced)
Leverage: <0.2x (200% total exposure on $1000 bankroll)
```

### **Performance Impact:**
```
Before: 1,056 trades, 97.9% win rate, +$5,415
After: ~20-40 trades, 97.9% win rate (model unchanged), +$200-400

Why fewer trades?
  - Debouncing: 260 signals -> 8 signals
  - Position limits: 1-2 per game instead of 80-145
  - Risk checks: Exposure capped

Why still profitable?
  - Model accuracy unchanged (97.9% win rate)
  - Taking best edges (MIN_EDGE_PCT = 5.0%)
  - Smaller positions = smaller gains BUT also smaller risk
```

---

## 🎯 **SUCCESS CRITERIA**

After implementing all fixes, a successful session looks like:

- ✅ 0-2 open positions per game
- ✅ Total exposure < 20% of bankroll
- ✅ All risk limits enforced
- ✅ No signal spam (reasonable rate)
- ✅ Win rate 70%+ (model still works)
- ✅ Steady profit growth (even if smaller)
- ✅ No catastrophic losses possible

---

## 💡 **KEY INSIGHT**

**You discovered the model works (97.9% win rate).** That's HUGE!

**Now you need to protect it with proper risk management.**

Think of it like this:
- **Before:** Ferrari with no brakes (fast but deadly)
- **After:** Ferrari with brakes (still fast, now safe)

The model is your edge. Risk management keeps you alive to use it.

---

**GOOD LUCK! Let's turn that 97.9% win rate into sustainable profits!** 🚀💰
