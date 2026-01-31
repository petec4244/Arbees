# Phase 5: Detailed Refactoring Plan - monitor_game() Function

## Executive Summary

Refactor the monolithic `monitor_game()` async function (600+ lines) into focused, testable sub-functions while maintaining **100% behavioral equivalence** with baseline. This phase carries HIGH risk due to complex game state logic, multiple external dependencies, and real-time signal generation requirements.

**Key Risk Factors:**
- Signal emission logic affects trading decisions (financial impact)
- Complex ESPN state parsing with edge cases
- Race conditions in concurrent game monitoring
- Latency-sensitive signal timing
- Multiple external API dependencies (ESPN, ZMQ, Redis, PostgreSQL)

**Acceptance Criteria:**
- All 23 tests passing
- 100% signal output parity with baseline over 48-hour test window
- No memory leaks or CPU spikes under load
- Graceful degradation on ESPN API failures
- 0 panics or crashes in production

---

## Current State Analysis

### monitor_game() Function Overview

**Location:** `services/game_shard_rust/src/shard.rs:613-1020` (407 lines)

**Current Signature:**
```rust
async fn monitor_game(
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    game_id: String,
    sport: String,
    poll_interval: Duration,
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    min_edge_pct: f64,
    espn_circuit_breaker: Arc<ApiCircuitBreaker>,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,
    transport_mode: TransportMode,
)
```

**13 Parameters** - High cognitive load, lots of shared state

### Current Structure (Main Loop)

```
1. Signal debouncing setup (5 lines)
2. Price staleness TTL configuration (10 lines)
3. Game state staleness configuration (8 lines)
4. Previous score tracking (6 lines)
5. Stale state warning counter (2 lines)

MAIN LOOP (380 lines):
  ├─ Sleep for poll_interval
  ├─ Fetch ESPN game data (with circuit breaker)
  │   └─ Handle ESPN API failures (retry logic, warnings)
  ├─ Convert ESPN data to GameState struct
  │   ├─ Parse team names
  │   ├─ Parse clock/quarter info
  │   ├─ Parse scores
  │   └─ Handle parsing errors
  ├─ Check game completion
  │   └─ Break if final
  ├─ Calculate home win probability (with caching)
  ├─ Check price staleness
  │   └─ Skip signal generation if too old
  ├─ Find matching team prices from market_prices map
  ├─ Check for arbitrage opportunities
  │   ├─ Cross-platform price checking
  │   ├─ Arbitrage signal generation
  │   └─ ZMQ/Redis publishing
  ├─ Check for model-based edge signals
  │   ├─ Platform selection
  │   ├─ Signal debouncing
  │   └─ Signal emission
  ├─ Check for latency/score-change signals
  │   └─ Signal generation on score changes
  ├─ Persist game state to database
  │   ├─ Insert game state row
  │   └─ Handle DB errors (log, continue)
  └─ Loop
```

### Critical State Management

| State Variable | Purpose | Scope | Risk |
|---|---|---|---|
| `prev_home_score` | Latency signal detection | Local | Score changes can be missed if state is lost |
| `prev_away_score` | Latency signal detection | Local | Same as above |
| `stale_state_warnings` | Rate-limited logging | Local | Could mask real issues |
| `last_signal_times` | Debounce signals | Local | HashMap could grow unbounded (one entry per team per direction) |
| `last_home_win_prob` | Probability caching | Shared Arc<RwLock> | Write contention, stale reads |
| `market_prices` | Price lookup | Shared Arc<RwLock> | Concurrent reads, potential locks |
| ESPN circuit breaker state | API failure tracking | Shared Arc | Affects all games when ESPN is down |

### External Dependencies and Interactions

```
┌─────────────────────────────────────────────────────────────┐
│                    monitor_game()                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────────┐  ┌──────────────────┐               │
│  │  ESPN API        │  │  Market Prices   │               │
│  │  (CircuitBreaker)│  │  (HashMap RwLock)│               │
│  └────────┬─────────┘  └────────┬─────────┘               │
│           │                     │                          │
│  ┌────────▼──────────────────────▼───────┐                │
│  │  GameState Parsing & Analysis          │                │
│  │  - Sport mapping                       │                │
│  │  - Score extraction                    │                │
│  │  - Time/period parsing                 │                │
│  │  - Win probability calculation         │                │
│  └────────┬──────────────────────┬────────┘                │
│           │                      │                         │
│  ┌────────▼────────┐  ┌─────────▼───────┐                │
│  │ Signal Detection │  │ Signal Emission │                │
│  │ - Arbitrage      │  │ - ZMQ publish   │                │
│  │ - Model edge     │  │ - Redis fallback│                │
│  │ - Latency/score  │  │                 │                │
│  └────────┬────────┘  └─────────┬───────┘                │
│           │                     │                         │
│  ┌────────▼─────────────────────▼──────────┐             │
│  │ Database Persistence                    │             │
│  │ - game_states table (TimescaleDB)       │             │
│  │ - market_prices table (optional)        │             │
│  └─────────────────────────────────────────┘             │
│                                                             │
│  Error Handling Paths:                                    │
│  ├─ ESPN API down → Circuit breaker → Skip signals       │
│  ├─ Stale prices → Skip signal generation                 │
│  ├─ DB failure → Log, continue (don't break game loop)   │
│  └─ Parse failures → Log, continue with partial state    │
└─────────────────────────────────────────────────────────────┘
```

### Key Behaviors to Preserve

1. **ESPN Circuit Breaker**: Prevent cascading failures
   - After N failures, stop trying for M seconds
   - Gradual recovery with exponential backoff

2. **Price Staleness Check**: Don't trade on old data
   - Market prices timestamped when received
   - Skip signals if last update > PRICE_STALENESS_TTL
   - Log staleness warnings (rate-limited)

3. **Game State Staleness**: Detect stalled games
   - Skip signals if ESPN data is too old
   - Warn operator if ESPN not updating

4. **Signal Debouncing**: Prevent duplicate trades
   - HashMap keyed by (team, direction)
   - Skip if last signal < SIGNAL_DEBOUNCE_SECS ago
   - **Must not grow unbounded** (memory leak risk)

5. **Latency Signal Logic**: Score-change detection
   - Compare `prev_home_score` vs `new_home_score`
   - Only emit if score changed (latency edges)
   - Currently disabled (ESPN too slow)

6. **Database Persistence**: Game state recording
   - Insert one row per poll_interval
   - Continue even if DB fails (non-critical)
   - Timestamp precision: milliseconds

---

## Target Architecture

### Function Decomposition Map

