# Arbees Edge Trading System - Detailed Process Documentation

## Overview

**Arbees** is a live sports arbitrage trading system that detects information latency and mispricing between live game events and prediction market prices. The system monitors games across multiple sports (NFL, NBA, NHL, MLB, NCAA, MLS, Soccer, Tennis, MMA), calculates win probabilities faster than markets can react, and executes trades to capture edge before the market adjusts.

## Core Concept: Edge Detection

The system detects **edge** in two primary ways:

1. **Model Edge**: The system's win probability model calculates the "true" probability faster than markets adjust. When `model_probability - market_price > threshold`, there's an edge to buy. When `market_price - model_probability > threshold`, there's an edge to sell.

2. **Cross-Market Arbitrage**: Price differences between platforms (Kalshi vs Polymarket) where buying YES on one platform + buying NO on another costs less than $1.00, guaranteeing profit at settlement.

## System Architecture Flow

```
ESPN (Live Games)
    ↓
orchestrator_rust (Game Discovery)
    ↓
market_discovery_rust (Market ID Lookup)
    ↓
game_shard_rust (Game State + Price Monitoring)
    ↓
signal_processor_rust (Signal Generation & Filtering)
    ↓
execution_service_rust (Trade Execution)
    ↓
position_tracker_rust (Position Management & Exit Logic)
    ↓
notification_service_rust (Notifications via Signal)
```

---

## Step-by-Step Process Walkthrough

### Phase 1: Game Discovery & Assignment

**Service**: `orchestrator_rust`  
**Location**: `services/orchestrator_rust/src/main.rs` and `services/orchestrator_rust/src/managers/game_manager.rs`

#### Step 1.1: Discover Live Games
- **Code**: `services/orchestrator_rust/src/managers/game_manager.rs:156-188`
- **Process**:
  1. Orchestrator runs discovery cycle every `DISCOVERY_INTERVAL_SECS` (default: 60s)
  2. Fetches live games from ESPN API for all configured sports
  3. Filters out games already assigned to shards
  4. For each new game, calls `process_new_game()`

#### Step 1.2: Market Discovery
- **Code**: `services/orchestrator_rust/src/managers/game_manager.rs:190-244`
- **Process**:
  1. Publishes discovery request to Redis channel `discovery:requests`
  2. `market_discovery_rust` service receives request and:
     - Searches Polymarket for matching markets (via Gamma API)
     - Searches Kalshi for matching markets
     - Uses team matching RPC to match team names
  3. Discovery results published to `discovery:results` channel
  4. Orchestrator receives result and caches market IDs

#### Step 1.3: Shard Assignment
- **Code**: `services/orchestrator_rust/src/managers/game_manager.rs:277-326`
- **Process**:
  1. Gets best available shard from `ShardManager` (least loaded, healthy)
  2. Constructs assignment command with:
     - `game_id`, `sport`
     - `kalshi_market_id` (if found)
     - `polymarket_market_id` (if found)
  3. Publishes command to Redis channel `shard:{shard_id}:command`
  4. Shard receives command and starts monitoring the game

---

### Phase 2: Game State Monitoring & Win Probability Calculation

**Service**: `game_shard_rust`  
**Location**: `services/game_shard_rust/src/shard.rs`

#### Step 2.1: Game Monitoring Loop
- **Code**: `services/game_shard_rust/src/shard.rs:413-589`
- **Process**:
  1. Each assigned game runs in its own async task (`monitor_game()`)
  2. Polls ESPN API every `POLL_INTERVAL` (default: 1.0s) for game state
  3. Fetches: scores, period, time remaining, possession, down/distance, field position

#### Step 2.2: Win Probability Calculation
- **Code**: `rust_core/src/win_prob.rs` and `rust_core/src/lib.rs:443`
- **Process**:
  1. Calls `calculate_win_probability(&state, for_home: bool)` 
  2. **NFL/NCAAF** (`calculate_football_win_prob`):
     - Base probability from score differential
     - Volatility decreases as game progresses: `volatility = 14.0 * sqrt(time_fraction)`
     - Possession bonus: ~2.5 points, increases with field position
     - Redzone bonus: up to 4.0 points
     - Down/distance adjustment: later downs penalized
     - Final: `logistic(score_diff / volatility + adjustments)`
   
  3. **NBA/NCAAB** (`calculate_basketball_win_prob`):
     - Estimates possessions remaining
     - Volatility based on possessions: `sqrt(possessions) * 2.2`
     - Catch-up difficulty: large deficits late in game reduce volatility (makes outcomes more certain)
     - Possession worth ~1 point
   
  4. **Other Sports**: Similar models with sport-specific adjustments
   
  5. Returns probability (0.0 to 1.0) for home team winning

