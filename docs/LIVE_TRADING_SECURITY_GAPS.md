# Live Trading Security Gaps & Required Safeguards

**Critical Security Analysis for Production Trading**

Date: 2026-01-28
Priority: CRITICAL - Must Address Before Live Trading
Status: 7 Critical Gaps, 5 High Priority Gaps, 4 Medium Priority Gaps

---

## Executive Summary

The Arbees trading system has **excellent foundation safeguards** (credential validation, safe order types, RSA authentication) but is **missing critical production-grade safety mechanisms** that are standard in financial trading systems.

**Current State:** Safe for paper trading ✅
**Ready for live trading:** ❌ Not without additional safeguards

**Risk Level if deployed as-is:** HIGH
- No confirmation prompt → Could place hundreds of orders before noticing misconfiguration
- No order size limits → Could place $10,000 order when intending $100
- No balance checks → Could overdraw account, incur margin calls
- No kill switch → No way to emergency stop all trading

---

## Critical Gaps (Must Fix Before Live Trading)

### 1. ❌ No Trading Authorization Confirmation

**Current Behavior:**
```rust
// main.rs:55-57
let paper_trading_val = env::var("PAPER_TRADING").unwrap_or_else(|_| "1".to_string());
let paper_trading = matches!(paper_trading_val.to_lowercase().as_str(), "1" | "true" | "yes");
let engine = ExecutionEngine::new(paper_trading).await;

// If PAPER_TRADING=0, immediately starts placing real orders
```

**Risk:**
- Operator sets `PAPER_TRADING=0` intending to test, service places real orders
- Typo in `.env` file (e.g., `PAPER_TRADIN=0`) → defaults to live trading
- Service restart after config change → immediate live trading without confirmation

**Impact:** Could place dozens of real orders before operator notices

**Required Fix:**

**Option A: Dual-Flag Authorization (Recommended)**
```rust
// Require TWO environment variables to enable live trading
let paper_trading_off = env::var("PAPER_TRADING")
    .unwrap_or("1".to_string())
    .to_lowercase() == "0";

let live_trading_authorized = env::var("LIVE_TRADING_AUTHORIZED")
    .unwrap_or("false".to_string())
    .to_lowercase() == "true";

if paper_trading_off && !live_trading_authorized {
    error!("CRITICAL: PAPER_TRADING=0 but LIVE_TRADING_AUTHORIZED not set");
    error!("Live trading is DISABLED for safety");
    error!("To enable live trading, set LIVE_TRADING_AUTHORIZED=true in .env");
    std::process::exit(1);
}

let paper_trading = !paper_trading_off;
```

**Configuration:**
```bash
# .env
PAPER_TRADING=0                    # Disable paper trading
LIVE_TRADING_AUTHORIZED=true       # Explicitly authorize live trading
```

**Benefits:**
- Requires TWO intentional changes to enable live trading
- Prevents accidental live trading from single typo
- Self-documenting: config clearly shows live trading is authorized

**Option B: Startup Confirmation Prompt**
```rust
if !paper_trading {
    println!("\n⚠️  WARNING: LIVE TRADING MODE ⚠️");
    println!("Real orders will be placed with real money.");
    println!("Kalshi Live: {}", engine.kalshi_live_enabled());
    println!("Polymarket Live: {}", engine.polymarket_live_enabled());
    println!("\nType 'CONFIRM LIVE TRADING' to continue, or Ctrl+C to abort:");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).expect("Failed to read input");

    if input.trim() != "CONFIRM LIVE TRADING" {
        error!("Live trading not confirmed. Exiting.");
        std::process::exit(1);
    }

    println!("✅ Live trading confirmed. Starting execution service...\n");
}
```

**Benefits:**
- Interactive confirmation prevents unattended startup
- Forces operator to acknowledge live trading
- Clear visual warning

**Drawback:**
- Incompatible with automated restarts (Docker, systemd)
- Use Option A for production deployment

**Files to Modify:**
- `services/execution_service_rust/src/main.rs` (lines 55-57)

---

### 2. ❌ No Maximum Order Size Limit

**Current Behavior:**
```rust
// engine.rs:262-290
let quantity = request.size.round() as i32;
if quantity < 1 {
    // Reject order
}
// No upper bound check! Could place 1000+ contract order
```

**Risk:**
- Signal processor bug generates $10,000 order when intending $100
- Kelly sizing calculation error → oversized position
- Decimal point error (500.0 vs 50.0) → 10x intended size

**Impact:** Single bad order could wipe out account

**Current Example:**
```rust
// Signal processor calculates:
let kelly_bet = 0.25 * 1000.0;  // Intended: $250
// But if bankroll is wrong:
let kelly_bet = 0.25 * 10000.0;  // Actual: $2500 (10x)
```

**Required Fix:**