```
monitor_game() [Orchestrator, 60 lines]
├─ initialize_monitoring_state() [40 lines]
│  ├─ Load debounce config from env
│  ├─ Load price/game staleness TTLs
│  ├─ Create empty debounce map
│  └─ Return GameMonitoringState struct
├─ main_loop() [Orchestrator, 30 lines]
│  ├─ sleep(poll_interval)
│  ├─ fetch_espn_game_state()
│  ├─ check_game_completion()
│  ├─ update_win_probability()
│  ├─ check_price_staleness()
│  ├─ detect_and_emit_signals()
│  ├─ persist_game_state()
│  └─ loop (infinite)
│
├─ fetch_espn_game_state() [50 lines]
│  ├─ Call EspnClient::get_game() with circuit breaker
│  ├─ Handle circuit breaker open (log, return None)
│  ├─ Handle HTTP errors (partial states allowed)
│  ├─ Parse ESPN response to GameState
│  └─ Return Result<GameState>
│
├─ parse_game_state() [80 lines]
│  ├─ Extract team names (home/away)
│  ├─ Extract scores (home/away)
│  ├─ Extract period/quarter
│  ├─ Extract clock/time remaining
│  ├─ Handle sport-specific formats
│  ├─ Handle edge cases (OT, halftime, final)
│  └─ Validate parsed state (sanity checks)
│
├─ check_game_completion() [15 lines]
│  ├─ Detect "final" status
│  ├─ Log completion
│  ├─ Return bool (should_stop)
│  └─ Break main loop if true
│
├─ update_win_probability() [40 lines]
│  ├─ Calculate new home_win_prob
│  ├─ Compare with cached value
│  ├─ Update last_home_win_prob Arc<RwLock>
│  ├─ Handle calculation errors gracefully
│  └─ Return (old_prob, new_prob) for signal use
│
├─ check_price_staleness() [30 lines]
│  ├─ Get youngest price timestamp
│  ├─ Compare with NOW - PRICE_STALENESS_TTL
│  ├─ Rate-limit staleness warnings
│  ├─ Log if stale
│  └─ Return bool (is_price_fresh)
│
├─ detect_and_emit_signals() [100 lines]
│  ├─ find_team_prices() [20 lines]
│  │  ├─ Query market_prices map for game_id
│  │  ├─ Fuzzy match team names
│  │  ├─ Filter by price staleness
│  │  └─ Return Vec<(team, platform, price)>
│  │
│  ├─ detect_arbitrage() [40 lines]
│  │  ├─ Call check_cross_platform_arb() [SIMD]
│  │  ├─ Filter by profit threshold
│  │  ├─ Call arbitrage::detect_and_emit()
│  │  └─ Log/count emissions
│  │
│  ├─ detect_model_edge() [30 lines]
│  │  ├─ For each team, calculate edge vs market
│  │  ├─ Check debounce map
│  │  ├─ Call model_edge::detect_and_emit()
│  │  ├─ Update debounce map
│  │  └─ Clean old entries (prevent leak)
│  │
│  └─ detect_latency_signals() [10 lines]
│     ├─ Compare prev_home_score vs current
│     ├─ If changed, call latency::detect_and_emit()
│     ├─ Update prev_home_score
│     └─ Return early (currently disabled)
│
├─ persist_game_state() [20 lines]
│  ├─ Build GameStateRow from current state
│  ├─ Execute INSERT to TimescaleDB
│  ├─ Handle DB errors (log, don't crash)
│  └─ Return Result (unused - just log)
│
└─ cleanup_debounce_map() [15 lines]
   ├─ Iterate debounce map
   ├─ Remove entries older than SIGNAL_DEBOUNCE_SECS
   ├─ Return count of cleaned entries
   └─ Call once per 10 iterations (prevent unbounded growth)
```

### New Struct: GameMonitoringState

```rust
pub struct GameMonitoringState {
    // Configuration
    pub signal_debounce_secs: u64,
    pub price_staleness_secs: i64,
    pub game_state_staleness_secs: i64,

    // Runtime state
    pub prev_home_score: Option<u16>,
    pub prev_away_score: Option<u16>,
    pub last_signal_times: HashMap<(String, String), Instant>, // (team, direction)
    pub stale_state_warnings: u32,
    pub debounce_cleanup_counter: u32,
}

impl GameMonitoringState {
    pub fn new(
        signal_debounce_secs: u64,
        price_staleness_secs: i64,
        game_state_staleness_secs: i64,
    ) -> Self { ... }

    pub fn should_debounce_signal(&self, team: &str, direction: &str) -> bool { ... }

    pub fn record_signal(&mut self, team: String, direction: String) { ... }

    pub fn cleanup_old_signals(&mut self) -> usize { ... }

    pub fn update_score(&mut self, home: u16, away: u16) -> (bool, bool) {
        // Returns (home_changed, away_changed)
    }
}
```

---

## Implementation Phases

### Phase 5A: Preparation & Testing Infrastructure (3 hours)

**Goal:** Set up comprehensive test harness before making changes

#### 5A.1: Create Test Fixtures

**File:** `services/game_shard_rust/tests/monitor_game_fixtures.rs` (NEW - 500 lines)

```rust
// Mock ESPN responses for different game scenarios
pub struct EspnFixture {
    pub responses: Vec<EspnGameResponse>,
    pub timestamps: Vec<Instant>,
    pub response_index: Arc<Mutex<usize>>,
}

impl EspnFixture {
    // Pre-recorded ESPN data for:
    // 1. Normal game (Q1-Q4, scores changing)
    // 2. Halftime game (end of Q2)
    // 3. Overtime game (extra periods)
    // 4. Final score game
    // 5. Stalled game (timestamps not updating)
    // 6. Parsing error case (missing fields)
    // 7. API timeout case
}

// Create 7 game scenarios with pre-populated prices
pub struct GameScenario {
    pub game_id: String,
    pub sport: String,
    pub espn_fixture: EspnFixture,
    pub market_prices: HashMap<String, HashMap<String, MarketPriceData>>,
    pub expected_signals: Vec<SignalExpectation>,
}

pub struct SignalExpectation {
    pub signal_type: SignalType,
    pub team: String,
    pub time_window: (Instant, Instant), // Allow time variance
    pub required: bool,                   // Some signals are conditional
}
```

**Scenarios to Create:**

