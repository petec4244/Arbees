# Execution Trace: PAPER_TRADING=0 (Live Trading Mode)

**⚠️ CRITICAL: This document traces what happens when live trading is enabled**

Date: 2026-01-28
Analysis Type: Code Flow Trace with Real Money Implications

---

## TL;DR - What Happens?

When `PAPER_TRADING=0` is set:
1. ✅ **Safeguards exist** - Credentials required, price/side validation, IOC/FAK orders only
2. ⚠️ **No confirmation prompt** - Code will place real orders without asking
3. ⚠️ **No dry-run mode** - Straight from paper to live (no intermediate testing mode)
4. ⚠️ **No order size limits** - Only minimum 1 contract, no maximum
5. ⚠️ **No balance checks** - Doesn't verify account has sufficient funds
6. ✅ **Safe order types** - IOC/FAK orders never rest on book (reduces one-sided fill risk)

---

## Complete Execution Flow

### Step 1: Service Startup

**File:** `services/execution_service_rust/src/main.rs:55-57`

```rust
let paper_trading_val = env::var("PAPER_TRADING").unwrap_or_else(|_| "1".to_string());
let paper_trading = matches!(paper_trading_val.to_lowercase().as_str(), "1" | "true" | "yes");
let engine = ExecutionEngine::new(paper_trading).await;
```

**What Happens:**
- Reads `PAPER_TRADING` from environment
- **Default:** `"1"` (paper trading ON)
- **Accepts:** `"1"`, `"true"`, `"yes"` as paper trading ON (case insensitive)
- **All other values** (including `"0"`, `"false"`, `"no"`) → paper trading OFF (live mode)
- Passes boolean to `ExecutionEngine::new()`

**Console Output:**
```
Execution Service ready (Paper Trading: false, Kalshi Live: true, Polymarket Live: true, Transport: ZmqOnly)
```

---

### Step 2: Engine Initialization

**File:** `services/execution_service_rust/src/engine.rs:60-99`

```rust
pub async fn new(paper_trading: bool) -> Self {
    // Try to load Kalshi credentials from environment
    let kalshi = match KalshiClient::from_env() {
        Ok(client) => {
            if client.has_credentials() {
                info!("Kalshi client initialized with trading credentials");
            } else {
                warn!("Kalshi client initialized without credentials (will reject live trades)");
            }
            client
        }
        // ...
    };

    // Try to initialize Polymarket CLOB executor (only for live trading)
    let polymarket_executor = if !paper_trading {
        match PolymarketExecutor::from_env().await {
            Ok(exec) => {
                info!("Polymarket CLOB executor initialized");
                Some(exec)
            }
            Err(e) => {
                warn!("Polymarket CLOB executor not available: {}", e);
                None
            }
        }
    } else {
        None
    };

    Self {
        kalshi,
        polymarket: PolymarketClient::new(),
        polymarket_executor,
        paper_trading,
    }
}
```

**What Happens:**

**If `paper_trading = false` (live mode):**
1. **Kalshi Setup:**
   - Reads `KALSHI_API_KEY` from env
   - Reads `KALSHI_PRIVATE_KEY` or `KALSHI_PRIVATE_KEY_PATH` from env
   - Reads `KALSHI_ENV` (default: "prod")
   - If credentials found: "Kalshi client initialized with trading credentials" ✅
   - If missing: "Kalshi client initialized without credentials (will reject live trades)" ⚠️