```rust
// Add to Config
pub struct Config {
    pub max_order_size: f64,           // Maximum order size in dollars
    pub max_order_contracts: i32,      // Maximum contracts per order
    pub max_position_per_market: f64,  // Maximum exposure per market
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            max_order_size: env::var("MAX_ORDER_SIZE")
                .unwrap_or("100.0".to_string())
                .parse()
                .unwrap_or(100.0),
            max_order_contracts: env::var("MAX_ORDER_CONTRACTS")
                .unwrap_or("100".to_string())
                .parse()
                .unwrap_or(100),
            max_position_per_market: env::var("MAX_POSITION_PER_MARKET")
                .unwrap_or("200.0".to_string())
                .parse()
                .unwrap_or(200.0),
        }
    }
}

// In ExecutionEngine
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // Size validation
    let order_value = request.size * request.limit_price;
    if order_value > self.config.max_order_size {
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(format!(
                "Order size ${:.2} exceeds maximum ${:.2}",
                order_value, self.config.max_order_size
            )),
            // ...
        });
    }

    let quantity = request.size.round() as i32;
    if quantity > self.config.max_order_contracts {
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(format!(
                "Order quantity {} exceeds maximum {}",
                quantity, self.config.max_order_contracts
            )),
            // ...
        });
    }

    // Check current position + new order doesn't exceed market limit
    let current_position = self.get_current_position(&request.market_id).await?;
    let total_exposure = current_position.abs() + order_value;
    if total_exposure > self.config.max_position_per_market {
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(format!(
                "Total market exposure ${:.2} would exceed maximum ${:.2}",
                total_exposure, self.config.max_position_per_market
            )),
            // ...
        });
    }

    // Proceed with execution...
}
```

**Configuration:**
```bash
# .env
MAX_ORDER_SIZE=100.0              # Max $100 per order (start conservative)
MAX_ORDER_CONTRACTS=100           # Max 100 contracts per order
MAX_POSITION_PER_MARKET=200.0     # Max $200 exposure per market
```

**Recommended Limits:**
- **Day 1:** MAX_ORDER_SIZE=50, MAX_POSITION_PER_MARKET=100
- **Week 1:** MAX_ORDER_SIZE=100, MAX_POSITION_PER_MARKET=200
- **Week 2:** MAX_ORDER_SIZE=200, MAX_POSITION_PER_MARKET=500
- **Month 1:** MAX_ORDER_SIZE=500, MAX_POSITION_PER_MARKET=1000

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs` (lines 111-149, 221-290)
- `services/execution_service_rust/src/main.rs` (add config)

---

### 3. ❌ No Account Balance Validation

**Current Behavior:**
```rust
// engine.rs:293-296
info!("Placing Kalshi IOC order: {} {} x{} @ {:.2} on {}", ...);

// Immediately places order without checking account balance
match self.kalshi.place_ioc_order(...).await { ... }
```

**Risk:**
- Order size exceeds available balance → rejected by exchange (wasted latency)
- Repeated rejections → exchange rate limiting/account suspension
- Margin call if exchange allows overdraft

**Impact:** Degraded execution, potential account suspension

**Required Fix:**

**Add Balance Tracking:**
```rust
pub struct ExecutionEngine {
    kalshi: KalshiClient,
    polymarket_executor: Option<PolymarketExecutor>,
    balance_cache: Arc<RwLock<BalanceCache>>,
    paper_trading: bool,
}

#[derive(Debug, Clone)]
pub struct BalanceCache {
    pub kalshi_balance: f64,
    pub polymarket_balance: f64,
    pub last_updated: DateTime<Utc>,
    pub ttl_seconds: u64,  // Time-to-live for cache
}

impl ExecutionEngine {
    pub async fn new(paper_trading: bool) -> Self {
        let balance_cache = Arc::new(RwLock::new(BalanceCache {
            kalshi_balance: 0.0,
            polymarket_balance: 0.0,
            last_updated: Utc::now(),
            ttl_seconds: 60,  // Refresh every 60 seconds
        }));

        // Start background balance refresh task
        if !paper_trading {
            let cache_clone = balance_cache.clone();
            let kalshi_clone = kalshi.clone();
            tokio::spawn(async move {
                Self::balance_refresh_loop(cache_clone, kalshi_clone).await;
            });
        }

        Self {
            kalshi,
            polymarket_executor,
            balance_cache,
            paper_trading,
        }
    }