| Scenario | Purpose | Expected Signals | Notes |
|----------|---------|------------------|-------|
| Normal Game (0-60 min) | Baseline behavior | 3-5 arbitrage, 2-3 model edge | Prices update regularly |
| Stale Prices | Price staleness detection | 0 signals (all skipped) | ESPN updates but prices stale |
| Stalled ESPN | ESPN not updating | 0 signals + warnings | No new game state |
| Score Change (OT) | Latency signals | 1-2 latency signals | prev_score tracking |
| API Failures (CB) | Circuit breaker | Graceful degradation | Keep looping, skip signals |
| Database Failure | DB error handling | Signals still emitted, DB rows missing | Must not crash loop |
| Mixed Prices (stale + fresh) | Partial price data | Signals only on fresh prices | Fuzzy matching required |

**Test Methods:**

```rust
#[tokio::test]
async fn test_fixture_espn_normal_game() {
    let fixture = GameScenario::normal_game();
    // Verify 7 ESPN responses are provided
    // Verify market prices for all teams/platforms
    // Verify expected signals list
}
```

#### 5A.2: Create Baseline Capture Infrastructure

**File:** `services/game_shard_rust/src/shard_test_utils.rs` (NEW - 300 lines)

Instrument current `monitor_game()` to capture:

```rust
pub struct SignalCapture {
    pub signal_id: String,
    pub timestamp: Instant,
    pub game_state_at_emission: GameState,
    pub market_prices_at_emission: HashMap<(String, String), MarketPriceData>,
    pub signal: TradingSignal,
}

pub struct MonitorGameBaseline {
    pub game_id: String,
    pub signals_captured: Vec<SignalCapture>,
    pub game_states_logged: Vec<(Instant, GameState)>,
    pub errors: Vec<(Instant, String)>,
    pub duration: Duration,
}

// Wrap current monitor_game() with capture layer
pub async fn monitor_game_with_capture(
    ... parameters ...
) -> MonitorGameBaseline {
    // Intercept ZMQ emissions
    // Log game states
    // Track timings
    // Return baseline
}
```

#### 5A.3: Run Baseline Collection

```bash
# For each scenario, capture baseline signals
./collect_baselines.sh \
  --scenario normal_game \
  --duration 5m \
  --output baseline_normal_game.json

# Generates:
# baseline_normal_game.json
# baseline_stale_prices.json
# baseline_score_change.json
# etc.
```

**Deliverables:**
- 7 baseline JSON files (signal sequences, timings, game states)
- Baseline verification script (for parity checks)

---

### Phase 5B: Core Function Extraction (8 hours)

**Goal:** Extract functions with 100% behavior equivalence to baseline

#### 5B.1: Extract Type & State Structs

**File:** `services/game_shard_rust/src/monitoring/game.rs` (CREATE - start)

```rust
// Add to monitoring/mod.rs: pub mod game;

pub struct GameMonitoringState {
    // ... (as defined in Target Architecture)
}

pub enum SignalEmissionResult {
    EmittedArbitrage { count: usize },
    EmittedModelEdge { count: usize },
    EmittedLatency { count: usize },
    Skipped { reason: String },
}
```

**Tests for State Struct:**

```rust
#[test]
fn test_debounce_blocks_repeated_signal() {
    let mut state = GameMonitoringState::new(30, 30, 30);

    // First signal should not be debounced
    assert!(!state.should_debounce_signal("Celtics", "buy"));
    state.record_signal("Celtics".to_string(), "buy".to_string());

    // Immediate repeat should be debounced
    assert!(state.should_debounce_signal("Celtics", "buy"));

    // Different direction should not be debounced
    assert!(!state.should_debounce_signal("Celtics", "sell"));

    // Fast forward past debounce window
    // (requires exposing time mocking)
    assert!(!state.should_debounce_signal("Celtics", "buy"));
}

#[test]
fn test_score_tracking_detects_changes() {
    let mut state = GameMonitoringState::new(30, 30, 30);

    // Initial update
    let (home_changed, away_changed) = state.update_score(7, 0);
    assert!(!home_changed && !away_changed); // First time

    // Home score changes
    let (home_changed, away_changed) = state.update_score(10, 0);
    assert!(home_changed && !away_changed);

    // No change
    let (home_changed, away_changed) = state.update_score(10, 0);
    assert!(!home_changed && !away_changed);
}

#[test]
fn test_cleanup_removes_old_signals() {
    let mut state = GameMonitoringState::new(30, 30, 30);

    state.record_signal("Team1".to_string(), "buy".to_string());
    state.record_signal("Team2".to_string(), "sell".to_string());

    // Fast forward 40 seconds
    // (needs time mock - use std::time::SystemTime or inject clock)

    let cleaned = state.cleanup_old_signals();
    assert_eq!(cleaned, 2); // Both removed
    assert_eq!(state.last_signal_times.len(), 0); // Verify memory
}

#[test]
fn test_cleanup_prevents_unbounded_growth() {
    let mut state = GameMonitoringState::new(30, 30, 30);

    // Add 1000 signals over time (simulating long game)
    for i in 0..1000 {
        state.record_signal(
            format!("Team{}", i % 30), // Repeat teams
            if i % 2 == 0 { "buy" } else { "sell" }.to_string()
        );

        if i % 50 == 0 {
            state.cleanup_old_signals();
        }
    }

    // Map should be bounded (30 teams * 2 directions max)
    assert!(state.last_signal_times.len() <= 60);
}
```

#### 5B.2: Extract Helper Functions (Non-Stateful)

**File:** `services/game_shard_rust/src/monitoring/game.rs` (continue)

Extract functions that don't need state mutation:

```rust
// 1. fetch_espn_game_state() - 50 lines
pub async fn fetch_espn_game_state(
    game_id: &str,
    espn: &EspnClient,
    circuit_breaker: &ApiCircuitBreaker,
) -> Result<GameState, GameMonitorError> {
    // Call ESPN API with circuit breaker
    // Parse response
    // Return Result
}

// 2. parse_game_state() - 80 lines (moved from shard.rs)
pub fn parse_game_state(
    espn_game: &EspnGame,
    sport: Sport,
) -> Result<GameState, ParseError> {
    // Extract scores, teams, period, time
    // Handle sport-specific logic
    // Validate sanity checks
}

// 3. check_game_completion() - 15 lines
pub fn is_game_final(game_state: &GameState) -> bool {
    game_state.status == "Final"
}

// 4. find_team_prices() - 30 lines (move from price::matching)
pub fn find_team_prices_for_game(
    game_id: &str,
    home_team: &str,
    away_team: &str,
    market_prices: &Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
) -> Result<Vec<(String, MarketPriceData)>, LookupError> {
    // Fuzzy match teams
    // Return both home and away prices
}

// 5. check_price_staleness() - 30 lines
pub fn is_price_fresh(
    prices: &[(String, MarketPriceData)],
    staleness_ttl: i64,
) -> bool {
    prices.iter().all(|(_, p)| {
        Utc::now().signed_duration_since(p.timestamp).num_seconds() < staleness_ttl
    })
}

// 6. log_stale_warning() - 8 lines
pub fn maybe_log_stale_warning(
    warning_count: &mut u32,
    rate_limit: u32,
) -> bool {
    if *warning_count % rate_limit == 0 {
        warn!("ESPN game data stale");
        true
    } else {
        false
    }
}

// 7. calculate_win_probability() - 40 lines (moved from shard.rs)
pub fn update_home_win_probability(
    game_state: &GameState,
    is_home: bool,
) -> f64 {
    // Use arbees_rust_core::win_prob
    // Return new probability
}
```