#### Step 2.3: Store Game State
- **Code**: `services/game_shard_rust/src/shard.rs:448-468`
- **Process**:
  1. Inserts game state into `game_states` hypertable (TimescaleDB)
  2. Publishes game state to Redis channel `game:{game_id}:state`
  3. Updates in-memory `last_home_win_prob` for signal generation

---

### Phase 3: Market Price Monitoring

**Service**: `game_shard_rust` (price listener) + `polymarket_monitor` (price publisher)  
**Location**: `services/game_shard_rust/src/shard.rs:310-384`

#### Step 3.1: Price Subscription
- **Code**: `services/game_shard_rust/src/shard.rs:310-384`
- **Process**:
  1. Shard subscribes to Redis pattern `game:*:price`
  2. `polymarket_monitor` service (separate) publishes prices to `game:{game_id}:price`
  3. Price messages contain:
     - `market_id`, `platform`, `contract_team`
     - `yes_bid`, `yes_ask`, `mid_price`
     - `timestamp`

#### Step 3.2: Price Storage
- **Code**: `services/game_shard_rust/src/shard.rs:352-381`
- **Process**:
  1. Filters out prices with no liquidity (`bid=0, ask=1` gives fake 50% mid)
  2. Stores prices in `market_prices` HashMap: `game_id -> team -> MarketPriceData`
  3. Also inserts into `market_prices` hypertable (TimescaleDB) for historical tracking

---

### Phase 4: Edge Detection & Signal Generation

**Service**: `game_shard_rust`  
**Location**: `services/game_shard_rust/src/shard.rs:495-584`

#### Step 4.1: Edge Calculation
- **Code**: `services/game_shard_rust/src/shard.rs:516-544`
- **Process**:
  1. Gets current market prices for home and away teams
  2. Calculates edges for both teams:
     ```rust
     home_edge = (home_win_prob - home_market_mid_price) * 100.0
     away_edge = (away_win_prob - away_market_mid_price) * 100.0
     ```
  3. **Critical**: Only emits signal for the team with **stronger edge** (absolute value)
  4. Prevents betting on both teams to win the same game

#### Step 4.2: Signal Generation Logic
- **Code**: `services/game_shard_rust/src/shard.rs:610-707`
- **Process**:
  1. Checks if game is in progress (skips pre-game, final, overtime)
  2. Validates edge threshold: `edge.abs() >= min_edge_pct` (default: 2.0%)
  3. Determines direction:
     - `edge > 0`: Model thinks team undervalued → **BUY**
     - `edge < 0`: Model thinks team overvalued → **SELL**
   
  4. Probability bounds check:
     - BUY: `model_prob <= MAX_BUY_PROB` (0.95) - avoid buying near-certain outcomes
     - SELL: `model_prob >= MIN_BUY_PROB` (0.05) - avoid selling very unlikely outcomes
   
  5. Signal debouncing: Prevents duplicate signals within `SIGNAL_DEBOUNCE_SECS` (default: 30s)

#### Step 4.3: Signal Creation
- **Code**: `services/game_shard_rust/src/shard.rs:657-682`
- **Process**:
  1. Creates `TradingSignal` with:
     - `signal_id`: UUID
     - `signal_type`: `ModelEdgeYes` or `ModelEdgeNo`
     - `game_id`, `sport`, `team`
     - `direction`: `Buy` or `Sell`
     - `model_prob`: Calculated win probability
     - `market_prob`: Market mid price
     - `edge_pct`: Absolute edge percentage
     - `confidence`: `(edge_pct.abs() / 10.0).min(1.0)` - simple confidence scaling
     - `platform_buy`: Platform to execute on (currently hardcoded to Polymarket)
     - `buy_price`: Market `yes_ask` (for BUY) or `yes_bid` (for SELL)
     - `expires_at`: 30 seconds from creation
   
  2. Publishes signal to Redis channel `signals:new`

---

### Phase 5: Signal Processing & Risk Filtering

**Service**: `signal_processor_rust`  
**Location**: `services/signal_processor_rust/src/main.rs`