    async fn balance_refresh_loop(
        cache: Arc<RwLock<BalanceCache>>,
        kalshi: KalshiClient,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;

            // Fetch latest balance from Kalshi
            if let Ok(balance) = kalshi.get_balance().await {
                let mut cache = cache.write().await;
                cache.kalshi_balance = balance.available;
                cache.last_updated = Utc::now();
                info!("Balance updated: Kalshi ${:.2}", balance.available);
            }
        }
    }

    async fn validate_balance(&self, request: &ExecutionRequest) -> Result<()> {
        if self.paper_trading {
            return Ok(());  // Skip for paper trading
        }

        let cache = self.balance_cache.read().await;

        // Calculate required balance
        let required = request.size * request.limit_price;
        let buffer = required * 0.1;  // 10% buffer for fees
        let required_with_buffer = required + buffer;

        let available = match request.platform {
            Platform::Kalshi => cache.kalshi_balance,
            Platform::Polymarket => cache.polymarket_balance,
            _ => return Ok(()),
        };

        if available < required_with_buffer {
            return Err(anyhow!(
                "Insufficient balance: have ${:.2}, need ${:.2} (including 10% fee buffer)",
                available, required_with_buffer
            ));
        }

        Ok(())
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        // Balance check before execution
        if let Err(e) = self.validate_balance(&request).await {
            warn!("Order rejected due to balance: {}", e);
            return Ok(ExecutionResult {
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(e.to_string()),
                // ...
            });
        }

        // Proceed with execution...
    }
}
```

**Add Kalshi Balance API:**
```rust
// rust_core/src/clients/kalshi.rs
pub async fn get_balance(&self) -> Result<KalshiBalance> {
    if !self.has_credentials() {
        return Err(anyhow!("Cannot fetch balance: no credentials"));
    }

    let url = format!("{}/trade-api/v2/portfolio/balance", self.base_url);
    let timestamp = Self::current_timestamp_ms();
    let signature = self.sign_request("GET", "/trade-api/v2/portfolio/balance", "", timestamp)?;

    let response = self.client
        .get(&url)
        .header("Authorization", &self.api_key.clone().unwrap())
        .header("KALSHI-ACCESS-SIGNATURE", signature)
        .header("KALSHI-ACCESS-TIMESTAMP", timestamp.to_string())
        .send()
        .await?;

    #[derive(Debug, Deserialize)]
    struct BalanceResponse {
        balance: KalshiBalance,
    }

    let response: BalanceResponse = response.json().await?;
    Ok(response.balance)
}

#[derive(Debug, Deserialize, Clone)]
pub struct KalshiBalance {
    pub available: f64,
    pub total: f64,
    pub reserved: f64,
}
```

**Benefits:**
- Prevents orders that will be rejected
- Reduces wasted latency
- Avoids exchange rate limiting
- Provides early warning when balance low

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`
- `rust_core/src/clients/kalshi.rs` (add get_balance())

---

### 4. ❌ No Rate Limiting / Order Throttling

**Current Behavior:**
```rust
// No rate limiting at all!
// If signal_processor generates 100 signals/second, execution service will attempt 100 orders/second
```

**Risk:**
- Signal processor bug → spam orders rapidly
- Exchange rate limits exceeded → account suspension
- Multiple shards generate signals for same game → duplicate orders

**Impact:** Account suspension, wasted execution attempts, duplicate positions

**Required Fix:**

```rust
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct RateLimiter {
    recent_orders: Arc<RwLock<VecDeque<DateTime<Utc>>>>,
    max_per_minute: usize,
    max_per_hour: usize,
}

impl RateLimiter {
    pub fn new(max_per_minute: usize, max_per_hour: usize) -> Self {
        Self {
            recent_orders: Arc::new(RwLock::new(VecDeque::new())),
            max_per_minute,
            max_per_hour,
        }
    }

    pub async fn check_and_record(&self) -> Result<()> {
        let mut orders = self.recent_orders.write().await;
        let now = Utc::now();

        // Remove orders older than 1 hour
        while let Some(oldest) = orders.front() {
            if (now - *oldest).num_seconds() > 3600 {
                orders.pop_front();
            } else {
                break;
            }
        }

        // Count recent orders
        let last_minute = orders.iter()
            .filter(|t| (now - **t).num_seconds() < 60)
            .count();
        let last_hour = orders.len();

        // Check limits
        if last_minute >= self.max_per_minute {
            return Err(anyhow!(
                "Rate limit exceeded: {} orders in last minute (max {})",
                last_minute, self.max_per_minute
            ));
        }

        if last_hour >= self.max_per_hour {
            return Err(anyhow!(
                "Rate limit exceeded: {} orders in last hour (max {})",
                last_hour, self.max_per_hour
            ));
        }

        // Record this order
        orders.push_back(now);
        Ok(())
    }
}

pub struct ExecutionEngine {
    kalshi: KalshiClient,
    rate_limiter: RateLimiter,
    // ...
}

impl ExecutionEngine {
    pub async fn new(paper_trading: bool) -> Self {
        let rate_limiter = RateLimiter::new(
            env::var("MAX_ORDERS_PER_MINUTE").unwrap_or("20".to_string()).parse().unwrap_or(20),
            env::var("MAX_ORDERS_PER_HOUR").unwrap_or("100".to_string()).parse().unwrap_or(100),
        );

        Self {
            kalshi,
            rate_limiter,
            // ...
        }
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        // Rate limit check
        if let Err(e) = self.rate_limiter.check_and_record().await {
            warn!("Order rate limited: {}", e);
            return Ok(ExecutionResult {
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(format!("Rate limit: {}", e)),
                // ...
            });
        }

        // Proceed with execution...
    }
}
```

**Configuration:**
```bash
# .env
MAX_ORDERS_PER_MINUTE=20          # Max 20 orders per minute
MAX_ORDERS_PER_HOUR=100           # Max 100 orders per hour
```

**Recommended Limits:**
- **Day 1:** 10/minute, 50/hour (very conservative)
- **Week 1:** 20/minute, 100/hour (normal)
- **Production:** 30/minute, 200/hour (aggressive)

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

### 5. ❌ No Emergency Kill Switch

**Current Behavior:**
```rust
// No way to stop all trading without restarting service or changing .env
```

**Risk:**
- Model malfunction → need to stop trading immediately
- Account balance dropping rapidly → need emergency halt
- Exchange outage → want to pause until resolved