**Tests for Helper Functions:**

```rust
#[tokio::test]
async fn test_fetch_espn_handles_circuit_breaker_open() {
    let cb = MockCircuitBreaker::new_open();
    let result = fetch_espn_game_state("game1", &mock_espn, &cb).await;

    assert!(matches!(result, Err(GameMonitorError::CircuitBreakerOpen)));
}

#[tokio::test]
async fn test_fetch_espn_handles_http_timeout() {
    let espn = MockEspnClient::new_timeout();
    let result = fetch_espn_game_state("game1", &espn, &mock_cb).await;

    assert!(matches!(result, Err(GameMonitorError::HttpTimeout)));
}

#[test]
fn test_parse_game_state_nfl_format() {
    let espn_response = serde_json::from_str(FIXTURE_NFL_Q2)?;
    let game_state = parse_game_state(&espn_response, Sport::Football)?;

    assert_eq!(game_state.home_team, "Patriots");
    assert_eq!(game_state.away_team, "Dolphins");
    assert_eq!(game_state.home_score, 14);
    assert_eq!(game_state.away_score, 10);
    assert_eq!(game_state.quarter, 2);
    assert_eq!(game_state.time_remaining_secs, 300);
}

#[test]
fn test_parse_game_state_nba_overtime() {
    let espn_response = serde_json::from_str(FIXTURE_NBA_OT)?;
    let game_state = parse_game_state(&espn_response, Sport::Basketball)?;

    assert!(game_state.is_overtime);
    assert_eq!(game_state.period, 5); // OT counts as period 5
}

#[test]
fn test_is_price_fresh_all_recent() {
    let prices = vec![
        (
            "Celtics|Kalshi".to_string(),
            MarketPriceData {
                timestamp: Utc::now() - Duration::seconds(10),
                ..Default::default()
            }
        ),
    ];

    assert!(is_price_fresh(&prices, 30)); // 10s < 30s TTL
}

#[test]
fn test_is_price_fresh_one_stale() {
    let prices = vec![
        ("Fresh".to_string(), MarketPriceData { timestamp: Utc::now(), ..Default::default() }),
        ("Stale".to_string(), MarketPriceData { timestamp: Utc::now() - Duration::seconds(60), ..Default::default() }),
    ];

    assert!(!is_price_fresh(&prices, 30)); // 60s > 30s TTL
}
```

#### 5B.3: Extract Signal Detection Functions

**File:** `services/game_shard_rust/src/monitoring/game.rs` (continue)

```rust
// 1. detect_and_emit_arbitrage_signals() - 40 lines
pub async fn detect_and_emit_arbitrage_signals(
    game_id: &str,
    sport: Sport,
    team: &str,
    kalshi_price: Option<&MarketPriceData>,
    polymarket_price: Option<&MarketPriceData>,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> SignalEmissionResult {
    // Use check_cross_platform_arb() from monitoring::espn
    // Detect arb opportunities
    // Call arbitrage::detect_and_emit() for each
}

// 2. detect_and_emit_model_edge_signals() - 30 lines
pub async fn detect_and_emit_model_edge_signals(
    game_id: &str,
    sport: Sport,
    home_win_prob: f64,
    team: &str,
    market_price: &MarketPriceData,
    min_edge_pct: f64,
    state: &mut GameMonitoringState,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> SignalEmissionResult {
    // Check debounce
    // Calculate edge vs market
    // Call model_edge::detect_and_emit()
    // Update debounce state
}

// 3. detect_and_emit_latency_signals() - 20 lines
pub async fn detect_and_emit_latency_signals(
    game_id: &str,
    sport: Sport,
    team: &str,
    prev_score: Option<u16>,
    current_score: u16,
    market_price: &MarketPriceData,
    win_prob: f64,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> SignalEmissionResult {
    // If LATENCY_SIGNALS_ENABLED && prev_score != current_score
    // Call latency::detect_and_emit()
}

// 4. detect_and_emit_all_signals() - 60 lines [ORCHESTRATOR]
pub async fn detect_and_emit_all_signals(
    game_id: &str,
    sport: Sport,
    game_state: &GameState,
    market_prices: &[(String, MarketPriceData)],
    home_win_prob: f64,
    min_edge_pct: f64,
    state: &mut GameMonitoringState,
    zmq_pub: &Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: &Arc<AtomicU64>,
) -> GameSignalSummary {
    // For each team in game_state:
    //   - Emit arbitrage signals
    //   - Emit model edge signals
    //   - Emit latency signals (if enabled)
    // Return summary (counts, errors)
}
```

**Tests for Signal Detection:**