2. **Polymarket Setup:**
   - Reads `POLYMARKET_PRIVATE_KEY` from env (L1 wallet private key)
   - Reads `POLYMARKET_FUNDER_ADDRESS` from env
   - Reads `POLYMARKET_CHAIN_ID` (default: 137 = mainnet)
   - Reads `POLYMARKET_CLOB_HOST` (default: https://clob.polymarket.com)
   - Derives API credentials using nonce
   - If successful: "Polymarket CLOB executor initialized" ✅
   - If missing: "Polymarket CLOB executor not available" ⚠️

**Console Output Example:**
```
INFO  Kalshi client initialized with trading credentials
INFO  Initializing Polymarket CLOB executor: host=https://clob.polymarket.com, chain_id=137
INFO  Deriving API credentials with nonce=0
INFO  Polymarket CLOB executor initialized successfully
```

---

### Step 3: Execution Request Received

**File:** `services/execution_service_rust/src/main.rs:320`

When a signal is generated and passes through signal_processor_rust, an `ExecutionRequest` arrives via:
- ZMQ topic: `execution.request.*` (if ZMQ mode)
- Redis channel: `execution:requests` (if Redis mode)

**Example ExecutionRequest:**
```json
{
  "request_id": "req_abc123",
  "idempotency_key": "idempotent_xyz",
  "platform": "Kalshi",
  "market_id": "NBAHOU-BOSJAN28",
  "token_id": null,
  "contract_team": "Celtics",
  "game_id": "nba_12345",
  "sport": "NBA",
  "signal_id": "signal_789",
  "signal_type": "ModelEdgeYes",
  "edge_pct": 18.5,
  "side": "Yes",
  "limit_price": 0.65,
  "size": 50.0,
  "created_at": "2026-01-28T20:30:00Z"
}
```

**Signal Age Check:**
```
INFO  ZMQ signal: req_abc123 age=142ms (ZMQ latency: 8ms)
```

---

### Step 4: Execution Dispatch

**File:** `services/execution_service_rust/src/engine.rs:111-149`

```rust
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    info!("Executing request: {:?}", request);

    if self.paper_trading {
        // Paper trading logic with realistic fee calculation
        info!("Paper trade simulation for {}", request.request_id);
        // ... returns simulated result
    }

    // Real execution
    match request.platform {
        Platform::Kalshi => {
            self.execute_kalshi(request).await
        }
        Platform::Polymarket => {
            if let Some(executor) = &self.polymarket_executor {
                executor.execute(request).await
            } else {
                // Rejection: Polymarket not configured
            }
        }
        Platform::Paper => {
            // Always simulated, even in live mode
        }
    }
}
```

**Critical Decision Point:**
- **Line 114:** If `self.paper_trading = true`, returns paper trade result immediately
- **Line 152-188:** If `self.paper_trading = false`, routes to REAL execution:
  - **Kalshi** → `execute_kalshi()` - **PLACES REAL ORDERS**
  - **Polymarket** → `executor.execute()` - **PLACES REAL ORDERS**
  - **Paper** → Always simulated (safety fallback)

**Console Output:**
```
INFO  Executing request: ExecutionRequest { platform: Kalshi, market_id: "NBAHOU-BOSJAN28", side: Yes, limit_price: 0.65, size: 50.0 }
```

---

### Step 5A: Kalshi Live Execution

**File:** `services/execution_service_rust/src/engine.rs:221-385`

```rust
async fn execute_kalshi(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    let start_time = Utc::now();

    // Check if we have credentials
    if !self.kalshi.has_credentials() {
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some("Kalshi credentials not configured"),
            // ...
        });
    }

    // Map ExecutionSide to Kalshi side string
    let side_str = match request.side {
        ExecutionSide::Yes => "yes",
        ExecutionSide::No => "no",
    };

    // Convert size to contracts (assuming size is in dollars and contracts are $1 each)
    let quantity = request.size.round() as i32;
    if quantity < 1 {
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(format!("Order size too small: {} contracts (minimum 1)", quantity)),
            // ...
        });
    }

    info!(
        "Placing Kalshi IOC order: {} {} x{} @ {:.2} on {}",
        "buy", side_str, quantity, request.limit_price, request.market_id
    );

    // 🚨 REAL ORDER PLACEMENT HERE 🚨
    match self
        .kalshi
        .place_ioc_order(&request.market_id, side_str, request.limit_price, quantity)
        .await
    {
        Ok(order) => {
            // Order placed successfully
            let filled_qty = order.filled_count() as f64;
            let status = if order.is_filled() {
                ExecutionStatus::Filled
            } else if order.is_partial() {
                ExecutionStatus::Partial
            } else {
                ExecutionStatus::Cancelled
            };
            // ...
        }
        Err(e) => {
            error!("Kalshi order failed: {}", e);
            // ...
        }
    }
}
```

**Safeguards:**
✅ **Credential Check** (line 224-234): Rejects if no API key/private key
✅ **Side Validation** (line 256-259): Only "yes" or "no" allowed
✅ **Quantity Validation** (line 262-290): Minimum 1 contract
✅ **Order Type = IOC** (Immediate-or-Cancel): Never rests on book

**Missing Safeguards:**
❌ **No maximum order size check** - Could accidentally place huge order
❌ **No balance check** - Doesn't verify account has funds
❌ **No confirmation prompt** - Places order immediately
❌ **No rate limiting** - Could spam orders in rapid succession
❌ **No sanity check on price** - Could place order at terrible price if signal is wrong

**Console Output:**
```
INFO  Placing Kalshi IOC order: buy yes x50 @ 0.65 on NBAHOU-BOSJAN28
```

---

### Step 5B: Kalshi API Call

**File:** `rust_core/src/clients/kalshi.rs:538-577`

```rust
pub async fn place_ioc_order(
    &self,
    ticker: &str,
    side: &str,
    price: f64,
    quantity: i32,
) -> Result<KalshiOrder> {
    if !self.has_credentials() {
        return Err(anyhow!("Cannot place order: no credentials configured"));
    }

    let side_lower = side.to_lowercase();
    if side_lower != "yes" && side_lower != "no" {
        return Err(anyhow!("Invalid side '{}': must be 'yes' or 'no'", side));
    }

    // Convert price to cents (Kalshi uses integer cents)
    let price_cents = (price * 100.0).round() as i32;
    if price_cents < 1 || price_cents > 99 {
        return Err(anyhow!("Invalid price {}: must be between 0.01 and 0.99", price));
    }

    let client_order_id = Self::generate_order_id();

    // Build IOC order request
    let order_req = KalshiOrderRequest {
        ticker: ticker.to_string(),
        action: "buy".to_string(),
        side: side_lower.clone(),
        order_type: "limit".to_string(),
        count: quantity,
        yes_price: if side_lower == "yes" { Some(price_cents) } else { None },
        no_price: if side_lower == "no" { Some(price_cents) } else { None },
        expiration_ts: None,
        client_order_id: client_order_id.clone(),
        // 🚨 CRITICAL: type = "ioc" 🚨
        r#type: Some("ioc".to_string()),
    };

    // Create authenticated request with RSA signature
    let url = format!("{}/trade-api/v2/portfolio/orders", self.base_url);
    let body = serde_json::to_string(&order_req)?;
    let timestamp = Self::current_timestamp_ms();
    let signature = self.sign_request("POST", "/trade-api/v2/portfolio/orders", &body, timestamp)?;

    // 🚨 MAKES REAL HTTP POST TO KALSHI 🚨
    let response = self
        .client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", &self.api_key.clone().unwrap())
        .header("KALSHI-ACCESS-SIGNATURE", signature)
        .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
        .body(body)
        .send()
        .await?;

    // Parse response
    let order_response: KalshiOrderResponse = response.json().await?;
    Ok(order_response.order)
}
```

**Safeguards:**
✅ **Credential Check** (line 545-547)
✅ **Side Validation** (lines 549-552)
✅ **Price Validation** (lines 554-558): Must be 0.01-0.99
✅ **RSA Signature** (line 590): Authenticated request
✅ **Order Type = IOC** (line 577): `type: "ioc"` (Immediate-or-Cancel)

**What IOC Means:**
- Order attempts to fill immediately at limit price or better
- Any unfilled portion is cancelled (does NOT rest on book)
- **Eliminates one-sided fill risk** in arbitrage trading
- Example: Order for 50 contracts, 30 fill immediately, 20 cancelled

**HTTP Request:**
```
POST https://api.elections.kalshi.com/trade-api/v2/portfolio/orders
Headers:
  Authorization: Bearer <API_KEY>
  KALSHI-ACCESS-SIGNATURE: <RSA_SIGNATURE>
  KALSHI-ACCESS-TIMESTAMP: 1706472600000
Body:
{
  "ticker": "NBAHOU-BOSJAN28",
  "action": "buy",
  "side": "yes",
  "order_type": "limit",
  "count": 50,
  "yes_price": 65,
  "type": "ioc",
  "client_order_id": "arbees_1706472600_abc123"
}
```

**Kalshi API Response:**
```json
{
  "order": {
    "order_id": "kalshi_order_xyz789",
    "ticker": "NBAHOU-BOSJAN28",
    "side": "yes",
    "action": "buy",
    "count": 50,
    "remaining_count": 20,
    "yes_price": 65,
    "status": "partial",
    "created_time": "2026-01-28T20:30:01Z"
  }
}
```

**Console Output:**
```
INFO  Kalshi IOC order kalshi_order_xyz789 status: Partial, filled: 30/50, fees: $1.35
```

---

### Step 6: Polymarket Live Execution

**File:** `services/execution_service_rust/src/polymarket_executor.rs:77-158`

```rust
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    let token_id = request
        .token_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Polymarket order requires token_id"))?;

    let start_time = Utc::now();

    debug!(
        "Executing Polymarket FAK order: token_id={}, side={:?}, price={}, size={}",
        token_id, request.side, request.limit_price, request.size
    );

    // 🚨 REAL ORDER PLACEMENT HERE 🚨
    // Execute FAK order based on side
    let fill = match request.side {
        ExecutionSide::Yes => {
            // Buying YES token
            self.client
                .buy_fak(token_id, request.limit_price, request.size)
                .await?
        }
        ExecutionSide::No => {
            // For NO side, we sell YES token (equivalent to buying NO)
            // Price needs to be inverted: NO price = 1 - YES price
            let no_price = 1.0 - request.limit_price;
            self.client
                .sell_fak(token_id, no_price, request.size)
                .await?
        }
    };

    // Calculate fees (2% Polymarket fee)
    let fees = fill.fill_cost * 0.02;
    let avg_price = if fill.filled_size > 0.0 {
        fill.fill_cost / fill.filled_size
    } else {
        0.0
    };

    let status = if fill.filled_size >= request.size {
        ExecutionStatus::Filled
    } else if fill.filled_size > 0.0 {
        ExecutionStatus::Partial
    } else {
        ExecutionStatus::Rejected
    };

    info!(
        "Polymarket order {}: status={:?}, filled={:.2}/{:.2}, avg_price={:.4}, fees={:.4}",
        fill.order_id, status, fill.filled_size, request.size, avg_price, fees
    );

    Ok(ExecutionResult { /* ... */ })
}
```

**Safeguards:**
✅ **token_id Required** (lines 78-81): Rejects if missing
✅ **Order Type = FAK** (Fill-and-Kill): Similar to IOC, never rests on book
✅ **Price Inversion for NO Side** (line 101): Correctly handles NO = 1 - YES
✅ **Authenticated via API Key** (derived from L1 wallet)

**Missing Safeguards:**
❌ **No maximum order size check**
❌ **No balance check** (doesn't verify USDC balance)
❌ **No confirmation prompt**

**What FAK Means:**
- FAK = Fill-and-Kill (Polymarket's IOC equivalent)
- Order fills immediately or gets rejected
- Does NOT rest on book

**HTTP Request to Polymarket CLOB:**
```
POST https://clob.polymarket.com/order
Headers:
  POLY-ADDRESS: <FUNDER_ADDRESS>
  POLY-SIGNATURE: <API_KEY_SIGNATURE>
  POLY-TIMESTAMP: 1706472600
Body:
{
  "tokenID": "71321045679252212594626385532706912750332728571942532289631379312455583992833",
  "price": 0.65,
  "size": 50.0,
  "side": "BUY",
  "orderType": "FOK"
}
```

**Polymarket CLOB Response:**
```json
{
  "orderID": "poly_order_abc123",
  "transactionHash": "0xabc123...",
  "filledSize": 50.0,
  "fillCost": 32.5,
  "makerOrders": [
    {"orderID": "maker1", "filledSize": 30.0, "price": 0.64},
    {"orderID": "maker2", "filledSize": 20.0, "price": 0.66}
  ]
}
```

**Console Output:**
```
INFO  Polymarket order poly_order_abc123: status=Filled, filled=50.00/50.00, avg_price=0.6500, fees=0.6500, latency=180ms
```

---

### Step 7: Execution Result Published

**File:** `services/execution_service_rust/src/main.rs:352-372`

```rust
// Publish notification on execution failure
if result.status == ExecutionStatus::Failed {
    let event = NotificationEvent {
        event_type: NotificationType::Error,
        priority: NotificationPriority::Error,
        data: serde_json::json!({
            "service": "execution_service_rust",
            "request_id": result.request_id,
            "message": result.rejection_reason.clone().unwrap_or_else(|| "execution_failed".to_string()),
        }),
        ts: Some(Utc::now()),
    };
    if let Err(e) = redis.publish(channels::NOTIFICATION_EVENTS, &event).await {
        warn!("Failed to publish notification event: {}", e);
    }
}

if let Err(e) = redis.publish(channels::EXECUTION_RESULTS, &result).await {
    error!("Failed to publish execution result: {}", e);
}
```

**What Happens:**
1. **If execution failed:** Publishes notification event to `notification:events`
2. **Always:** Publishes execution result to `execution:results`
3. **Position Tracker** subscribes to `execution:results` and updates positions
4. **Database** stores trade in `paper_trades` table (even for live trades)

**ExecutionResult Published:**
```json
{
  "request_id": "req_abc123",
  "idempotency_key": "idempotent_xyz",
  "status": "Filled",
  "rejection_reason": null,
  "order_id": "kalshi_order_xyz789",
  "filled_qty": 30.0,
  "avg_price": 0.65,
  "fees": 1.35,
  "platform": "Kalshi",
  "market_id": "NBAHOU-BOSJAN28",
  "contract_team": "Celtics",
  "game_id": "nba_12345",
  "sport": "NBA",
  "signal_id": "signal_789",
  "signal_type": "ModelEdgeYes",
  "edge_pct": 18.5,
  "side": "Yes",
  "requested_at": "2026-01-28T20:30:00Z",
  "executed_at": "2026-01-28T20:30:01.234Z",
  "latency_ms": 1234.0
}
```

---

## Summary of What Actually Happens

### When PAPER_TRADING=0:

1. **Service starts** with live trading enabled
2. **Credentials loaded** from environment (Kalshi API key/private key, Polymarket wallet)
3. **Signal arrives** via ZMQ or Redis
4. **Execution engine** routes to live execution path
5. **For Kalshi:**
   - Validates credentials, side, price, quantity
   - Creates IOC order request
   - Signs request with RSA private key
   - **Makes real HTTP POST to Kalshi API**
   - Kalshi fills order immediately or cancels unfilled portion
   - Returns fill details
6. **For Polymarket:**
   - Validates token_id is present
   - Creates FAK order request
   - Signs request with derived API key from L1 wallet
   - **Makes real HTTP POST to Polymarket CLOB**
   - CLOB fills order immediately or rejects
   - Returns fill details
7. **Result published** to Redis for position tracking
8. **Trade recorded** in database

---

## Safeguards That EXIST

### ✅ Credential Checks
- Kalshi: Requires `KALSHI_API_KEY` + `KALSHI_PRIVATE_KEY`
- Polymarket: Requires `POLYMARKET_PRIVATE_KEY` + `POLYMARKET_FUNDER_ADDRESS`
- **Rejects orders if credentials missing**

### ✅ Input Validation
- Side must be "yes" or "no"
- Price must be 0.01-0.99 (Kalshi)
- Quantity must be >= 1 contract
- token_id required for Polymarket

### ✅ Safe Order Types
- **Kalshi IOC** (Immediate-or-Cancel): Fills immediately or cancels
- **Polymarket FAK** (Fill-and-Kill): Fills immediately or rejects
- **Never rests on book** → Eliminates one-sided fill risk in arbitrage

### ✅ RSA/API Key Authentication
- Kalshi orders signed with RSA-PSS private key
- Polymarket orders signed with API key derived from L1 wallet
- **Prevents unauthorized trading**

---

## Safeguards That DON'T EXIST

### ❌ No Confirmation Prompt
**Risk:** Code places real orders immediately without asking
**Impact:** If misconfigured, could place hundreds of orders before noticing
**Mitigation:** None currently - relies on operator to set PAPER_TRADING correctly

### ❌ No Dry-Run Mode
**Risk:** No intermediate testing mode between paper and live
**Impact:** First live order is a real order with real money
**Mitigation:** None currently - must test in paper mode first

### ❌ No Maximum Order Size
**Risk:** Could accidentally place huge order (e.g., 1000 contracts = $1000)
**Impact:** Large unexpected loss if order fills at bad price
**Mitigation:** Signal processor has Kelly sizing, but no hard limit

### ❌ No Account Balance Check
**Risk:** Could attempt order larger than account balance
**Impact:** Order rejected by exchange, but wasted latency
**Mitigation:** None currently - relies on exchange validation

### ❌ No Rate Limiting
**Risk:** Could spam orders in rapid succession
**Impact:** Hit exchange rate limits, get temporarily banned
**Mitigation:** None currently - relies on signal generation rate

### ❌ No Price Sanity Check
**Risk:** Could place order at terrible price if signal is wrong
**Impact:** Fill at 0.99 when fair value is 0.50 = instant 49% loss
**Mitigation:** Signal processor filters by MIN_EDGE_PCT, but no absolute price check

### ❌ No Duplicate Order Detection
**Risk:** Same signal could generate multiple orders within seconds
**Impact:** Double exposure on same game
**Mitigation:** Idempotency key exists but not enforced at execution level

---

## Recommended Safety Improvements

### 1. Add Confirmation Prompt (HIGH PRIORITY)
```rust
if !self.paper_trading && !self.live_trading_confirmed {
    return Err(anyhow!(
        "Live trading not confirmed. Set LIVE_TRADING_CONFIRMED=true to enable."
    ));
}
```

### 2. Add Maximum Order Size (HIGH PRIORITY)
```rust
let max_order_size = env::var("MAX_ORDER_SIZE")
    .unwrap_or("100.0".to_string())
    .parse::<f64>()
    .unwrap_or(100.0);

if request.size > max_order_size {
    return Err(anyhow!("Order size {} exceeds maximum {}", request.size, max_order_size));
}
```

### 3. Add Balance Check (MEDIUM PRIORITY)
```rust
let balance = self.kalshi.get_balance().await?;
let required = request.size * request.limit_price;
if balance.available < required {
    return Err(anyhow!(
        "Insufficient balance: have ${:.2}, need ${:.2}",
        balance.available, required
    ));
}
```

### 4. Add Rate Limiting (MEDIUM PRIORITY)
```rust
// Track last order time
if let Some(last_order_time) = self.last_order_time {
    let elapsed = Utc::now() - last_order_time;
    if elapsed.num_milliseconds() < 1000 {
        return Err(anyhow!("Rate limit: wait 1s between orders"));
    }
}
```

### 5. Add Dry-Run Mode (LOW PRIORITY)
```rust
// Add new mode: DRY_RUN=true
// Places real order but immediately cancels it
// Validates credentials and connectivity without risk
```

---

## Testing Checklist Before Going Live

### Step 1: Paper Trading Validation (1 Week Minimum)
- [ ] Run `PAPER_TRADING=1` for at least 1 week
- [ ] Verify 100+ paper trades executed
- [ ] Confirm win rate >55%
- [ ] Confirm edge realization >50%
- [ ] Verify no service crashes or errors

### Step 2: Credential Setup
- [ ] Generate Kalshi API key in production account
- [ ] Generate Kalshi RSA private key and store securely
- [ ] Set `KALSHI_API_KEY` in production `.env`
- [ ] Set `KALSHI_PRIVATE_KEY` or `KALSHI_PRIVATE_KEY_PATH`
- [ ] Set `KALSHI_ENV=prod` (not demo)
- [ ] Fund Polymarket wallet with USDC (if using Polymarket)
- [ ] Set `POLYMARKET_PRIVATE_KEY` (L1 wallet private key)
- [ ] Set `POLYMARKET_FUNDER_ADDRESS` (proxy wallet address)

### Step 3: Risk Limit Configuration
- [ ] Set `MAX_DAILY_LOSS=100.0` (start conservative)
- [ ] Set `MAX_GAME_EXPOSURE=25.0` (1/4 of paper limit)
- [ ] Set `MAX_SPORT_EXPOSURE=100.0` (1/2 of paper limit)
- [ ] Set `MIN_EDGE_PCT=20.0` (higher threshold initially)

### Step 4: Safety Checks
- [ ] Verify `PAPER_TRADING=1` in `.env` (DO NOT SET TO 0 YET)
- [ ] Test Kalshi authentication: `cargo test --package arbees_rust_core test_kalshi_auth`
- [ ] Verify service logs show "Paper Trading: true"
- [ ] Run one full day with paper trading to confirm stability

### Step 5: Live Trading (After All Above Complete)
- [ ] **BACKUP EVERYTHING** (code, database, configuration)
- [ ] Create git tag: `git tag v1.0.0-live-trading`
- [ ] Set `PAPER_TRADING=0` in `.env`
- [ ] Restart execution_service_rust: `docker compose restart execution_service_rust`
- [ ] **WATCH LOGS CONTINUOUSLY FOR FIRST HOUR**
- [ ] Verify console shows "Paper Trading: false, Kalshi Live: true"
- [ ] Wait for first signal
- [ ] **MANUALLY VERIFY ORDER ON KALSHI WEBSITE BEFORE SECOND ORDER**
- [ ] If anything looks wrong, immediately set `PAPER_TRADING=1` and restart

### Step 6: First Day Monitoring
- [ ] Check every trade on Kalshi/Polymarket website
- [ ] Verify fills match expected prices
- [ ] Verify fees calculated correctly
- [ ] Monitor account balance changes
- [ ] Check for any unexpected behavior

---

## Emergency Stop Procedure

**If anything goes wrong:**

1. **Immediately set** `PAPER_TRADING=1` in `.env`
2. **Restart service:** `docker compose restart execution_service_rust`
3. **Verify in logs:** "Paper Trading: true"
4. **Cancel any open orders** on Kalshi/Polymarket website
5. **Review what went wrong** before re-enabling live trading

---

## Key Takeaways

### ✅ What Works Well:
- Credential-based authentication prevents unauthorized trading
- IOC/FAK order types eliminate one-sided fill risk
- Input validation prevents obviously invalid orders
- RSA signatures prevent API key theft attacks

### ⚠️ What to Watch Out For:
- No confirmation prompt - will place real orders immediately
- No maximum order size - could place large orders accidentally
- No balance checks - relies on exchange rejection
- No rate limiting - could spam orders if signal generator misbehaves

### 💡 Best Practice:
1. **Always test in paper trading first** (minimum 1 week, 100+ trades)
2. **Start with low limits** (MAX_DAILY_LOSS=100, MIN_EDGE_PCT=20)
3. **Monitor closely for first day** (watch every single trade)
4. **Gradually scale up** (increase limits weekly as confidence builds)
5. **Have emergency stop ready** (know how to quickly revert to paper trading)

---

**CRITICAL REMINDER:**

When `PAPER_TRADING=0`, the system **WILL PLACE REAL ORDERS WITH REAL MONEY** without asking for confirmation. Make absolutely certain you're ready before flipping this switch.

**Test thoroughly. Start small. Monitor closely. Scale gradually.**

---

*Document Generated: 2026-01-28*
*Code Traced: execution_service_rust + arbees_rust_core*
*Status: CRITICAL - REVIEW BEFORE GOING LIVE*