**Impact:** Can't stop bleeding without service restart (slow, risky)

**Required Fix:**

**Option A: Redis Kill Switch**
```rust
pub struct ExecutionEngine {
    redis: Arc<RedisBus>,
    kill_switch_enabled: Arc<AtomicBool>,
    // ...
}

impl ExecutionEngine {
    pub async fn new(paper_trading: bool) -> Self {
        let kill_switch = Arc::new(AtomicBool::new(false));

        // Start background kill switch monitor
        let redis_clone = redis.clone();
        let kill_switch_clone = kill_switch.clone();
        tokio::spawn(async move {
            Self::kill_switch_monitor(redis_clone, kill_switch_clone).await;
        });

        Self {
            redis,
            kill_switch_enabled: kill_switch,
            // ...
        }
    }

    async fn kill_switch_monitor(redis: Arc<RedisBus>, kill_switch: Arc<AtomicBool>) {
        let mut pubsub = redis.subscribe("trading:kill_switch").await.unwrap();
        let mut stream = pubsub.on_message();

        while let Some(msg) = stream.next().await {
            if let Ok(command) = msg.get_payload::<String>() {
                match command.as_str() {
                    "ENABLE" => {
                        error!("🚨 KILL SWITCH ACTIVATED - ALL TRADING STOPPED 🚨");
                        kill_switch.store(true, Ordering::SeqCst);
                    }
                    "DISABLE" => {
                        warn!("✅ Kill switch deactivated - trading resumed");
                        kill_switch.store(false, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        }
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        // Check kill switch
        if self.kill_switch_enabled.load(Ordering::SeqCst) {
            warn!("Order rejected: kill switch enabled");
            return Ok(ExecutionResult {
                status: ExecutionStatus::Rejected,
                rejection_reason: Some("Trading halted: kill switch active".to_string()),
                // ...
            });
        }

        // Proceed with execution...
    }
}
```

**Usage:**
```bash
# Activate kill switch (stops all trading immediately)
redis-cli PUBLISH trading:kill_switch ENABLE

# Deactivate kill switch (resume trading)
redis-cli PUBLISH trading:kill_switch DISABLE

# Check status
redis-cli GET trading:kill_switch_status
```

**Option B: File-Based Kill Switch**
```rust
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // Check for kill switch file
    if std::path::Path::new("/tmp/arbees_kill_switch").exists() {
        warn!("Order rejected: kill switch file exists");
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some("Trading halted: kill switch file present".to_string()),
            // ...
        });
    }

    // Proceed with execution...
}
```

**Usage:**
```bash
# Activate kill switch
touch /tmp/arbees_kill_switch

# Deactivate kill switch
rm /tmp/arbees_kill_switch
```

**Benefits:**
- Instant trading halt (no service restart)
- Can be triggered from monitoring scripts
- Reversible without code changes

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

### 6. ❌ No Idempotency Enforcement

**Current Behavior:**
```rust
// ExecutionRequest has idempotency_key field but it's not enforced!
pub struct ExecutionRequest {
    pub idempotency_key: String,  // Present but unused
    // ...
}
```

**Risk:**
- Duplicate signals from multiple shards → duplicate orders
- Signal processor restarts → replays recent signals
- Network retry → same order placed twice

**Impact:** Unintended double position (2x exposure)

**Required Fix:**

```rust
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ExecutionEngine {
    recent_idempotency_keys: Arc<RwLock<HashSet<String>>>,
    // ...
}

impl ExecutionEngine {
    pub async fn new(paper_trading: bool) -> Self {
        let recent_keys = Arc::new(RwLock::new(HashSet::new()));

        // Start background cleanup task (remove keys older than 5 minutes)
        let keys_clone = recent_keys.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let mut keys = keys_clone.write().await;
                // In production, track timestamps and remove old keys
                // For now, clear all after 5 minutes (simple but works)
                if keys.len() > 1000 {
                    keys.clear();
                }
            }
        });

        Self {
            recent_idempotency_keys: recent_keys,
            // ...
        }
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        // Check idempotency
        let mut keys = self.recent_idempotency_keys.write().await;
        if keys.contains(&request.idempotency_key) {
            warn!(
                "Duplicate execution request detected: idempotency_key={}",
                request.idempotency_key
            );
            return Ok(ExecutionResult {
                status: ExecutionStatus::Rejected,
                rejection_reason: Some(format!(
                    "Duplicate request: idempotency_key {} already processed",
                    request.idempotency_key
                )),
                // ...
            });
        }

        // Record this key
        keys.insert(request.idempotency_key.clone());
        drop(keys);  // Release lock before executing

        // Proceed with execution...
    }
}
```

**Benefits:**
- Prevents duplicate orders within 5-minute window
- No database queries needed (in-memory)
- Automatic cleanup of old keys

**Alternative: Database-Based Idempotency**
```sql
-- Add table to track processed requests
CREATE TABLE execution_idempotency (
    idempotency_key VARCHAR(128) PRIMARY KEY,
    request_id VARCHAR(128) NOT NULL,
    status VARCHAR(32) NOT NULL,
    processed_at TIMESTAMPTZ DEFAULT NOW()
);

-- Index for cleanup
CREATE INDEX idx_execution_idempotency_processed_at
ON execution_idempotency (processed_at);

-- Auto-delete after 24 hours
SELECT add_retention_policy('execution_idempotency', INTERVAL '24 hours');
```

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