```rust
#[tokio::test]
async fn test_detect_arbitrage_kalshi_yes_polymarket_no() {
    let kalshi = MarketPriceData {
        platform: "Kalshi".to_string(),
        yes_ask: 0.45,
        yes_bid: 0.40,
        ..Default::default()
    };

    let polymarket = MarketPriceData {
        platform: "Polymarket".to_string(),
        yes_ask: 0.55,
        yes_bid: 0.50,
        ..Default::default()
    };

    let result = detect_and_emit_arbitrage_signals(
        "game1", Sport::Football, "Celtics",
        Some(&kalshi), Some(&polymarket),
        &None, &Arc::new(AtomicU64::new(0))
    ).await;

    // Should detect: buy Polymarket NO (sell YES at 0.50) + buy Kalshi YES at 0.45
    // Profit = 5 cents before fees
    assert!(matches!(result, SignalEmissionResult::EmittedArbitrage { count: 1 }));
}

#[tokio::test]
async fn test_detect_model_edge_respects_debounce() {
    let mut state = GameMonitoringState::new(30, 30, 30);

    // First signal should emit
    let result1 = detect_and_emit_model_edge_signals(
        "game1", Sport::Football, 0.65, "Celtics",
        &MarketPriceData { yes_ask: 0.50, ..Default::default() },
        5.0, &mut state, &None, &Arc::new(AtomicU64::new(0))
    ).await;
    assert!(matches!(result1, SignalEmissionResult::EmittedModelEdge { count: 1 }));

    // Immediate second signal should be debounced
    let result2 = detect_and_emit_model_edge_signals(
        "game1", Sport::Football, 0.65, "Celtics",
        &MarketPriceData { yes_ask: 0.50, ..Default::default() },
        5.0, &mut state, &None, &Arc::new(AtomicU64::new(0))
    ).await;
    assert!(matches!(result2, SignalEmissionResult::Skipped { .. }));
}

#[tokio::test]
async fn test_detect_latency_on_score_change() {
    let result = detect_and_emit_latency_signals(
        "game1", Sport::Football, "Celtics",
        Some(7),  // prev score
        10,       // current score (changed!)
        &MarketPriceData { yes_ask: 0.50, ..Default::default() },
        0.65,
        &None, &Arc::new(AtomicU64::new(0))
    ).await;

    // Should emit if enabled
    #[cfg(feature = "latency_signals")]
    assert!(matches!(result, SignalEmissionResult::EmittedLatency { .. }));

    #[cfg(not(feature = "latency_signals"))]
    assert!(matches!(result, SignalEmissionResult::Skipped { .. }));
}

#[tokio::test]
async fn test_detect_latency_no_score_change() {
    let result = detect_and_emit_latency_signals(
        "game1", Sport::Football, "Celtics",
        Some(10),  // prev score
        10,        // current score (same)
        &MarketPriceData { yes_ask: 0.50, ..Default::default() },
        0.65,
        &None, &Arc::new(AtomicU64::new(0))
    ).await;

    // Should skip (no score change)
    assert!(matches!(result, SignalEmissionResult::Skipped { .. }));
}
```

#### 5B.4: Extract Main Loop Functions

**File:** `services/game_shard_rust/src/monitoring/game.rs` (continue)

```rust
// Main orchestrator
pub async fn monitor_game(
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    game_id: String,
    sport: String,
    poll_interval: Duration,
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    min_edge_pct: f64,
    espn_circuit_breaker: Arc<ApiCircuitBreaker>,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,
    transport_mode: TransportMode,
) {
    // Initialize state
    let mut state = GameMonitoringState::new(
        load_signal_debounce_secs(),
        load_price_staleness_secs(),
        load_game_state_staleness_secs(),
    );

    loop {
        tokio::time::sleep(poll_interval).await;

        // Fetch game state
        let game_state = match fetch_espn_game_state(&game_id, &espn, &espn_circuit_breaker).await {
            Ok(gs) => gs,
            Err(e) => {
                warn!("Failed to fetch ESPN data: {}", e);
                continue; // Try again next iteration
            }
        };

        // Check completion
        if is_game_final(&game_state) {
            info!("Game {} final, exiting monitor loop", game_id);
            break;
        }

        // Update win probability
        let home_win_prob = update_home_win_probability(&game_state, true);
        let _ = last_home_win_prob.write().await.insert(home_win_prob);

        // Check price freshness
        let market_prices_lock = market_prices.read().await;
        let prices = find_team_prices_for_game(
            &game_id,
            &game_state.home_team,
            &game_state.away_team,
            &market_prices,
        ).unwrap_or_default();
        drop(market_prices_lock);

        if !is_price_fresh(&prices, state.price_staleness_secs) {
            state.stale_state_warnings += 1;
            maybe_log_stale_warning(&mut state.stale_state_warnings, 10);
            continue; // Skip signal generation
        }

        // Detect and emit signals
        let signal_summary = detect_and_emit_all_signals(
            &game_id,
            sport,
            &game_state,
            &prices,
            home_win_prob,
            min_edge_pct,
            &mut state,
            &zmq_pub,
            &zmq_seq,
        ).await;

        // Periodic cleanup
        state.debounce_cleanup_counter += 1;
        if state.debounce_cleanup_counter >= 10 {
            let cleaned = state.cleanup_old_signals();
            debug!("Cleaned {} old debounce entries", cleaned);
            state.debounce_cleanup_counter = 0;
        }

        // Persist game state
        if let Err(e) = persist_game_state(
            &db_pool,
            &game_id,
            &game_state,
            home_win_prob,
        ).await {
            warn!("Failed to persist game state: {}", e);
            // Don't break - DB is non-critical
        }
    }
}

async fn persist_game_state(
    db_pool: &PgPool,
    game_id: &str,
    game_state: &GameState,
    home_win_prob: f64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO game_states
            (game_id, home_team, away_team, home_score, away_score, period, time_remaining, home_win_prob, recorded_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
        "#
    )
    .bind(game_id)
    .bind(&game_state.home_team)
    .bind(&game_state.away_team)
    .bind(game_state.home_score)
    .bind(game_state.away_score)
    .bind(game_state.period as i32)
    .bind(game_state.time_remaining_secs as i32)
    .bind(home_win_prob)
    .execute(db_pool)
    .await?;

    Ok(())
}
```

**Tests for Main Loop:**

```rust
#[tokio::test]
async fn test_monitor_game_main_loop_one_iteration() {
    let scenario = GameScenario::normal_game();
    let baseline = baseline::baseline_for_scenario(&scenario);

    // Mock ESPN to return fixture responses
    let mock_espn = EspnFixture::new(scenario.espn_responses);

    // Run one iteration of monitor_game
    monitor_game_one_iteration(
        &mock_espn,
        &scenario.game_id,
        &scenario.market_prices,
        &scenario.sport,
    ).await;

    // Verify signals match baseline (within time window)
    // verify_signal_parity(&captured_signals, &baseline)?;
}
```

#### 5B.5: Replace Old monitor_game in shard.rs

```rust
// In shard.rs, replace old monitor_game() with:

async fn monitor_game(
    redis: RedisBus,
    espn: EspnClient,
    db_pool: PgPool,
    game_id: String,
    sport: String,
    poll_interval: Duration,
    last_home_win_prob: Arc<RwLock<Option<f64>>>,
    market_prices: Arc<RwLock<HashMap<String, HashMap<String, MarketPriceData>>>>,
    min_edge_pct: f64,
    espn_circuit_breaker: Arc<ApiCircuitBreaker>,
    zmq_pub: Option<Arc<Mutex<PubSocket>>>,
    zmq_seq: Arc<AtomicU64>,
    transport_mode: TransportMode,
) {
    // Delegate to new implementation
    crate::monitoring::game::monitor_game(
        redis, espn, db_pool, game_id, sport, poll_interval,
        last_home_win_prob, market_prices, min_edge_pct,
        espn_circuit_breaker, zmq_pub, zmq_seq, transport_mode
    ).await
}
```