#### Step 5.1: Signal Reception
- **Code**: `services/signal_processor_rust/src/main.rs:1162-1184`
- **Process**:
  1. Subscribes to Redis channel `signals:new`
  2. Parses `TradingSignal` from JSON
  3. Calls `handle_signal()` for each signal

#### Step 5.2: Pre-Trade Filtering
- **Code**: `services/signal_processor_rust/src/main.rs:786-861`
- **Process**:
  1. **Edge Threshold**: `signal.edge_pct >= config.min_edge_pct` (default: 3.5%)
  2. **Probability Bounds**:
     - BUY: `model_prob <= max_buy_prob` (0.95)
     - SELL: `model_prob >= min_sell_prob` (0.05)
  3. **Duplicate Position Check**:
     - Checks for existing open position on same game/team/side
     - Prevents doubling down (unless `ALLOW_HEDGING=true`)
  4. **Cooldown Check**:
     - Win cooldown: 180s after winning trade
     - Loss cooldown: 300s after losing trade
  5. **Rule Evaluation**:
     - Checks `trading_rules` table for custom rules
     - Rules can reject signals or override edge thresholds

#### Step 5.3: Market Price Lookup
- **Code**: `services/signal_processor_rust/src/main.rs:633-722`
- **Process**:
  1. Queries `market_prices` table for recent prices (< 2 minutes old)
  2. Uses team matching (`match_team_in_text()`) to find correct market
  3. Requires `team_match_min_confidence >= 0.7`
  4. Falls back to any recent price if no match found

#### Step 5.4: Position Size Calculation
- **Code**: `services/signal_processor_rust/src/main.rs:724-733`
- **Process**:
  1. Gets current balance from `bankroll` table
  2. Calculates Kelly fraction: `kelly = signal.kelly_fraction()` (from edge and probabilities)
  3. Applies fractional Kelly: `fractional_kelly = kelly * config.kelly_fraction` (default: 0.25)
  4. Caps at `max_position_pct` (default: 10% of balance)
  5. Minimum size: $1.00

#### Step 5.5: Risk Limit Checks
- **Code**: `services/signal_processor_rust/src/main.rs:420-523`
- **Process**:
  1. **Bankroll Sufficiency**: `proposed_size <= available_balance`
  2. **Daily Loss Limit**: `daily_loss < max_daily_loss` (default: $100)
  3. **Game Exposure**: `game_exposure + proposed_size <= max_game_exposure` (default: $50)
  4. **Sport Exposure**: `sport_exposure + proposed_size <= max_sport_exposure` (default: $200)
  5. **Opposing Position**: Checks for opposite-side position on same team (prevents flip-flopping)
  6. **Position Count**: Max 2 positions per game

#### Step 5.6: Execution Request Creation
- **Code**: `services/signal_processor_rust/src/main.rs:735-784`
- **Process**:
  1. Creates `ExecutionRequest` with:
     - `request_id`: UUID
     - `idempotency_key`: `"{game_id}_{team}_{direction}"` - prevents duplicate executions
     - `game_id`, `sport`, `platform`, `market_id`
     - `side`: `Yes` (BUY) or `No` (SELL)
     - `limit_price`: `yes_ask` (BUY) or `yes_bid` (SELL)
     - `size`: Calculated position size
     - `signal_id`, `edge_pct`, `model_prob`, `market_prob`
   
  2. Dedupe check: Verifies `idempotency_key` not already in-flight
  3. Publishes to Redis channel `execution:requests`

---

### Phase 6: Trade Execution

**Service**: `execution_service_rust`  
**Location**: `services/execution_service_rust/src/main.rs` and `services/execution_service_rust/src/engine.rs`

#### Step 6.1: Execution Request Reception
- **Code**: `services/execution_service_rust/src/main.rs:30-49`
- **Process**:
  1. Subscribes to Redis channel `execution:requests`
  2. Parses `ExecutionRequest` from JSON
  3. Calls `engine.execute(request)`

#### Step 6.2: Execution Logic
- **Code**: `services/execution_service_rust/src/engine.rs:22-138`
- **Process**:
  1. **Paper Trading Mode** (`PAPER_TRADING=1`):
     - Simulates execution immediately
     - Returns `ExecutionResult` with:
       - `status`: `Filled`
       - `order_id`: `paper-{uuid}`
       - `filled_qty`: Requested size
       - `avg_price`: Limit price
       - `fees`: 0.0 (calculated later in position tracker)
   
  2. **Live Trading Mode** (`PAPER_TRADING=0`):
     - **NOT IMPLEMENTED**: Currently returns `Rejected` with "Real execution not implemented yet"
     - TODO: Call `KalshiClient.place_order()` or `PolymarketClient.place_order()`