### 7. ❌ No Detailed Audit Logging

**Current Behavior:**
```rust
// Only basic logging
info!("Executing request: {:?}", request);
info!("Kalshi IOC order {} status: {:?}", order.order_id, status);
```

**Risk:**
- Can't reconstruct what happened if dispute arises
- Can't prove execution price to exchange
- Can't debug why certain orders were placed

**Impact:** No audit trail for compliance, debugging, or disputes

**Required Fix:**

**Add Structured Audit Log:**
```rust
use serde_json::json;

#[derive(Debug, Serialize)]
pub struct AuditLogEntry {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub service: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub platform: Platform,
    pub market_id: String,
    pub side: ExecutionSide,
    pub price: f64,
    pub size: f64,
    pub status: ExecutionStatus,
    pub order_id: Option<String>,
    pub rejection_reason: Option<String>,
    pub latency_ms: f64,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub enum AuditEventType {
    ExecutionRequested,
    OrderPlaced,
    OrderFilled,
    OrderPartialFill,
    OrderCancelled,
    OrderRejected,
    OrderFailed,
}

impl ExecutionEngine {
    async fn audit_log(&self, entry: AuditLogEntry) {
        // Log to console (structured JSON)
        info!(
            target: "audit",
            "{}",
            serde_json::to_string(&entry).unwrap()
        );

        // Also publish to Redis for centralized audit log collection
        if let Err(e) = self.redis.publish("audit:execution", &entry).await {
            error!("Failed to publish audit log: {}", e);
        }

        // Optional: Write to dedicated audit log file
        // let file = OpenOptions::new()
        //     .create(true)
        //     .append(true)
        //     .open("/var/log/arbees/execution_audit.jsonl")
        //     .unwrap();
        // serde_json::to_writer(&file, &entry).unwrap();
    }

    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        let start_time = Utc::now();

        // Audit: execution requested
        self.audit_log(AuditLogEntry {
            timestamp: start_time,
            event_type: AuditEventType::ExecutionRequested,
            service: "execution_service_rust".to_string(),
            request_id: request.request_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            platform: request.platform,
            market_id: request.market_id.clone(),
            side: request.side,
            price: request.limit_price,
            size: request.size,
            status: ExecutionStatus::Pending,
            order_id: None,
            rejection_reason: None,
            latency_ms: 0.0,
            metadata: json!({
                "game_id": request.game_id,
                "sport": request.sport,
                "signal_id": request.signal_id,
                "signal_type": request.signal_type,
                "edge_pct": request.edge_pct,
            }),
        }).await;

        // Execute order...
        let result = self.execute_kalshi(request.clone()).await?;

        // Audit: order result
        self.audit_log(AuditLogEntry {
            timestamp: Utc::now(),
            event_type: match result.status {
                ExecutionStatus::Filled => AuditEventType::OrderFilled,
                ExecutionStatus::Partial => AuditEventType::OrderPartialFill,
                ExecutionStatus::Cancelled => AuditEventType::OrderCancelled,
                ExecutionStatus::Rejected => AuditEventType::OrderRejected,
                ExecutionStatus::Failed => AuditEventType::OrderFailed,
                _ => AuditEventType::OrderPlaced,
            },
            service: "execution_service_rust".to_string(),
            request_id: result.request_id.clone(),
            idempotency_key: result.idempotency_key.clone(),
            platform: result.platform,
            market_id: result.market_id.clone(),
            side: result.side,
            price: result.avg_price,
            size: result.filled_qty,
            status: result.status,
            order_id: result.order_id.clone(),
            rejection_reason: result.rejection_reason.clone(),
            latency_ms: result.latency_ms,
            metadata: json!({
                "fees": result.fees,
                "requested_price": request.limit_price,
                "requested_size": request.size,
            }),
        }).await;

        Ok(result)
    }
}
```

**Audit Log Output:**
```json
{
  "timestamp": "2026-01-28T20:30:00.123Z",
  "event_type": "ExecutionRequested",
  "service": "execution_service_rust",
  "request_id": "req_abc123",
  "idempotency_key": "idempotent_xyz",
  "platform": "Kalshi",
  "market_id": "NBAHOU-BOSJAN28",
  "side": "Yes",
  "price": 0.65,
  "size": 50.0,
  "status": "Pending",
  "order_id": null,
  "rejection_reason": null,
  "latency_ms": 0.0,
  "metadata": {
    "game_id": "nba_12345",
    "sport": "NBA",
    "signal_id": "signal_789",
    "signal_type": "ModelEdgeYes",
    "edge_pct": 18.5
  }
}
```

**Benefits:**
- Complete audit trail for every order
- Structured JSON for easy parsing
- Centralized via Redis for aggregation
- Compliance-ready logs

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

## High Priority Gaps

### 8. ⚠️ No Price Sanity Checks

**Current Behavior:**
```rust
// Kalshi validates price 0.01-0.99, but no sanity check vs fair value
let price_cents = (price * 100.0).round() as i32;
if price_cents < 1 || price_cents > 99 {
    return Err(anyhow!("Invalid price"));
}
```