---

### Phase 5C: Parity Testing (8 hours)

**Goal:** Verify new implementation matches baseline exactly

#### 5C.1: Automated Parity Tests

**File:** `services/game_shard_rust/tests/monitor_game_parity.rs` (NEW - 400 lines)

```rust
#[tokio::test]
async fn test_parity_baseline_vs_refactored_normal_game() {
    let scenario = GameScenario::normal_game();

    // Capture baseline signals from old implementation
    let baseline_signals = monitor_game_with_capture_old(
        &scenario.espn_fixture,
        &scenario.game_id,
        &scenario.sport,
    ).await;

    // Capture signals from new implementation
    let refactored_signals = monitor_game_with_capture_new(
        &scenario.espn_fixture,
        &scenario.game_id,
        &scenario.sport,
    ).await;

    // Compare
    assert_signal_parity(&baseline_signals, &refactored_signals, ALLOWED_TIME_VARIANCE);
}

#[tokio::test]
async fn test_parity_stale_prices() {
    let scenario = GameScenario::stale_prices();
    let baseline = monitor_game_with_capture_old(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;
    let refactored = monitor_game_with_capture_new(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;

    // Both should emit 0 signals (prices stale)
    assert_eq!(baseline.signals_captured.len(), 0);
    assert_eq!(refactored.signals_captured.len(), 0);
}

#[tokio::test]
async fn test_parity_api_failure_handling() {
    let scenario = GameScenario::api_failures();
    let baseline = monitor_game_with_capture_old(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;
    let refactored = monitor_game_with_capture_new(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;

    // Both should handle errors gracefully
    assert_eq!(baseline.errors.len(), refactored.errors.len());
    // Both should continue looping
    assert!(baseline.duration >= Duration::seconds(30));
    assert!(refactored.duration >= Duration::seconds(30));
}

#[tokio::test]
async fn test_parity_score_change_detection() {
    let scenario = GameScenario::score_change();
    let baseline = monitor_game_with_capture_old(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;
    let refactored = monitor_game_with_capture_new(&scenario.espn_fixture, &scenario.game_id, &scenario.sport).await;

    // Compare latency signal emissions
    let baseline_latency = baseline.signals_captured
        .iter()
        .filter(|s| s.signal.signal_type == SignalType::ScoringPlay)
        .count();
    let refactored_latency = refactored.signals_captured
        .iter()
        .filter(|s| s.signal.signal_type == SignalType::ScoringPlay)
        .count();

    assert_eq!(baseline_latency, refactored_latency);
}

fn assert_signal_parity(
    baseline: &[SignalCapture],
    refactored: &[SignalCapture],
    time_variance: Duration,
) {
    assert_eq!(
        baseline.len(),
        refactored.len(),
        "Signal count mismatch: baseline={}, refactored={}",
        baseline.len(),
        refactored.len()
    );

    for (b, r) in baseline.iter().zip(refactored.iter()) {
        assert_eq!(b.signal.signal_type, r.signal.signal_type, "Signal type mismatch");
        assert_eq!(b.signal.team, r.signal.team, "Team mismatch");
        assert_eq!(b.signal.direction, r.signal.direction, "Direction mismatch");

        // Allow time variance (processing may differ slightly)
        let time_diff = (b.timestamp - r.timestamp).abs();
        assert!(
            time_diff <= time_variance,
            "Signal timing variance too large: {}ms (allowed: {}ms)",
            time_diff.num_milliseconds(),
            time_variance.num_milliseconds()
        );
    }
}
```

#### 5C.2: Behavioral Equivalence Checks

```rust
#[tokio::test]
async fn test_debounce_behavior_identical() {
    // Run same signals through both old and new debounce logic
    // Verify same signals are emitted/blocked
}

#[tokio::test]
async fn test_price_staleness_checks_identical() {
    // Check both implementations skip signals identically
}

#[tokio::test]
async fn test_game_state_parsing_identical() {
    // Parse same ESPN responses through both
    // Verify identical GameState structures
}

#[tokio::test]
async fn test_win_probability_calculation_identical() {
    // Calculate prob with same game states
    // Verify byte-for-byte equal (within f64 epsilon)
}

#[tokio::test]
async fn test_database_inserts_identical() {
    // Verify same game_states rows inserted
    // Same schema, same values
}
```

---

### Phase 5D: Integration & Load Testing (6 hours)

**Goal:** Verify robustness under real-world conditions

#### 5D.1: Multi-Game Concurrent Load Test

```rust
#[tokio::test]
#[ignore] // Long-running test
async fn test_monitor_10_games_simultaneously() {
    // Spawn 10 monitor_game tasks
    // Each with different scenarios
    // Run for 10 minutes

    let handles: Vec<_> = (0..10)
        .map(|i| {
            tokio::spawn(async move {
                monitor_game_with_capture(
                    SCENARIOS[i],
                    Duration::from_millis(100), // Fast poll for testing
                ).await
            })
        })
        .collect();

    // Collect results
    let results = futures::future::join_all(handles).await;

    // Verify all completed successfully
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_ok(), "Game {} failed: {:?}", i, result.err());
    }

    // Verify signal emissions
    // Check for resource leaks
    // Verify no hangs or timeouts
}
```

#### 5D.2: Resource Leak Detection

```rust
#[tokio::test]
#[ignore]
async fn test_no_memory_leaks_long_game() {
    // Run single game for 30 minutes (300 iterations at 6s poll)
    let initial_memory = get_process_memory();

    let result = monitor_game_long_running(
        SCENARIO_NORMAL,
        Duration::from_secs(6),
        Duration::from_secs(1800),
    ).await;

    let final_memory = get_process_memory();
    let memory_growth = final_memory - initial_memory;

    // Memory growth should be minimal (< 50MB for 300 iterations)
    assert!(
        memory_growth < Bytes::from_megabytes(50),
        "Memory growth too large: {} MB",
        memory_growth.num_megabytes()
    );

    // Specifically check debounce map doesn't grow unbounded
    assert!(result.max_debounce_map_size < 100);
}
```

#### 5D.3: ESPN API Failure Scenarios