#### Step 6.3: Execution Result Publishing
- **Code**: `services/execution_service_rust/src/main.rs:82-101`
- **Process**:
  1. Publishes `ExecutionResult` to Redis channel `execution:results`
  2. If execution failed, publishes error notification to `notification:events`

---

### Phase 7: Position Tracking & Exit Logic

**Service**: `position_tracker_rust`  
**Location**: `services/position_tracker_rust/src/main.rs`

#### Step 7.1: Position Opening
- **Code**: `services/position_tracker_rust/src/main.rs:283-434`
- **Process**:
  1. Subscribes to Redis channel `execution:results`
  2. On `ExecutionStatus::Filled`:
     - Creates `OpenPosition` in memory
     - Inserts into `paper_trades` table with `status='open'`
     - Publishes `PositionUpdate` to `position:updates` channel
     - Publishes `TradeEntry` notification to `notification:events`

#### Step 7.2: Exit Monitoring Loop
- **Code**: `services/position_tracker_rust/src/main.rs:678-771`
- **Process**:
  1. Runs every `EXIT_CHECK_INTERVAL_SECS` (default: 1.0s)
  2. For each open position:
     - Skips if held less than `MIN_HOLD_SECONDS` (default: 10s)
     - Gets current market price from Redis cache or database
     - Validates price staleness (< 30s old)
     - Validates orderbook (not pathological: `bid > 0` or `ask < 1`, spread < 0.5)
     - Calculates mark price: `(yes_bid + yes_ask) / 2.0`
     - Evaluates exit conditions

#### Step 7.3: Exit Condition Evaluation
- **Code**: `services/position_tracker_rust/src/main.rs:773-806`
- **Process**:
  1. **Hold for Settlement**:
     - BUY position with price > 0.85: Hold (likely settling at 1.00)
     - SELL position with price < 0.15: Hold (likely settling at 0.00)
   
  2. **Take Profit**: `price_move >= take_profit_pct` (default: 3.0%)
     - BUY: `current_price - entry_price >= 0.03`
     - SELL: `entry_price - current_price >= 0.03`
   
  3. **Stop Loss**: `price_move <= -stop_loss_pct` (sport-specific, default: 5.0%)
     - BUY: `current_price - entry_price <= -0.05`
     - SELL: `entry_price - current_price <= -0.05`
   
  4. Exit debouncing: Requires `DEBOUNCE_EXIT_CHECKS` consecutive triggers (default: 0, disabled)

#### Step 7.4: Position Closing
- **Code**: `services/position_tracker_rust/src/main.rs:513-676`
- **Process**:
  1. Calculates P&L:
     - **Gross PnL**: `size * (exit_price - entry_price)` for BUY, `size * (entry_price - exit_price)` for SELL
     - **Exit Fees**: Platform-specific fee rate × exit value
     - Kalshi: 0.7% of contract value
     - Polymarket: 2.0% of contract value
     - Paper: 0.7% (simulates Kalshi)
     - **Net PnL**: `gross_pnl - entry_fees - exit_fees`
   
  2. **Piggybank Logic**: 50% of net profit goes to `piggybank_balance`, 50% to `current_balance`
   
  3. Updates `paper_trades` table:
     - `status = 'closed'`
     - `exit_price`, `exit_time`
     - `outcome`: `'win'` if `net_pnl > 0`, else `'loss'`
     - `pnl`: Net PnL (after fees)
     - `pnl_pct`: `(net_pnl / size) * 100.0`
   
  4. Updates `bankroll` table:
     - `current_balance`, `piggybank_balance`
     - `peak_balance`: Tracks all-time high
     - `trough_balance`: Tracks all-time low
   
  5. Records cooldown: `game_cooldowns[game_id] = (now, was_win)` for signal processor
   
  6. Publishes `PositionUpdate` and `TradeExit` notification

#### Step 7.5: Game End Settlement
- **Code**: `services/position_tracker_rust/src/main.rs:436-484`
- **Process**:
  1. Subscribes to Redis channel `games:ended`
  2. On game end:
     - Determines winner from final scores
     - Closes all open positions for that game:
       - Winning team: `exit_price = 1.0`
       - Losing team: `exit_price = 0.0`
     - Uses team matching to identify which positions are on which team