**Risk:**
- Model bug generates 0.95 price when fair value is 0.50
- Fill at bad price = instant 45% loss
- Signal processor edge calculation wrong → place bad order

**Impact:** Large instant losses from bad prices

**Required Fix:**

```rust
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // Price sanity check: reject if price is extreme
    if request.limit_price < 0.05 || request.limit_price > 0.95 {
        warn!(
            "Order rejected: extreme price {:.3} (likely model error)",
            request.limit_price
        );
        return Ok(ExecutionResult {
            status: ExecutionStatus::Rejected,
            rejection_reason: Some(format!(
                "Price {:.3} outside safe range [0.05, 0.95]",
                request.limit_price
            )),
            // ...
        });
    }

    // Optional: Check price vs pregame probability (if available)
    if let Some(pregame_prob) = request.pregame_prob {
        let price_diff = (request.limit_price - pregame_prob).abs();
        if price_diff > 0.30 {
            warn!(
                "Order price {:.3} differs from pregame {:.3} by {:.3} (>30% threshold)",
                request.limit_price, pregame_prob, price_diff
            );
            // Could reject or just warn depending on policy
        }
    }

    // Proceed with execution...
}
```

**Configuration:**
```bash
# .env
MIN_SAFE_PRICE=0.05               # Reject prices <5%
MAX_SAFE_PRICE=0.95               # Reject prices >95%
MAX_PRICE_DELTA_FROM_PREGAME=0.30 # Warn if price moves >30% from pregame
```

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

### 9. ⚠️ No Real-Time Balance Monitoring

**Current Behavior:**
- Balance only checked before each order
- No monitoring of P&L vs limits
- No alerts when approaching loss limits

**Risk:**
- Daily loss limit exceeded before operator notices
- Account balance dropping rapidly without alerts

**Impact:** Unexpected losses, breach of risk limits

**Required Fix:**

**Add Balance Monitor Service:**
```rust
// In execution_service_rust/src/main.rs
async fn balance_monitoring_loop(engine: Arc<ExecutionEngine>, redis: Arc<RedisBus>) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    let max_daily_loss = env::var("MAX_DAILY_LOSS")
        .unwrap_or("500.0".to_string())
        .parse::<f64>()
        .unwrap_or(500.0);

    loop {
        interval.tick().await;

        // Fetch current balance
        if let Ok(balance) = engine.kalshi.get_balance().await {
            // Check vs daily loss limit
            let pnl_today = balance.total - balance.starting_balance_today;

            if pnl_today < -max_daily_loss {
                // Critical alert: daily loss limit exceeded
                let alert = NotificationEvent {
                    event_type: NotificationType::Critical,
                    priority: NotificationPriority::Critical,
                    data: json!({
                        "service": "execution_service_rust",
                        "alert": "DAILY_LOSS_LIMIT_EXCEEDED",
                        "pnl_today": pnl_today,
                        "max_daily_loss": max_daily_loss,
                        "current_balance": balance.available,
                    }),
                    ts: Some(Utc::now()),
                };
                redis.publish(channels::NOTIFICATION_EVENTS, &alert).await.ok();

                // Activate kill switch
                redis.publish_str("trading:kill_switch", "ENABLE").await.ok();
                error!("🚨 DAILY LOSS LIMIT EXCEEDED: ${:.2} / ${:.2}", pnl_today.abs(), max_daily_loss);
            } else if pnl_today < -max_daily_loss * 0.8 {
                // Warning: 80% of daily loss limit
                warn!("⚠️  Approaching daily loss limit: ${:.2} / ${:.2} (80%)", pnl_today.abs(), max_daily_loss);
                // Send notification but don't halt trading
            }
        }
    }
}
```

**Files to Modify:**
- `services/execution_service_rust/src/main.rs` (add monitoring loop)

---

### 10. ⚠️ No Order Confirmation Receipts

**Current Behavior:**
- Orders placed but no verification order was accepted by exchange
- If API call succeeds but order fails to reach exchange, no detection

**Risk:**
- Order placed successfully but not filled → think we have position when we don't
- Network error after API call → don't know if order placed

**Impact:** Position tracking mismatch with reality

**Required Fix:**