```rust
#[tokio::test]
async fn test_circuit_breaker_opens_after_failures() {
    let mock_espn = EspnMock::with_consecutive_failures(10);

    let result = monitor_game_with_capture(
        SCENARIO_WITH_API_MOCK,
        Duration::from_millis(100),
    ).await;

    // Should have circuit breaker opening
    assert!(result.circuit_breaker_opened);
    // Should continue looping (not crash)
    assert!(result.duration >= Duration::seconds(5));
}

#[tokio::test]
async fn test_circuit_breaker_recovers_gradually() {
    let mock_espn = EspnMock::failing_then_recovering(
        failures: 10,
        recovery_time: Duration::from_secs(2),
    );

    let result = monitor_game_with_capture(SCENARIO_WITH_API_MOCK).await;

    // After recovery, should start emitting signals again
    assert!(result.signals_before_open > 0);
    assert!(result.signals_after_recovery > 0);
}
```

#### 5D.4: Database Failure Handling

```rust
#[tokio::test]
async fn test_continues_on_database_failure() {
    let mock_db = MockDb::failing();

    let result = monitor_game_with_capture(
        SCENARIO_NORMAL,
        mock_db: Some(mock_db),
    ).await;

    // Should emit signals normally (DB is non-critical)
    assert!(result.signals_emitted > 5);

    // Should log DB errors but continue
    assert!(result.errors.iter().any(|e| e.contains("database")));
}

#[tokio::test]
async fn test_database_connection_pool_exhaustion() {
    let mock_db = MockDb::limited_connections(5);

    // Run 10 games concurrently
    // Some will need to wait for DB connections

    let handles: Vec<_> = (0..10)
        .map(|_| {
            tokio::spawn(monitor_game_with_capture(SCENARIO_NORMAL, mock_db.clone()))
        })
        .collect();

    let results = futures::future::join_all(handles).await;

    // All should complete despite connection limits
    assert!(results.iter().all(|r| r.is_ok()));

    // Should have some connection waits (not errors)
    // Verify connection pool was actually stressed
}
```

#### 5D.5: Concurrent Market Price Updates

```rust
#[tokio::test]
async fn test_handles_concurrent_price_updates() {
    let market_prices = Arc::new(RwLock::new(HashMap::new()));

    // Spawn monitor_game
    let monitor_handle = {
        let mp = market_prices.clone();
        tokio::spawn(monitor_game_with_mock(
            market_prices: mp,
            poll_interval: Duration::from_millis(100),
        ))
    };

    // Concurrently update prices
    let price_update_handle = {
        let mp = market_prices.clone();
        tokio::spawn(async move {
            for i in 0..100 {
                let mut prices = mp.write().await;
                prices.insert(
                    format!("game1:{}", i % 10),
                    MarketPriceData {
                        timestamp: Utc::now(),
                        yes_ask: 0.50 + (i as f64) * 0.001,
                        ..Default::default()
                    }
                );
                drop(prices);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };

    // Both should complete without deadlock
    let (monitor_result, price_result) = tokio::join!(monitor_handle, price_update_handle);
    assert!(monitor_result.is_ok());
    assert!(price_result.is_ok());
}
```

---

### Phase 5E: Real-World Scenario Testing (4 hours)

**Goal:** Test with recorded real game data and market conditions

#### 5E.1: Replay Recorded Game Data

Create test fixtures from actual production games:

```rust
#[tokio::test]
async fn test_replay_real_game_nfl_chiefs_49ers() {
    let game_recording = load_recorded_game("2024-09-15-NFL-chiefs-49ers.json");
    // Contains:
    // - 200 ESPN polling responses (one per 30 seconds, real game data)
    // - 1000+ market price updates (from Kalshi and Polymarket)
    // - Expected signals (arbitrage, edge, latency)

    let captured = monitor_game_with_replay(game_recording).await;

    // Verify:
    // - Game completion detected at correct time
    // - Win probability evolves realistically
    // - Signals align with actual arbitrage opportunities
    // - No spurious signals on stale data
    // - Proper handling of halftime (no updates for ~12 min)

    assert_signal_quality(&captured, &game_recording.expected);
}

#[tokio::test]
async fn test_replay_real_game_nba_celtics_heat_overtime() {
    let game_recording = load_recorded_game("2024-06-10-NBA-celtics-heat.json");

    let captured = monitor_game_with_replay(game_recording).await;

    // Verify overtime handling:
    // - Period tracking through 4+ periods
    // - Score changes during OT
    // - Win probability swings in OT
    // - Proper latency signal emissions

    assert!(captured.periods_covered > 4);
    assert!(captured.overtimes_detected);
}
```

#### 5E.2: Market Stress Scenarios

```rust
#[tokio::test]
async fn test_wide_bid_ask_spreads() {
    // Market with extreme spreads (e.g., 0.40 bid / 0.95 ask)
    let scenario = GameScenario::wide_spreads();

    let result = monitor_game_with_capture(scenario).await;

    // Should still detect arbitrage when profitable
    // Should not emit false positives
    // Should handle low liquidity gracefully
}

#[tokio::test]
async fn test_rapid_price_changes() {
    // Market updates multiple times per second
    let scenario = GameScenario::high_frequency_prices();

    let result = monitor_game_with_capture(scenario).await;

    // Should handle rapid updates
    // Should not double-emit signals
    // Debounce should work correctly
    // No race conditions in price lookups
}

#[tokio::test]
async fn test_one_sided_market() {
    // Only one team has prices (other not traded)
    let scenario = GameScenario::missing_team_prices();

    let result = monitor_game_with_capture(scenario).await;

    // Should gracefully skip that team
    // Should still process other team
    // Should not crash on missing team
}
```

#### 5E.3: Full 48-Hour Parallel Baseline Test

**This is the ultimate validation before production deployment**

```bash
#!/bin/bash
# run_48h_baseline_test.sh

# Phase 1: Start both implementations side-by-side
echo "Starting old implementation..."
./game_shard_v1 &
OLD_PID=$!

sleep 2

echo "Starting new implementation..."
./game_shard_refactored &
NEW_PID=$!

# Phase 2: Monitor for 48 hours
echo "Running parallel test for 48 hours..."
./monitor_parallel_test.sh \
  --old-pid $OLD_PID \
  --new-pid $NEW_PID \
  --duration 48h \
  --output baseline_48h_results.json

# Phase 3: Analysis
echo "Analyzing results..."
python analyze_baseline_comparison.py baseline_48h_results.json

# Phase 4: Generate report
./generate_comparison_report.sh baseline_48h_results.json > phase5_validation_report.md

# Phase 5: Cleanup
kill $OLD_PID
kill $NEW_PID
```