#### Step 7.6: Orphan Position Sweep
- **Code**: `services/position_tracker_rust/src/main.rs:866-958`
- **Process**:
  1. Runs every 5 minutes
  2. Queries database for ended games with open positions
  3. Settles orphaned positions (positions that didn't get game end event)

---

### Phase 8: Notifications

**Service**: `notification_service_rust`  
**Location**: `services/notification_service_rust/src/main.rs`

#### Step 8.1: Notification Reception
- **Code**: `services/notification_service_rust/src/main.rs:122-167`
- **Process**:
  1. Subscribes to Redis channel `notification:events`
  2. Parses `NotificationEvent` from JSON
  3. Applies filtering (quiet hours, rate limiting, priority filtering)

#### Step 8.2: Notification Filtering
- **Code**: `services/notification_service_rust/src/filters.rs`
- **Process**:
  1. **Quiet Hours**: Suppresses notifications during configured hours (default: disabled)
  2. **Rate Limiting**: Limits notifications per minute (default: 60/min)
  3. **Priority Filtering**: Can filter by priority level (Info, Warning, Error)

#### Step 8.3: Message Formatting
- **Code**: `services/notification_service_rust/src/formatters.rs`
- **Process**:
  1. Formats notification based on event type:
     - **TradeEntry**: "🟢 TRADE ENTRY\n{sport} {game_id}\n{team} ({side}) @ {price}\nsize=${size} edge={edge}%"
     - **TradeExit**: "💰/📉 TRADE EXIT\n{sport} {game_id}\n{team}\npnl={pnl} ({pnl_pct}%)\nentry={entry} exit={exit}"
     - **RiskRejection**: "🛑 RISK REJECTION\n{game_id} {team}\nedge={edge}% size=${size}\nreason={reason}"
     - **Error**: "⚠️ ERROR\nservice={service}\n{message}"
   
  2. Sends formatted message via Signal API to configured recipients

---

## Edge Detection Algorithms

### Model Edge Detection

**Location**: `services/game_shard_rust/src/shard.rs:621-624`

```rust
let edge_pct = (model_prob - market_prob) * 100.0;
```

**Logic**:
- **Positive Edge (BUY)**: Model probability > Market price
  - Example: Model says 60% chance, market prices at 55% → 5% edge → BUY
- **Negative Edge (SELL)**: Model probability < Market price
  - Example: Model says 40% chance, market prices at 45% → 5% edge → SELL

**Why It Works**:
- ESPN updates faster than prediction markets can adjust
- Win probability model incorporates real-time game state (scores, possession, field position)
- Markets lag behind live events, creating temporary mispricing

### Cross-Market Arbitrage Detection

**Location**: `rust_core/src/lib.rs:53-121`

**Logic**:
```rust
// Strategy 1: Buy YES on Platform A + Buy NO on Platform B
no_ask_b = 1.0 - market_b.yes_bid;  // Buying NO = selling YES at bid
total_cost_1 = market_a.yes_ask + no_ask_b;

if total_cost_1 < 1.0 {
    profit = 1.0 - total_cost_1;
    edge_pct = profit * 100.0;
    // Arbitrage opportunity found!
}
```

**Example**:
- Kalshi YES ask: $0.48, YES bid: $0.50
- Polymarket YES ask: $0.54, YES bid: $0.52
- Strategy: Buy Kalshi YES @ $0.48 + Buy Polymarket NO @ $0.48 (from $0.52 bid)
- Total cost: $0.48 + $0.48 = $0.96
- Profit: $1.00 - $0.96 = $0.04 (4% edge)

**Note**: This is currently detected but not actively traded in the main flow (only model edge signals are executed).

---

## Key Configuration Parameters

### Edge Detection
- `MIN_EDGE_PCT`: 2.0% (game_shard) / 3.5% (signal_processor) - Minimum edge to generate/execute signal
- `MAX_BUY_PROB`: 0.95 - Don't buy near-certain outcomes
- `MIN_SELL_PROB`: 0.05 - Don't sell very unlikely outcomes

### Position Sizing
- `KELLY_FRACTION`: 0.25 - Fractional Kelly criterion (25% of full Kelly)
- `MAX_POSITION_PCT`: 10.0% - Maximum position size as % of balance
- `INITIAL_BANKROLL`: $1000.0 - Starting balance

### Risk Limits
- `MAX_DAILY_LOSS`: $100.0 - Stop trading if daily losses exceed this
- `MAX_GAME_EXPOSURE`: $50.0 - Maximum exposure per game
- `MAX_SPORT_EXPOSURE`: $200.0 - Maximum exposure per sport

### Exit Logic
- `TAKE_PROFIT_PCT`: 3.0% - Exit when profit reaches this threshold
- `DEFAULT_STOP_LOSS_PCT`: 5.0% - Default stop loss (sport-specific overrides exist)
- `MIN_HOLD_SECONDS`: 10.0 - Minimum time to hold position before exit check
- `EXIT_CHECK_INTERVAL_SECS`: 1.0 - How often to check exit conditions

### Cooldowns
- `WIN_COOLDOWN_SECONDS`: 180 - Wait 3 minutes after winning trade
- `LOSS_COOLDOWN_SECONDS`: 300 - Wait 5 minutes after losing trade
- `SIGNAL_DEBOUNCE_SECS`: 30 - Prevent duplicate signals within 30s

---

## Data Flow Summary

### Redis Channels

| Channel | Publisher | Subscriber | Purpose |
|---------|-----------|------------|---------|
| `discovery:requests` | orchestrator | market_discovery | Request market ID lookup |
| `discovery:results` | market_discovery | orchestrator | Return market IDs |
| `shard:{id}:command` | orchestrator | game_shard | Assign/remove games |
| `shard:{id}:heartbeat` | game_shard | orchestrator | Health monitoring |
| `game:{id}:state` | game_shard | (monitoring) | Live game state |
| `game:{id}:price` | polymarket_monitor | game_shard, position_tracker | Market prices |
| `signals:new` | game_shard | signal_processor | Trading signals |
| `execution:requests` | signal_processor | execution_service | Trade execution requests |
| `execution:results` | execution_service | position_tracker | Execution results |
| `position:updates` | position_tracker | (monitoring) | Position state changes |
| `games:ended` | (orchestrator?) | position_tracker | Game end events |
| `notification:events` | signal_processor, position_tracker, execution_service | notification_service | Notifications |
| `health:heartbeats` | All services | (monitoring) | Service health |

### Database Tables

| Table | Purpose | Key Fields |
|-------|---------|------------|
| `games` | Game metadata | `game_id`, `sport`, `home_team`, `away_team` |
| `game_states` | Time-series game snapshots | `game_id`, `time`, `home_score`, `away_score`, `home_win_prob` |
| `market_prices` | Time-series price history | `market_id`, `platform`, `time`, `yes_bid`, `yes_ask` |
| `paper_trades` | Trade records | `trade_id`, `game_id`, `side`, `entry_price`, `exit_price`, `pnl` |
| `bankroll` | Account balance | `current_balance`, `piggybank_balance`, `peak_balance` |
| `trading_signals` | Signal history | `signal_id`, `edge_pct`, `model_prob`, `market_prob` |
| `trading_rules` | Custom trading rules | `rule_id`, `conditions`, `action` |

---

## Issues & Discrepancies Found

### 🔴 Critical Issues

1. **Live Trading Not Implemented**
   - **Location**: `services/execution_service_rust/src/engine.rs:54-110`
   - **Issue**: Real execution returns `Rejected` with "Real execution not implemented yet"
   - **Impact**: System only works in paper trading mode
   - **Fix Required**: Implement `KalshiClient.place_order()` and `PolymarketClient.place_order()`

2. **Hardcoded Platform**
   - **Location**: `services/game_shard_rust/src/shard.rs:668`
   - **Issue**: `platform_buy` is hardcoded to `Platform::Polymarket`
   - **Impact**: Cannot trade on Kalshi even if it has better prices
   - **Fix Required**: Select platform based on market prices or configuration

3. **Liquidity Not Checked**
   - **Location**: `services/game_shard_rust/src/shard.rs:672`
   - **Issue**: `liquidity_available` is hardcoded to `10000.0` with TODO comment
   - **Impact**: May attempt to trade on markets with insufficient liquidity
   - **Fix Required**: Get actual liquidity from market data

4. **Cross-Market Arbitrage Not Executed**
   - **Location**: `rust_core/src/lib.rs:53-121`
   - **Issue**: Arbitrage detection exists but signals are not generated for it
   - **Impact**: Missing profitable opportunities
   - **Fix Required**: Add arbitrage signal generation in `game_shard_rust`

### 🟡 Medium Issues

5. **Team Matching Confidence Threshold Mismatch**
   - **Location**: `services/signal_processor_rust/src/main.rs:687` vs `services/game_shard_rust/src/shard.rs:592-608`
   - **Issue**: Signal processor requires `>= 0.7` confidence, but game_shard uses fuzzy matching without threshold
   - **Impact**: May generate signals for wrong teams
   - **Fix Required**: Standardize team matching logic

6. **Price Staleness Check Inconsistent**
   - **Location**: `services/signal_processor_rust/src/main.rs:647` (2 minutes) vs `services/position_tracker_rust/src/main.rs:699` (30 seconds)
   - **Issue**: Different staleness thresholds
   - **Impact**: May use stale prices for execution
   - **Fix Required**: Standardize to single threshold

7. **Exit Price Calculation**
   - **Location**: `services/position_tracker_rust/src/main.rs:724-728`
   - **Issue**: Uses `yes_bid` for BUY exits (selling) but doesn't account for slippage
   - **Impact**: Actual exit price may be worse than calculated
   - **Fix Required**: Add slippage buffer or use more conservative prices

8. **Fee Calculation Inconsistency**
   - **Location**: `services/execution_service_rust/src/engine.rs:38` (0.0) vs `services/position_tracker_rust/src/main.rs:532-538` (calculated)
   - **Issue**: Entry fees not calculated in execution service
   - **Impact**: P&L calculations may be inaccurate
   - **Fix Required**: Calculate fees at execution time

### 🟢 Minor Issues

9. **Overtime Detection**
   - **Location**: `services/game_shard_rust/src/shard.rs:507-510`
   - **Issue**: Overtime detection may not work for all sports correctly
   - **Impact**: May skip signals during overtime when they could be profitable
   - **Fix Required**: Review overtime logic per sport

10. **Signal Expiration**
    - **Location**: `services/game_shard_rust/src/shard.rs:680`
    - **Issue**: Signals expire after 30 seconds but processing may take longer
    - **Impact**: Valid signals may be rejected as expired
    - **Fix Required**: Increase expiration or check expiration at execution time

11. **Database Connection Pooling**
    - **Location**: Multiple services
    - **Issue**: Each service creates its own pool with `max_connections=5`
    - **Impact**: May exhaust database connections under load
    - **Fix Required**: Use connection pooler (PgBouncer) or increase limits

12. **Error Handling**
    - **Location**: Multiple services
    - **Issue**: Many errors are logged but not propagated or handled gracefully
    - **Impact**: Services may continue in degraded state
    - **Fix Required**: Add circuit breakers and better error recovery

---

## Performance Considerations

### Latency Critical Path

1. **ESPN Poll → Signal Generation**: ~1-2 seconds
   - ESPN API call: ~200-500ms
   - Win probability calculation: <1ms
   - Edge calculation: <1ms
   - Signal generation: <1ms

2. **Signal → Execution**: ~100-500ms
   - Signal processing: ~50-200ms (DB queries)
   - Risk checks: ~50-200ms (multiple DB queries)
   - Execution: ~50-100ms (paper) / ~500-2000ms (live)

3. **Total Latency**: ~1.5-3 seconds from game event to trade execution

### Optimization Opportunities

1. **Batch DB Queries**: Combine multiple queries in signal processor
2. **Redis Caching**: Cache market prices and game states in Redis
3. **Parallel Processing**: Process multiple games concurrently
4. **SIMD Optimization**: Already implemented in `rust_core/src/simd/` for batch arbitrage scanning

---

## Testing Recommendations

1. **Unit Tests**: Win probability calculations, edge detection logic
2. **Integration Tests**: End-to-end signal generation → execution → position tracking
3. **Load Tests**: Multiple games, high-frequency price updates
4. **Edge Case Tests**: Overtime, game cancellations, market closures
5. **Paper Trading**: Extensive paper trading before live deployment

---

## Conclusion

The Arbees system is a sophisticated edge trading bot that leverages information latency between live sports events and prediction markets. The core flow is:

1. **Discover** games from ESPN
2. **Monitor** game state and market prices
3. **Calculate** win probabilities faster than markets
4. **Detect** edge when model differs from market
5. **Filter** signals through risk limits
6. **Execute** trades on approved signals
7. **Track** positions and exit on profit/loss thresholds
8. **Notify** via Signal messaging

The system is currently operational in paper trading mode but requires implementation of live trading execution before production deployment.