**Add Order Status Polling:**
```rust
async fn verify_order_placement(
    &self,
    platform: Platform,
    order_id: &str,
) -> Result<bool> {
    match platform {
        Platform::Kalshi => {
            // Poll Kalshi order status
            let order = self.kalshi.get_order(order_id).await?;
            Ok(order.status != "failed")
        }
        Platform::Polymarket => {
            // Poll Polymarket order status
            // (Implementation depends on CLOB API)
            Ok(true)
        }
        _ => Ok(true),
    }
}

pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // Place order...
    let result = self.execute_kalshi(request).await?;

    // Verify order was actually placed
    if let Some(order_id) = &result.order_id {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match self.verify_order_placement(result.platform, order_id).await {
            Ok(true) => {
                info!("Order {} verified on exchange", order_id);
            }
            Ok(false) => {
                error!("Order {} not found on exchange (verification failed)", order_id);
                // Update result status
                result.status = ExecutionStatus::Failed;
                result.rejection_reason = Some("Order not confirmed by exchange".to_string());
            }
            Err(e) => {
                warn!("Could not verify order {}: {}", order_id, e);
                // Continue anyway but log warning
            }
        }
    }

    Ok(result)
}
```

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`
- `rust_core/src/clients/kalshi.rs` (add get_order())

---

### 11. ⚠️ No Position Reconciliation

**Current Behavior:**
- Position tracking based on ExecutionResult messages
- If message lost or service restart, positions could be wrong

**Risk:**
- Actual position on exchange != position tracker thinks we have
- Could place orders thinking we have room when already at limit

**Impact:** Incorrect risk exposure

**Required Fix:**

**Add Daily Position Reconciliation:**
```rust
// New service or periodic task
async fn reconcile_positions_with_exchange() -> Result<()> {
    // 1. Fetch actual positions from Kalshi
    let kalshi_positions = kalshi.get_portfolio().await?;

    // 2. Fetch expected positions from database
    let db_positions = sqlx::query!(
        "SELECT market_id, SUM(filled_qty) as net_position
         FROM paper_trades
         WHERE status = 'open'
         GROUP BY market_id"
    )
    .fetch_all(&db)
    .await?;

    // 3. Compare and flag mismatches
    for kalshi_pos in kalshi_positions {
        let db_pos = db_positions.iter()
            .find(|p| p.market_id == kalshi_pos.market_id)
            .map(|p| p.net_position.unwrap_or(0.0))
            .unwrap_or(0.0);

        if (kalshi_pos.position - db_pos).abs() > 0.01 {
            error!(
                "Position mismatch: market {} has {} on Kalshi but {} in DB",
                kalshi_pos.market_id, kalshi_pos.position, db_pos
            );
            // Send alert
            // Auto-reconcile or require manual intervention
        }
    }

    Ok(())
}
```

**Run Schedule:**
- Every 1 hour during trading hours
- After every service restart
- On demand via admin command

**Files to Add:**
- `services/position_reconciliation/` (new service)

---

### 12. ⚠️ No Graceful Degradation

**Current Behavior:**
- If Kalshi API is down, all orders fail
- No fallback to Polymarket only
- No "pause and retry" mechanism

**Risk:**
- Exchange outage → can't trade at all
- Could miss opportunities during temporary outage

**Impact:** Lost trading opportunities

**Required Fix:**

```rust
pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
    // Try primary platform
    let result = match request.platform {
        Platform::Kalshi => self.execute_kalshi(request.clone()).await,
        Platform::Polymarket => self.execute_polymarket(request.clone()).await,
        _ => Ok(/* ... */),
    };

    match result {
        Ok(exec_result) if exec_result.status == ExecutionStatus::Failed => {
            // If failed due to API error, try fallback platform
            if exec_result.rejection_reason.as_ref().map(|r| r.contains("API error")).unwrap_or(false) {
                warn!(
                    "Primary platform {} failed, attempting fallback",
                    request.platform
                );

                // Try alternate platform (if signal has both)
                // (Implementation depends on market discovery finding both platforms)
            }
            Ok(exec_result)
        }
        other => other,
    }
}
```

**Files to Modify:**
- `services/execution_service_rust/src/engine.rs`

---

## Medium Priority Gaps

### 13. No Per-Sport Exposure Limits

**Current State:** Signal processor has MAX_SPORT_EXPOSURE but execution service doesn't enforce

**Required:** Add per-sport limits in execution service

---

### 14. No Time-Based Trading Windows

**Current State:** Trading 24/7 when markets available

**Required:** Add configurable trading hours (e.g., only trade NBA during games, not pregame)

---

### 15. No Model Version Tracking

**Current State:** Can't tell which model version generated signal

**Required:** Add model_version field to signals, track in audit logs

---

### 16. No Execution Quality Metrics

**Current State:** No tracking of fill rate, slippage, latency percentiles

**Required:** Add execution quality dashboard (Grafana)

---

## Implementation Priority

### Phase 1: Critical Safeguards (Deploy Before Live Trading)
**Timeline:** 2-3 days

1. ✅ Trading Authorization Confirmation (#1) - 4 hours
2. ✅ Maximum Order Size Limits (#2) - 4 hours
3. ✅ Account Balance Validation (#3) - 6 hours
4. ✅ Emergency Kill Switch (#5) - 4 hours
5. ✅ Idempotency Enforcement (#6) - 4 hours

**Total:** ~22 hours of focused development

### Phase 2: High Priority (Deploy Within First Week)
**Timeline:** 3-4 days

1. ✅ Rate Limiting (#4) - 4 hours
2. ✅ Detailed Audit Logging (#7) - 6 hours
3. ✅ Price Sanity Checks (#8) - 3 hours
4. ✅ Real-Time Balance Monitoring (#9) - 4 hours

**Total:** ~17 hours

### Phase 3: Medium Priority (Deploy Within First Month)
**Timeline:** 1 week

1. ✅ Order Confirmation Receipts (#10) - 4 hours
2. ✅ Position Reconciliation (#11) - 8 hours
3. ✅ Graceful Degradation (#12) - 6 hours
4. ✅ Per-Sport Exposure Limits (#13) - 3 hours

**Total:** ~21 hours

---

## Testing Checklist

### Before Implementing Safeguards
- [ ] Document current behavior
- [ ] Create test cases for each safeguard
- [ ] Verify paper trading still works

### After Implementing Safeguards
- [ ] Unit tests for each safeguard
- [ ] Integration tests with real signals (paper trading)
- [ ] Load test: 100 signals/minute with rate limiting
- [ ] Chaos test: Simulate exchange API failures
- [ ] Kill switch test: Verify immediate halt
- [ ] Balance test: Verify rejection when insufficient funds
- [ ] Idempotency test: Send same signal twice, verify rejection
- [ ] Audit log test: Verify all events logged

### Before Live Trading
- [ ] Run 1 week of paper trading with all safeguards enabled
- [ ] Verify NO false rejections from safeguards
- [ ] Verify safeguards DID prevent bad orders (inject test errors)
- [ ] Manual kill switch test in production environment
- [ ] Confirm audit logs are being collected and stored

---

## Configuration Template

```bash
# .env additions for live trading security