**What to Measure (48 hours):**
- Signal count: Both implementations should emit within 1% signals
- Signal timing: Same signals within ±100ms
- No crashes or panics in either
- Memory usage: Stable or within 100MB variance
- CPU usage: <5% per game
- Latency: Average signal emit time <50ms
- Database inserts: All rows match schema
- Error rates: <0.1%

---

### Phase 5F: Code Review & Safety Checks (2 hours)

#### 5F.1: Clippy & Rust Safety

```bash
cargo clippy --package game_shard_rust -- -D warnings
cargo audit
cargo tarpaulin --package game_shard_rust  # Code coverage
```

**Targets:**
- Code coverage > 85%
- 0 clippy warnings
- 0 security issues

#### 5F.2: Thread Safety Review

```rust
// Verify all Arc<RwLock<>> usage is sound
// - No locks held across await points
// - No potential deadlocks
// - No data races in shared state

#[tokio::test]
async fn test_no_deadlock_on_concurrent_access() {
    // Create contention on market_prices
    // Verify no deadlock
}
```

#### 5F.3: Documentation

Add code comments:
```rust
/// Monitors a single game in real-time, emitting trading signals
///
/// This function:
/// - Polls ESPN API on `poll_interval`
/// - Detects game state changes (scores, periods, completion)
/// - Calculates win probabilities
/// - Detects and emits arbitrage signals (cross-platform mispricing)
/// - Detects and emits model-based edge signals
/// - Detects and emits latency signals (score changes)
/// - Persists game state to TimescaleDB
///
/// ## Behavior
/// - Runs in infinite loop until game is final
/// - Handles ESPN API failures gracefully (circuit breaker)
/// - Skips signal generation if prices are stale
/// - Rate-limits stale data warnings
/// - Non-critical DB failures don't stop game monitoring
///
/// ## External Dependencies
/// - ESPN API (via `espn` parameter)
/// - ZMQ for signal publishing (optional)
/// - Redis for signal fallback (via transport_mode)
/// - TimescaleDB for state persistence
/// - Market price data (from `market_prices` Arc<RwLock>)
///
/// ## Concurrency
/// - Runs as separate tokio task per game
/// - Shares market_prices and win_probability via Arc<RwLock>
/// - No locks held across await points
pub async fn monitor_game(...)
```

---

## Testing Summary

### Test Pyramid

```
                    /\
                   /  \
                  / E2E \         [4h] Real game replays
                 /______\         [2h] Parallel baseline
                  /    \
                 /  Int \          [6h] Load tests
                /________\         [4h] Integration
                 /      \
                /  Unit   \        [8h] Function tests
               /__________ \       [8h] State tests
```

**Total Testing Effort: 32 hours across 40+ test suites**

### Test Matrix

| Category | Tests | Effort | Risk | Coverage |
|----------|-------|--------|------|----------|
| Unit (Functions) | 30+ | 8h | Low | 85%+ |
| Integration | 20+ | 8h | Medium | All paths |
| Load & Concurrency | 10+ | 6h | High | Edge cases |
| Real-world Scenarios | 5+ | 4h | High | Production-like |
| Parallel Baseline | 1 | 48h! | Critical | 100% parity |

---

## Risk Mitigation

### High-Risk Areas & Mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Signal double-emission | Financial loss | Debounce test coverage + replay validation |
| Price staleness handling | Wrong trades | Staleness test + real-world scenario testing |
| ESPN API failures | No signals | Circuit breaker tests + long-running load test |
| Memory leaks | Process crash | Long-running test (30m) + memory profiling |
| Race conditions | Spurious signals/crashes | Concurrent stress tests + thread safety review |
| Database failures | Lost game state | DB failure test + non-critical handling verification |

### Rollback Plan

If issues detected during Phase 5E (real-world testing):

**Immediate Rollback (< 5 min):**
```bash
# Option 1: Revert Phase 5 commit
git revert HEAD

# Option 2: Feature flag (if implemented)
# Set ENABLE_REFACTORED_MONITOR_GAME=false

# Option 3: Use old shard binary
docker pull game_shard:phase-4
docker run game_shard:phase-4
```

**Incident Response:**
1. Stop new version immediately
2. Redeploy old version from Phase 4 tag
3. Preserve error logs and metrics
4. Schedule post-mortem
5. Update test suite based on failure
6. Re-attempt with fixes

---

## Success Criteria (Phase 5 Complete)

- ✅ All 24 unit tests passing
- ✅ All 20+ integration tests passing
- ✅ Load tests: 10 concurrent games, 30 minutes, 0 crashes
- ✅ 48-hour parallel baseline: <1% signal deviation
- ✅ Code coverage > 85%
- ✅ 0 clippy warnings
- ✅ Memory stable (growth < 50MB per 300 iterations)
- ✅ CPU per game < 5%
- ✅ No panics in any test
- ✅ Real-world scenario tests pass (halftime, OT, API failures)
- ✅ Documentation complete
- ✅ Code review approval

---

## Implementation Timeline

| Phase | Duration | Effort | Owner |
|-------|----------|--------|-------|
| 5A: Test Infrastructure | 3h | 3h | Claude |
| 5B: Core Extraction | 8h | 8h | Claude |
| 5C: Parity Testing | 8h | 8h | Claude |
| 5D: Integration/Load | 6h | 6h | Claude |
| 5E: Real-world Scenarios | 4h | 4h | Claude |
| 5F: Code Review & Safety | 2h | 2h | Claude |
| **Parallel Baseline Test** | **48h** | **passive** | CI/CD |
| **Total Active Work** | **31h** | **31h** | **Claude** |

**Recommended Execution:**
- Days 1-2: 5A + 5B (11 hours)
- Days 2-3: 5C + 5D (14 hours)
- Days 3-4: 5E + 5F (6 hours)
- Days 4-6: Parallel baseline (passive, monitor for issues)

---

## Next Steps

1. User approval of plan
2. Create `monitor_game_fixtures.rs` with 7 test scenarios
3. Extract test infrastructure and baseline capture
4. Run baseline collection for all scenarios
5. Begin Phase 5A in detail
6. After each sub-phase: unit tests passing → move to next
7. After 5F: run parallel baseline test
8. After baseline passes: production deployment

---

**This plan provides:**
- ✅ Low-level detailed steps (6 phases, each with sub-tasks)
- ✅ Outside interaction handling (ESPN, ZMQ, Redis, DB failures)
- ✅ Expansive testing (32+ hours of tests, real-world scenarios, 48h baseline)
- ✅ Risk mitigation (rollback, circuit breaker testing, load testing)
- ✅ Clear success criteria
- ✅ Implementation timeline

**Ready to proceed? Approve this plan and I'll begin Phase 5A.**