# ============================================================================
# LIVE TRADING AUTHORIZATION (Critical - Phase 1)
# ============================================================================
PAPER_TRADING=0                          # Disable paper trading
LIVE_TRADING_AUTHORIZED=true             # Explicitly authorize live trading

# ============================================================================
# ORDER SIZE LIMITS (Critical - Phase 1)
# ============================================================================
MAX_ORDER_SIZE=100.0                     # Max $100 per order (start conservative)
MAX_ORDER_CONTRACTS=100                  # Max 100 contracts per order
MAX_POSITION_PER_MARKET=200.0            # Max $200 exposure per market

# ============================================================================
# RATE LIMITING (Critical - Phase 1)
# ============================================================================
MAX_ORDERS_PER_MINUTE=20                 # Max 20 orders per minute
MAX_ORDERS_PER_HOUR=100                  # Max 100 orders per hour

# ============================================================================
# PRICE SAFETY (High Priority - Phase 2)
# ============================================================================
MIN_SAFE_PRICE=0.05                      # Reject prices <5%
MAX_SAFE_PRICE=0.95                      # Reject prices >95%
MAX_PRICE_DELTA_FROM_PREGAME=0.30        # Warn if price moves >30% from pregame

# ============================================================================
# BALANCE MONITORING (High Priority - Phase 2)
# ============================================================================
BALANCE_REFRESH_INTERVAL_SECS=60         # Refresh balance every 60 seconds
BALANCE_LOW_THRESHOLD=100.0              # Alert when balance <$100
DAILY_LOSS_ALERT_THRESHOLD=0.8           # Alert at 80% of MAX_DAILY_LOSS

# ============================================================================
# AUDIT LOGGING (High Priority - Phase 2)
# ============================================================================
AUDIT_LOG_ENABLED=true                   # Enable detailed audit logging
AUDIT_LOG_PATH=/var/log/arbees/execution_audit.jsonl
AUDIT_LOG_RETENTION_DAYS=90              # Keep audit logs for 90 days
```

---

## Monitoring & Alerts

### Critical Alerts (Immediate Action)
- Kill switch activated
- Daily loss limit exceeded
- Balance below threshold
- Repeated order rejections (>10/minute)
- Position reconciliation mismatch

### Warning Alerts (Review Within Hour)
- Approaching daily loss limit (>80%)
- Rate limit hit (indicates possible bug)
- Extreme price detected and rejected
- Exchange API errors

### Info Alerts (Review Daily)
- Total orders placed today
- Fill rate percentage
- Average execution latency
- Safeguard rejection counts

---

## Rollback Plan

If safeguards cause issues:

1. **Immediate:** Set `PAPER_TRADING=1`, restart service
2. **Investigate:** Check logs for false positives
3. **Adjust:** Tune safeguard thresholds (e.g., increase MAX_ORDER_SIZE)
4. **Re-enable:** Set `PAPER_TRADING=0`, monitor closely
5. **If still broken:** Disable specific safeguard, create issue

**Never disable all safeguards - fix individually**

---

## Summary

**Critical Gaps (Must Fix):**
1. No trading authorization confirmation
2. No maximum order size limit
3. No account balance validation
4. No rate limiting
5. No emergency kill switch
6. No idempotency enforcement
7. No detailed audit logging

**Estimated Time to Fix Critical Gaps:** ~39 hours (5 days focused development)

**Risk if Deployed Without Fixes:** HIGH
- Could lose entire account balance in minutes
- No way to stop runaway trading
- No audit trail for compliance

**Recommendation:**
1. **DO NOT** enable live trading (`PAPER_TRADING=0`) until Phase 1 safeguards implemented
2. Implement all 7 critical safeguards (5 days)
3. Test thoroughly in paper mode (1 week)
4. Deploy Phase 2 safeguards during first week of live trading
5. Monitor closely and tune thresholds based on real data

**The foundation is solid. These safeguards will make it production-grade.**

---

*Document Generated: 2026-01-28*
*Analysis Based On: execution_service_rust source code*
*Priority: CRITICAL - MUST IMPLEMENT BEFORE LIVE TRADING*
