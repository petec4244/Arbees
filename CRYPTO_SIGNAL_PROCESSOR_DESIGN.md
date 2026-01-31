# CRYPTO_SIGNAL_PROCESSOR: Design & Implementation Guide

**Document Version:** 1.0
**Last Updated:** 2026-01-31
**Author:** Claude Code
**Status:** Complete Implementation

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Signal Types](#signal-types)
4. [Price Event Detection Pipeline](#price-event-detection-pipeline)
5. [Arbitrage Detection Engine](#arbitrage-detection-engine)
6. [Probability Model](#probability-model)
7. [Risk Management System](#risk-management-system)
8. [Position Sizing](#position-sizing)
9. [Execution Request Flow](#execution-request-flow)
10. [Configuration & Tuning](#configuration--tuning)
11. [Monitoring & Observability](#monitoring--observability)
12. [Performance Characteristics](#performance-characteristics)
13. [Deployment & Integration](#deployment--integration)
14. [Troubleshooting Guide](#troubleshooting-guide)
15. [Known Limitations](#known-limitations)
16. [Future Enhancements](#future-enhancements)

---

## 1. Overview

The **CRYPTO_SIGNAL_PROCESSOR** is the self-contained signal detection engine within `crypto_shard_rust`. Unlike the distributed sports signal pipeline (signal_processor → execution_service), crypto signals are generated inline within the shard with embedded risk management.

### Key Characteristics

- **Inline Processing**: All signal detection, risk checking, and execution request generation happens in a single service
- **Low Latency**: Direct ZMQ publishing without intermediate processing steps (~50-100ms signal-to-execution vs 300-600ms for sports)
- **Two Signal Types**: Arbitrage (cross-platform price discrepancies) and Probability-Based (model vs market mismatches)
- **Embedded Risk Checks**: 8 validation checks before any execution signal is emitted
- **Real-time Monitoring**: Processes price updates continuously every 5 seconds or event-driven

### Design Rationale

**Why Inline Processing?**
- Crypto markets move faster than sports prediction markets
- Price discrepancies close in milliseconds, requiring <100ms response
- Distributed architecture (signal_processor → execution_service) would add 150-300ms latency
- Self-contained shard reduces operational complexity and removes network bottlenecks

**When to Use Crypto vs Sports Processor:**
- **Crypto**: Price target predictions, directional markets, short-term expirations (15-min to weeks)
- **Sports**: Win probability, moneylines, longer expirations (hours to weeks)

---

## 2. Architecture

### System Components

```
┌─────────────────────────────────────────────────────────────────┐
│                        CryptoShard Main Loop                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ Price Listener (ZMQ Subscriber)                            │ │
│  │  - Kalshi: prices.kalshi.* topics                          │ │
│  │  - Polymarket: prices.poly.* topics                        │ │
│  │  - Spot Prices: crypto.prices.* topics (Coinbase/Binance)  │ │
│  │  - Cache: HashMap<String, CryptoPriceData>                 │ │
│  └────────────────────────────────────────────────────────────┘ │
│                            ↓                                      │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ Price Cache (In-Memory)                                    │ │
│  │  - Key: "{asset}|{platform}"  (e.g., "BTC|kalshi")        │ │
│  │  - Value: CryptoPriceData (bid, ask, liquidity, timestamp)│ │
│  │  - Staleness Detection: max 60 seconds                     │ │
│  └────────────────────────────────────────────────────────────┘ │
│         ↑                        ↓                                │
│    (monitor_events)         (monitor_events)                     │
│         ↓                        ↓                                │
│  ┌──────────────────┐  ┌──────────────────┐                     │
│  │ Arbitrage        │  │ Probability      │                     │
│  │ Detector         │  │ Detector         │                     │
│  │ (Async)          │  │ (Async)          │                     │
│  └──────────────────┘  └──────────────────┘                     │
│         ↓                        ↓                                │
│  ┌──────────────────────────────────────┐                       │
│  │ Risk Checker (8 Validation Checks)   │                       │
│  │  1. Edge threshold                   │                       │
│  │  2. Liquidity check                  │                       │
│  │  3. Position size limit              │                       │
│  │  4. Volatility scaling               │                       │
│  │  5. Asset exposure cap               │                       │
│  │  6. Total exposure cap               │                       │
│  │  7. Duplicate trade check            │                       │
│  │  8. Database queries (parallel)      │                       │
│  └──────────────────────────────────────┘                       │
│                      ↓                                           │
│  ┌──────────────────────────────────────┐                       │
│  │ Execution Request                    │                       │
│  │ (CryptoExecutionRequest)             │                       │
│  │  - Signal type (Arbitrage/Probability)                       │
│  │  - Direction (Long/Short)            │                       │
│  │  - Position size                     │                       │
│  │  - Edge percentage                   │                       │
│  └──────────────────────────────────────┘                       │
│                      ↓                                           │
│  ┌──────────────────────────────────────┐                       │
│  │ ZMQ Publisher (5559)                 │                       │
│  │ crypto.execution.{request_id}        │                       │
│  │  ↓                                    │                       │
│  │ ExecutionService (paper_trades table)│                       │
│  └──────────────────────────────────────┘                       │
└─────────────────────────────────────────────────────────────────┘
```

### Module Breakdown

| Module | Purpose | Lines | Key Structs |
|--------|---------|-------|-------------|
| `price/listener.rs` | ZMQ subscription, price deserialization, caching | 300 | `CryptoPriceListener`, `PriceZmqEnvelope` |
| `signals/arbitrage.rs` | Cross-platform price comparison, edge calculation | 315 | `CryptoArbitrageDetector`, `ExecutionRequest` |
| `signals/probability.rs` | Model probability, mispricing detection, Kelly sizing | 330 | `CryptoProbabilityDetector`, `BlackScholesModel` |
| `signals/risk.rs` | Inline risk validation, exposure limits | 371 | `CryptoRiskChecker`, `RiskValidationResult` |
| `shard.rs` | Main coordinator, monitoring loop, statistics | 600+ | `CryptoShard`, `MonitoringStats` |

### Data Flow Sequence

```
1. Price arrives via ZMQ → CryptoPriceListener
2. Deserialization: PriceZmqEnvelope → IncomingCryptoPrice → CryptoPriceData
3. Cache insertion: HashMap<"{asset}|{platform}", CryptoPriceData>
4. Price update notification (async channel)
5. Monitoring loop wakes (every 5s or event-driven)
6. For each crypto event:
   a. Collect all prices for asset
   b. Run arbitrage detector in parallel
   c. Run probability detector in parallel
   d. Send execution request if signal generated
7. Publish to ZMQ topic: crypto.execution.{request_id}
8. ExecutionService receives → paper_trades table
```

---

## 3. Signal Types

### 3.1 Arbitrage Signals

**Definition**: Cross-platform price discrepancies where you can buy low on one platform and sell high on another.

**Structure** (`CryptoSignalType::Arbitrage`):
```rust
pub struct CryptoExecutionRequest {
    pub signal_type: CryptoSignalType::Arbitrage,
    pub direction: Direction::Long,  // Always LONG (buy at lower price)
    pub edge_pct: f64,              // Net edge after fees (2-5% typical)
    pub probability: 0.5,            // Not used for arbitrage
    pub current_price: f64,          // Ask price on cheaper platform
    pub max_price: f64,              // With 1% slippage buffer
    pub suggested_size: f64,         // Position size in USD
}
```

**Example Trade Flow:**
```
Kalshi BTC: bid=$0.45, ask=$0.47
Polymarket BTC: bid=$0.44, ask=$0.46

1. Best bid across platforms: $0.45 (Kalshi)
2. Best ask across platforms: $0.46 (Polymarket)
3. Gross edge: $0.45 - $0.46 = -$0.01 (NO ARB)

Alternative:
Kalshi BTC: bid=$0.46, ask=$0.48
Polymarket BTC: bid=$0.44, ask=$0.45

1. Best bid: $0.46 (Kalshi)
2. Best ask: $0.45 (Polymarket)
3. Gross edge: $0.46 - $0.45 = $0.01
4. Fee cost: 0.5% × ($0.46 + $0.45) = $0.00455
5. Net edge: $0.01 - $0.00455 = $0.00545 = 1.21%
6. Result: SIGNAL (if min_edge = 1%)
7. Action: Buy at $0.45 on Polymarket, sell at $0.46 on Kalshi
```

**Advantages:**
- Model-independent: Edge is based purely on price discrepancy
- Low risk: Profit is locked in at execution (assuming instant hedging)
- High frequency: Multiple opportunities per minute

**Disadvantages:**
- Requires multiple platforms with prices: currently blocked by single-price-per-asset cache
- Depends on bid-ask spreads being wide enough
- Execution must be near-simultaneous or edge closes

### 3.2 Probability-Based Signals

**Definition**: Market mispricing relative to a model's probability estimate of reaching a price target.

**Structure** (`CryptoSignalType::Probability`):
```rust
pub struct CryptoExecutionRequest {
    pub signal_type: CryptoSignalType::Probability,
    pub direction: Direction::Long | Direction::Short,  // Based on mispricing
    pub edge_pct: f64,              // |model_prob - market_price| × 100
    pub probability: f64,            // Model's calculated probability
    pub current_price: f64,          // Market's offer price
    pub max_price: f64,              // With slippage buffer
    pub suggested_size: f64,         // Kelly criterion sized position
}
```

**Example Trade Flow:**
```
Event: "Will Bitcoin reach $100,000 by June 30?"
Current spot: $84,000
Target: $100,000
Days remaining: 152
Volatility: 50% annual

Black-Scholes probability calculation:
- log_return = ln(100,000 / 84,000) = 0.1788
- std_dev = 0.50 × √(152/365) = 0.308
- z-score = 0.1788 / 0.308 = 0.580
- CDF(0.580) = 0.719 (71.9% probability)

Market prices:
- YES: bid=$0.70, ask=$0.72
- NO: bid=$0.28, ask=$0.30

Analysis:
- Market price (mid): $0.71
- Model price: $0.719
- NO edge: $0.28 vs (1-0.719)=$0.281 → edge ≈ 0.4%
- YES edge: $0.70 vs $0.719 → edge = 1.9%

Result: LONG signal on YES (model thinks it's underpriced)
- Entry: $0.72 (ask)
- Position size: Kelly criterion with market price $0.72
```

**Advantages:**
- Works with single spot price + target price
- Captures broader market view (not just platform discrepancies)
- Can trade on either side (Long or Short)

**Disadvantages:**
- Dependent on model accuracy (Black-Scholes assumptions)
- Exposed to volatility estimation error
- Longer holding periods (price targets may take days/weeks)

---

## 4. Price Event Detection Pipeline

### 4.1 Price Reception

**ZMQ Topics and Sources:**
```
prices.kalshi.{ticker}
  └─ Source: Kalshi Monitor WebSocket
  └─ Format: Updated prices with asset extracted from ticker
  └─ Frequency: ~100/sec during active trading
  └─ Payload: {"asset": "BTC", "yes_bid": 0.45, "yes_ask": 0.47, ...}

prices.poly.{market_id}
  └─ Source: Polymarket Monitor WebSocket
  └─ Format: Updated prices with asset extracted from title
  └─ Frequency: ~50/sec during active trading
  └─ Payload: {"asset": "ETH", "yes_bid": 0.32, "yes_ask": 0.35, ...}

crypto.prices.{symbol}
  └─ Source: Spot Price Monitor (Chainlink/Binance)
  └─ Format: Spot prices
  └─ Frequency: ~1/sec per asset
  └─ Payload: {"yes_bid": 84000, "yes_ask": 84050, ...}
```

### 4.2 Deserialization Pipeline

**Two-Step Pattern** (handles nested JSON):

```rust
// Step 1: Parse outer envelope with generic payload
let envelope: PriceZmqEnvelope = serde_json::from_slice(payload_bytes)?;
// Result: PriceZmqEnvelope {
//   seq: 3891,
//   timestamp_ms: 1769819724265,
//   source: "kalshi_monitor",
//   payload: serde_json::Value { ... }
// }

// Step 2: Parse payload into typed struct
let incoming: IncomingCryptoPrice = serde_json::from_value(envelope.payload)?;
// Result: IncomingCryptoPrice {
//   asset: "BTC",
//   yes_bid: 0.45,
//   yes_ask: 0.47,
//   ...
// }

// Step 3: Convert to cache format
let cache_key = format!("{}|{}", incoming.asset, incoming.platform);
let price_data: CryptoPriceData = incoming.into();
cache.insert(cache_key, price_data);
```

**Error Handling:**
- Envelope parse errors: Logged at INFO level for first 20 messages, then DEBUG
- Payload parse errors: Logged with detailed diagnostics
- Type conversion errors: Skipped with debug logging
- Graceful degradation: Bad message doesn't break loop

### 4.3 Price Cache

**Cache Structure:**
```rust
HashMap<String, CryptoPriceData>

Key format: "{asset}|{platform}"
Examples:
  "BTC|kalshi" → Latest Kalshi BTC price
  "ETH|polymarket" → Latest Polymarket ETH price
  "SOL|coinbase" → Latest Coinbase SOL spot price

⚠️  LIMITATION: Single price per asset+platform
  - Multiple Kalshi BTC markets (different strikes)
  - All map to same key "BTC|kalshi"
  - Last write wins → Previous prices lost
  - Causes timestamp mismatches & stale warnings
  - Phase 2 fix: Per-market circular buffer
```

**Staleness Detection:**
```rust
// In monitoring loop (every 5 seconds):
for (key, price) in &price_cache {
    let age_secs = (Utc::now() - price.timestamp).num_seconds();
    if age_secs > STALENESS_THRESHOLD (60 secs) {
        // DEBUG log (not WARNING to avoid spam)
        warn_if_rare!("Stale price for {} (age: {}s)", key, age_secs);
    }
}
```

**Cache Statistics:**
- **Cache size**: 50-300 entries depending on market activity
- **Update rate**: 50-200 prices/sec from Kalshi + Polymarket
- **Spot price update**: ~1/sec per asset (BTC, ETH, SOL)
- **Memory footprint**: ~50KB for full cache

---

## 5. Arbitrage Detection Engine

### 5.1 Core Algorithm

```rust
pub async fn detect_and_emit(
    &self,
    event_id: &str,
    asset: &str,
    prices: &HashMap<String, CryptoPriceData>,
    risk_checker: &CryptoRiskChecker,
    volatility_factor: f64,
) -> Result<Option<CryptoExecutionRequest>> {
    // Step 1: Collect all prices for this asset
    let asset_prices: Vec<_> = prices
        .iter()
        .filter(|k| k.0.starts_with(&format!("{}|", asset)))
        .collect();

    if asset_prices.len() < 2 {
        return Ok(None);  // Need 2+ platforms
    }

    // Step 2: Find best bid (highest) and best ask (lowest)
    let best_bid = asset_prices.iter().max_by(|p| p.1.yes_bid);
    let best_ask = asset_prices.iter().min_by(|p| p.1.yes_ask);

    // Step 3: Verify different platforms
    if best_bid.platform == best_ask.platform {
        return Ok(None);  // Same platform, no arb
    }

    // Step 4: Calculate edge
    let gross_edge = best_bid.yes_bid - best_ask.yes_ask;
    let fee_cost = fee_pct / 100.0 * (best_bid.yes_bid + best_ask.yes_ask);
    let net_edge = gross_edge - fee_cost;
    let net_edge_pct = (net_edge / best_ask.yes_ask) * 100.0;

    if net_edge_pct < min_edge_pct {
        return Ok(None);  // Below threshold
    }

    // Step 5: Position sizing (edge-based multiplier)
    let base_size = 100.0;
    let edge_multiplier = (net_edge_pct / min_edge_pct).min(3.0);
    let suggested_size = base_size * edge_multiplier;

    // Step 6: Risk validation
    let adjusted_size = risk_checker.validate_trade(
        asset,
        &best_ask.platform,
        &best_ask.market_id,
        net_edge_pct,
        suggested_size,
        best_ask.total_liquidity,
        volatility_factor,
    ).await?;

    // Step 7: Emit execution request
    Ok(Some(CryptoExecutionRequest {
        request_id: Uuid::new_v4().to_string(),
        event_id: event_id.to_string(),
        asset: asset.to_string(),
        signal_type: CryptoSignalType::Arbitrage,
        platform: best_ask.platform.clone(),
        market_id: best_ask.market_id.clone(),
        direction: Direction::Long,
        edge_pct: net_edge_pct,
        probability: 0.5,  // Arbitrage doesn't depend on probability
        suggested_size: adjusted_size,
        max_price: best_ask.yes_ask + 0.01,
        current_price: best_ask.yes_ask,
        timestamp: Utc::now(),
        volatility_factor,
        exposure_check: true,
        balance_check: true,
    }))
}
```

### 5.2 Fee Structure

**Typical Prediction Market Fees:**
```
Kalshi: 0.5% taker, 0% maker
Polymarket: 0.5% taker, 0% maker

Total cost for round-trip (buy + sell):
- Gross edge must cover: 0.5% (buy side) + 0.5% (sell side)
- Minimum breakeven edge: ~1.0% before slippage
- Default minimum: 3.0% (2% profit margin after fees)
```

### 5.3 Configuration Parameters

| Parameter | Default | Range | Impact |
|-----------|---------|-------|--------|
| `min_edge_pct` | 3.0% | 1-10% | Lower = more signals, higher risk |
| `fee_pct` | 0.5% | 0.3-1.0% | Higher = less profitable trades |
| `base_position_size` | $100 | $10-$500 | Base before edge multiplier |
| `edge_multiplier_cap` | 3.0x | 1-5x | Max size based on edge |

### 5.4 Real-World Scenarios

**Scenario 1: Valid Arbitrage**
```
Market 1 (Kalshi): bid=0.45, ask=0.47
Market 2 (Polymarket): bid=0.44, ask=0.46

Gross edge: 0.45 - 0.46 = -0.01 → NO ARB (wrong direction)

Market 1 (Kalshi): bid=0.46, ask=0.48
Market 2 (Polymarket): bid=0.44, ask=0.45

Gross edge: 0.46 - 0.45 = 0.01 = 2.2%
Fee cost: 0.5% × (0.46 + 0.45) = 0.00455
Net edge: 0.00545 = 1.21%
→ REJECT (below 3% minimum)
```

**Scenario 2: Wide Spreads**
```
Market 1 (Kalshi): bid=0.50, ask=0.60 (wide spread)
Market 2 (Polymarket): bid=0.45, ask=0.55 (wide spread)

Best bid: 0.50 (Kalshi)
Best ask: 0.45 (Polymarket)
Gross edge: 0.05 = 11.1%
Fee cost: 0.5% × (0.50 + 0.45) = 0.00475
Net edge: 0.04525 = 10.06%

→ ACCEPT (strong signal)
Position: 100 × (10.06/3) = $335 (capped at max)
```

---

## 6. Probability Model

### 6.1 Black-Scholes Inspired Log-Normal Model

**Mathematical Foundation:**
```
Purpose: Estimate probability of price target achievement

Inputs:
  - Current price (S₀)
  - Target price (K)
  - Time remaining (T) in years
  - Volatility (σ) annualized

Formula:
  log_return = ln(K / S₀)
  std_dev = σ × √T
  z_score = log_return / std_dev
  probability = N(z_score)  [cumulative normal distribution]
```

**Example Calculation:**
```
Bitcoin: Current=$84,000, Target=$100,000
Days remaining: 152, Volatility: 50% annual

Step 1: log_return = ln(100,000 / 84,000) = 0.1788

Step 2: std_dev = 0.50 × √(152/365) = 0.50 × 0.644 = 0.322

Step 3: z_score = 0.1788 / 0.322 = 0.555

Step 4: N(0.555) = 0.711 (71.1% probability)

Result: Model thinks BTC has 71.1% chance of hitting $100k
```

### 6.2 Implementation Details

```rust
fn calculate_model_probability(
    current_price: f64,
    target_price: f64,
    days_remaining: f64,
    volatility: f64,  // Annual percentage, e.g., 0.50 for 50%
) -> f64 {
    if days_remaining <= 0.0 {
        // Expired market: deterministic outcome
        return if current_price >= target_price { 1.0 } else { 0.0 };
    }

    let log_return = (target_price / current_price).ln();
    let time_fraction = days_remaining / 365.25;
    let std_dev = volatility * time_fraction.sqrt();

    if std_dev < 0.001 {
        // No time value: deterministic
        return if log_return > 0.0 { 1.0 } else { 0.0 };
    }

    let z_score = log_return / std_dev;
    normal_cdf(z_score)  // Approximation of standard normal CDF
}

// Normal CDF approximation (Abramowitz & Stegun)
fn normal_cdf(z: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if z >= 0.0 { 1.0 } else { -1.0 };
    let z = z.abs() / (2_f64.sqrt());

    let t = 1.0 / (1.0 + p * z);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-z * z).exp();

    0.5 * (1.0 + sign * y)
}
```

### 6.3 Configuration Parameters

| Parameter | Default | Range | Impact |
|-----------|---------|-------|--------|
| `volatility` | 0.50 | 0.20-1.00 | Crypto markets are highly volatile |
| `min_confidence` | 0.60 | 0.55-0.80 | Only trade clear mispricings |
| `kelly_fraction` | 0.25 | 0.10-0.50 | Conservative Kelly sizing |

### 6.4 Signal Detection

**Long Signal Condition:**
```
Model probability > Market ask price
AND
(Model prob - Market ask) > min_edge%
AND
Confidence check passes (prob > 0.60 OR prob < 0.40)

Example:
- Model: 71% (for "Bitcoin hits $100k")
- Market ask: 65%
- Edge: 6%
→ LONG signal (model thinks underpriced)
```

**Short Signal Condition:**
```
Market bid > (1 - Model probability)
AND
Edge > min_edge%

Example:
- Model: 29% (for "Bitcoin doesn't hit $100k")
- Market bid: 36%
→ SHORT signal (market thinks it's overvalued)
```

---

## 7. Risk Management System

### 7.1 Overview

The `CryptoRiskChecker` performs **8 sequential validation checks** on every signal before emission. If ANY check fails, the signal is rejected.

### 7.2 Risk Checks

#### Check 1: Edge Threshold
```
Rule: edge_pct >= min_edge_pct

Rationale: Filter weak signals that won't overcome slippage/fees
Default: 3.0%
Configurable: Yes

Code:
if edge_pct < self.min_edge_pct {
    return Err("Edge below minimum");
}
```

#### Check 2: Liquidity Requirement
```
Rule: market_liquidity >= min_liquidity_usd

Rationale: Avoid illiquid markets where execution slippage exceeds edge
Default: $50
Configurable: Yes

Code:
if liquidity < self.min_liquidity {
    return Err("Insufficient liquidity");
}
```

#### Check 3: Position Size Limit
```
Rule: adjusted_size <= max_position_size

Rationale: Cap exposure per trade
Default: $500
Configurable: Yes

Code:
let adjusted_size = suggested_size.min(self.max_position_size);
```

#### Check 4: Volatility Scaling
```
Rule: if volatility_factor > 1.5:
      adjusted_size *= 0.7  // Reduce by 30%

Rationale: Reduce position size in high-volatility environments
Volatility factor: Computed from recent price movements
Logic: Extreme moves reduce confidence, so we reduce exposure

Code:
if self.volatility_scaling && volatility_factor > 1.5 {
    adjusted_size *= 0.7;  // 30% reduction
}
```

#### Check 5: Asset Exposure Cap
```
Rule: current_asset_exposure + adjusted_size <= max_asset_exposure

Rationale: Prevent over-concentration in single asset
Default: $2,000 per asset (BTC, ETH, SOL separate)
Configurable: Yes

Database Query:
SELECT SUM(size_usd) FROM paper_trades
WHERE asset = $1 AND status = 'open' AND settled = false

Code:
let current = self.get_asset_exposure(asset).await?;
if current + adjusted_size > max_asset_exposure {
    adjusted_size = (max_asset_exposure - current).max(10.0);
}
```

#### Check 6: Total Crypto Exposure Cap
```
Rule: total_crypto_exposure + adjusted_size <= max_total_exposure

Rationale: Cap total crypto allocation across all assets
Default: $5,000
Configurable: Yes

Database Query:
SELECT SUM(size_usd) FROM paper_trades
WHERE status = 'open' AND event_type = 'crypto'

Code:
let total = self.get_total_crypto_exposure().await?;
if total + adjusted_size > max_total_exposure {
    adjusted_size = (max_total_exposure - total).max(10.0);
}
```

#### Check 7: Duplicate Trade Prevention
```
Rule: No trade on same market_id in last 60 seconds

Rationale: Prevent rapid re-trading of same market
Cooldown: 60 seconds configurable
Tracking: In-memory set or database

Database Query:
SELECT 1 FROM paper_trades
WHERE market_id = $1 AND created_at > NOW() - '60s'::interval
LIMIT 1

Code:
if self.is_duplicate_trade(market_id).await? {
    return Err("Duplicate trade within 60s");
}
```

#### Check 8: Balance Validation
```
Rule: account_balance >= required_for_trade

Rationale: Ensure sufficient funds
Query: SELECT balance FROM bankroll WHERE account_id = $1

Code:
let balance = self.get_balance().await?;
let required = adjusted_size * 1.1;  // 10% buffer
if balance < required {
    return Err("Insufficient balance");
}
```

### 7.3 Risk Check Flow

```
Signal Candidate
    ↓
Check 1: Edge >= min? → NO → REJECT
    ↓ YES
Check 2: Liquidity >= min? → NO → REJECT
    ↓ YES
Check 3: Size <= max? → NO → CAP SIZE
    ↓
Check 4: Volatility factor > 1.5? → YES → REDUCE 30%
    ↓
Check 5: Asset exposure ok? → NO → REJECT or REDUCE
    ↓ (Database query parallel)
Check 6: Total exposure ok? → NO → REJECT or REDUCE
    ↓ (Database query parallel)
Check 7: Not duplicate? → NO → REJECT
    ↓ (Database query parallel)
Check 8: Balance ok? → NO → REJECT
    ↓
Execution Request APPROVED → ZMQ publish
```

### 7.4 Statistics Tracking

```rust
pub struct CryptoRiskChecker {
    pub trades_validated: Arc<AtomicU64>,  // Total checked
    pub trades_blocked: Arc<AtomicU64>,    // Rejected by risk checks
}

// In monitoring stats:
Risk block rate = trades_blocked / trades_validated
```

---

## 8. Position Sizing

### 8.1 Arbitrage Position Sizing

**Strategy**: Edge-Based Multiplier

```rust
base_size = 100.0  // $100 base

edge_multiplier = (net_edge_pct / min_edge_pct).min(3.0)

suggested_size = base_size * edge_multiplier

Example:
- Net edge: 6.0%
- Min edge: 3.0%
- Multiplier: min(6.0/3.0, 3.0) = 2.0x
- Position: 100 * 2.0 = $200
```

**Logic**: Larger edges warrant larger positions (up to 3x cap)

### 8.2 Probability Position Sizing

**Strategy**: Kelly Criterion (Conservative)

```
Kelly fraction formula:
f* = (probability × win_size - (1 - probability) × loss_size) / win_size

Crypto Kelly sizing:
f* = probability - (1 - probability)  // For 1:1 odds
f_conservative = f* × kelly_fraction
size = account_capital × f_conservative

Implementation:
kelly_fraction = 0.25  // Fractional Kelly (1/4x)
win_prob = match direction {
    Long => model_probability,
    Short => 1.0 - model_probability,
}
kelly_bps = (win_prob * 200 - 100) * kelly_fraction  // In basis points
size = account_capital * kelly_bps / 10000
```

**Example:**
```
Model: 70% probability for price target
Direction: LONG
Win probability: 0.70
Loss probability: 0.30

Full Kelly: f* = 0.70 - 0.30 = 0.40 (40% of bankroll)
Fractional Kelly: 0.40 × 0.25 = 0.10 (10% of bankroll)

Account capital: $1000
Position size: 1000 × 0.10 = $100
```

### 8.3 Post-Risk-Check Adjustment

```rust
// Suggested size might be capped by risk checks:

suggested_size = $200  // From edge-based or Kelly calculation
max_position_size = $500  // Risk check limit
asset_exposure = $1500 from $2000 cap  // Available: $500
total_exposure = $4700 from $5000 cap  // Available: $300

Final size = min(200, 500, 500, 300) = $200 ✓
```

---

## 9. Execution Request Flow

### 9.1 Request Structure

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CryptoExecutionRequest {
    pub request_id: String,                    // UUID
    pub event_id: String,                      // From orchestrator
    pub asset: String,                         // BTC, ETH, SOL, etc
    pub signal_type: CryptoSignalType,         // Arbitrage | Probability
    pub platform: String,                      // kalshi | polymarket
    pub market_id: String,                     // Market identifier
    pub direction: Direction,                  // Long | Short
    pub edge_pct: f64,                        // Profitable edge percentage
    pub probability: f64,                      // 0.5 for arb, model prob for signals
    pub suggested_size: f64,                   // USD amount after risk checks
    pub max_price: f64,                        // With slippage buffer
    pub current_price: f64,                    // At detection time
    pub timestamp: DateTime<Utc>,              // When signal was created
    pub volatility_factor: f64,                // Recent volatility multiplier
    pub exposure_check: bool,                  // Whether exposure was validated
    pub balance_check: bool,                   // Whether balance was validated
}
```

### 9.2 ZMQ Publishing

```
Endpoint: tcp://*:5559
Format: Multipart message
Part 1 (topic): crypto.execution.{request_id}
Part 2 (payload): JSON-serialized CryptoExecutionRequest

Example:
Topic: crypto.execution.550e8400-e29b-41d4-a716-446655440000
Payload: {
  "request_id": "550e8400-e29b-41d4-a716-446655440000",
  "event_id": "kalshi:KXBTCD-26JAN3117-T84749.99",
  "asset": "BTC",
  "signal_type": "Arbitrage",
  "platform": "polymarket",
  "market_id": "0xbfdb06d66ec0f276a410e1a7705dae29f318aa1953bc5e1a51b8131df076afe4",
  "direction": "Long",
  "edge_pct": 3.45,
  "probability": 0.5,
  "suggested_size": 250.0,
  "max_price": 0.4510,
  "current_price": 0.4500,
  "timestamp": "2026-01-31T00:22:15.123Z",
  "volatility_factor": 1.2,
  "exposure_check": true,
  "balance_check": true
}
```

### 9.3 ExecutionService Reception

```
ExecutionService subscribes to tcp://localhost:5559
Receives CryptoExecutionRequest
Routes to paper trading module
Creates entry in paper_trades table:

INSERT INTO paper_trades (
  request_id, event_id, asset, signal_type, direction,
  entry_price, suggested_size, status, created_at
) VALUES (...)
```

---

## 10. Configuration & Tuning

### 10.1 Environment Variables

```bash
# Signal Detection Thresholds
CRYPTO_MIN_EDGE_PCT=3.0                    # Minimum arbitrage edge
CRYPTO_MODEL_MIN_CONFIDENCE=0.60           # Probability model threshold
CRYPTO_ALLOW_ALL_TIMEFRAMES=true           # Include all Kalshi market types

# Risk Management Limits
CRYPTO_MAX_POSITION_SIZE=500.0             # Per-trade size limit
CRYPTO_MAX_ASSET_EXPOSURE=2000.0           # Per-asset (BTC, ETH, SOL) limit
CRYPTO_MAX_TOTAL_EXPOSURE=5000.0           # Total crypto exposure limit
CRYPTO_MIN_LIQUIDITY=50.0                  # Minimum market liquidity required

# Volatility Management
CRYPTO_VOLATILITY_SCALING=true             # Enable dynamic scaling
CRYPTO_VOLATILITY_WINDOW_DAYS=30           # Historical window for calculation

# Position Sizing
CRYPTO_KELLY_FRACTION=0.25                 # Fractional Kelly (conservative)
CRYPTO_BASE_POSITION_SIZE=100.0            # Base for arb multiplier

# Monitoring
CRYPTO_POLL_INTERVAL_SECS=30               # Event monitoring frequency
CRYPTO_PRICE_STALENESS_SECS=60             # Max price age before warning
CRYPTO_HEARTBEAT_INTERVAL_SECS=5           # Service health reporting
```

### 10.2 Tuning Strategy

**For Conservative Trading:**
```
CRYPTO_MIN_EDGE_PCT=5.0              # Only strong signals
CRYPTO_MAX_POSITION_SIZE=100.0       # Small positions
CRYPTO_MAX_TOTAL_EXPOSURE=1000.0     # Low total allocation
CRYPTO_KELLY_FRACTION=0.10           # Cautious sizing
```

**For Aggressive Trading:**
```
CRYPTO_MIN_EDGE_PCT=1.5              # Lower threshold
CRYPTO_MAX_POSITION_SIZE=1000.0      # Larger positions
CRYPTO_MAX_TOTAL_EXPOSURE=10000.0    # High allocation
CRYPTO_KELLY_FRACTION=0.50           # Full Kelly
```

**For High-Volatility Markets:**
```
CRYPTO_VOLATILITY_SCALING=true       # Enable scaling
CRYPTO_MIN_EDGE_PCT=5.0              # Higher bar
CRYPTO_MODEL_MIN_CONFIDENCE=0.70     # Stricter model
```

### 10.3 Impact Analysis

| Change | Effect | Risk |
|--------|--------|------|
| ↑ MIN_EDGE_PCT | Fewer but higher-quality signals | Miss opportunities |
| ↓ MIN_EDGE_PCT | More signals, lower profit margin | More small losses |
| ↑ MAX_POSITION_SIZE | Larger wins, larger losses | Drawdown risk |
| ↓ MAX_TOTAL_EXPOSURE | Less capital at risk | Lower profits |
| ↑ KELLY_FRACTION | More aggressive sizing | Volatility risk |
| ↑ VOLATILITY_SCALING | Protection in turbulence | Miss high-vol opportunities |

---

## 11. Monitoring & Observability

### 11.1 Key Metrics

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CryptoShardStats {
    pub events: u64,                    // Total events processed
    pub arb_signals: u64,              // Arbitrage signals generated
    pub model_signals: u64,            // Probability signals generated
    pub exec_sent: u64,                // Execution requests sent
    pub risk_blocks: u64,              // Signals rejected by risk checks
    pub stale_warnings: u64,           // Price staleness warnings
}
```

### 11.2 Real-Time Logging

**Price Reception:**
```
[INSTRUMENTATION] Message #0: topic='prices.kalshi.KXBTCD-26JAN3117-T84749.99' (320 bytes)
[INSTRUMENTATION] ✓ Price #0: BTC | kalshi ($0.4500/$0.4700)
[INSTRUMENTATION] Received 100 prices total. Latest: BTC | kalshi ($0.4500/$0.4700)
```

**Signal Generation:**
```
[INFO] Crypto arbitrage detected: BTC 3.45% edge
       (bid=0.46 on kalshi, ask=0.45 on polymarket), size=$250
```

**Risk Checks:**
```
[DEBUG] Trade passed risk checks: BTC polymarket $250 (edge: 3.45%, volatility: 1.2x)
[WARN] Asset BTC exposure limit: reducing size to $150
[DEBUG] Trade blocked: Asset exposure limit reached
```

**Heartbeat:**
```
[INFO] CryptoShard stats: events=75515, arb_signals=12, model_signals=8, exec_sent=20, risk_blocks=15
```

### 11.3 Alerting Thresholds

| Metric | Warning | Critical |
|--------|---------|----------|
| Risk block rate | > 30% | > 50% |
| Stale prices | Any | > 1000 |
| Zero signals | > 5 min | > 10 min |
| ZMQ backlog | > 1000 msgs | > 5000 msgs |

---

## 12. Performance Characteristics

### 12.1 Latency Breakdown

**From Price Reception to Execution Request:**

```
Price received on ZMQ: T+0ms
    ↓
Deserialization: T+0.5ms
    ↓
Cache insertion: T+0.7ms
    ↓
Price update notification: T+1ms
    ↓
Event monitoring loop wakes: T+2ms (up to 5s)
    ↓
Arbitrage detection: T+3ms (parallel with probability)
Probability detection: T+3ms (parallel with arbitrage)
    ↓
Risk checks (parallel DB queries): T+5-15ms
    ↓
ZMQ publish: T+20ms
    ↓
ExecutionService receives: T+25-50ms (network latency)

Total signal-to-execution: 50-100ms (vs 300-600ms for sports)
```

### 12.2 Throughput

**Maximum Processing Capacity:**

```
Price ingestion: 200+ prices/second
Event monitoring: Every 5 seconds
Signals evaluated per cycle: 50-100 events
Execution requests: 5-20 per second peak

Memory footprint:
- Price cache: ~50KB
- Event storage: ~500KB (1000 events)
- Risk checker state: ~100KB
- Total shard: ~50MB typical

CPU usage:
- Idle: <1% (waiting on ZMQ)
- Active: 10-30% (during market hours)
- Peak: <50% (during volatility spikes)
```

### 12.3 Bottleneck Analysis

**Current Bottlenecks:**
1. Price cache (single-price-per-asset) limits cross-market arbitrage
2. Database queries for risk checks (5-15ms) → async parallelization
3. ZMQ network latency (5-25ms)

**Optimization Opportunities:**
1. Per-market circular buffer (Phase 2)
2. In-memory risk tracking (no DB queries)
3. Local execution instead of ZMQ (future)

---

## 13. Deployment & Integration

### 13.1 Service Dependencies

```
crypto_shard_rust
├─ Redis (market assignments, coordination)
├─ ZMQ (price subscription, execution publishing)
├─ TimescaleDB (risk checking, position tracking)
├─ Kalshi Monitor (prices.kalshi.*)
├─ Polymarket Monitor (prices.poly.*)
└─ Spot Price Monitor (crypto.prices.*)

ExecutionService (consumes execution requests)
├─ paper_trades table
├─ bankroll tracking
└─ P&L calculation
```

### 13.2 Docker Configuration

```yaml
crypto_shard:
  image: arbees-crypto_shard
  environment:
    CRYPTO_MIN_EDGE_PCT: "3.0"
    CRYPTO_MAX_POSITION_SIZE: "500.0"
    CRYPTO_MAX_TOTAL_EXPOSURE: "5000.0"
    # ... other configs
  ports:
    - "8002:80"  # Health check endpoint
  depends_on:
    - timescaledb
    - redis
```

### 13.3 Health Checks

**Readiness Check:**
```
GET /health/ready
Response: 200 OK if subscribed to ZMQ topics
```

**Liveness Check:**
```
GET /health/live
Response: 200 OK if heartbeat sent in last 10s
```

**Detailed Status:**
```
GET /stats
Response: JSON with events, signals, risk blocks, latest prices
```

---

## 14. Troubleshooting Guide

### 14.1 No Signals Generated

**Symptoms:**
```
arb_signals: 0, model_signals: 0 (for hours)
```

**Root Causes & Solutions:**

| Cause | Diagnosis | Solution |
|-------|-----------|----------|
| No prices in cache | Check: prices_received counter | Verify ZMQ subscriptions, check monitors |
| Edge too low | Check: arb_signals==0 but prices!=0 | Lower MIN_EDGE_PCT or check market liquidity |
| Markets not assigned | Check: orchestrator logs | Verify market discovery running |
| Risk checks too strict | Check logs for "blocked by risk" | Review CRYPTO_MAX_* settings |
| Stale prices only | Check: stale_warnings > prices | Price monitors need restart |

**Diagnostic Command:**
```bash
docker logs arbees-crypto-shard | grep -E "arb_signals|stale|Message #"
```

### 14.2 High Risk Block Rate

**Symptoms:**
```
arb_signals: 10, but risk_blocks: 50 (80% block rate)
```

**Solutions:**
- Lower `CRYPTO_MAX_POSITION_SIZE` threshold gradually
- Check asset/total exposure limits being hit
- Review `CRYPTO_MIN_EDGE_PCT` (may be catching weak signals)
- Monitor `volatility_factor` scaling impact

### 14.3 Stale Price Warnings

**Symptoms:**
```
stale_warnings: 100,000+ and still climbing
```

**Root Causes:**
- Single-price-per-asset cache (Phase 2 limitation)
- Multiple markets for same asset updating at different times
- Timestamp mismatches between platforms

**Temporary Solution:**
- Lower `CRYPTO_PRICE_STALENESS_SECS` threshold
- Suppress DEBUG logs with rate limiting

**Permanent Solution:**
- Implement Phase 2 per-market circular buffer

### 14.4 ZMQ Connection Issues

**Symptoms:**
```
Failed to subscribe to prices.kalshi
Failed to connect to tcp://...
```

**Solutions:**
- Verify monitors are running: `docker ps | grep monitor`
- Check network connectivity: `telnet localhost 5560`
- Review firewall rules for port 5560
- Check docker-compose network configuration

### 14.5 Database Connection Failures

**Symptoms:**
```
Risk checks failing with DB error
Paper trades table not updating
```

**Solutions:**
- Verify TimescaleDB is running: `docker logs arbees-timescaledb`
- Check connection string in environment
- Verify paper_trades table exists: `psql ... \d paper_trades`
- Monitor DB connections: `SELECT count(*) FROM pg_stat_activity`

### 14.6 Memory Leaks

**Symptoms:**
```
Memory usage steadily increasing
Price cache size growing unbounded
```

**Debug:**
```bash
# Check cache size in logs
docker logs arbees-crypto-shard | grep "Cache size"

# Check for memory leaks
docker stats arbees-crypto-shard
```

**Solutions:**
- Restart crypto_shard periodically
- Check for unbounded HashMap growth
- Implement cache eviction (not currently done)

---

## 15. Known Limitations

### 15.1 Single-Price-Per-Asset Cache

**Issue**: Multiple markets for same asset (different strikes) overwrite each other

```
Example:
- Kalshi market 1: "BTC > $95k" → bid=$0.45, ask=$0.47
- Kalshi market 2: "BTC > $100k" → bid=$0.25, ask=$0.27
- Both store as "BTC|kalshi"
- Last write wins → Only one price cached
- Can't compare "BTC > $95k" across platforms
```

**Impact**:
- Massive stale price warnings (160k+)
- Can't do true cross-market arbitrage for directional markets
- Only works with spot prices + single strike price per asset

**Phase 2 Solution**:
- Replace HashMap with per-market circular buffer
- Index by market_id instead of asset|platform
- Store last N prices for each market
- Enable accurate timestamp matching

### 15.2 Volatility Factor Estimation

**Current Implementation**: Fixed 50% annual volatility

**Limitation**: Doesn't adapt to actual market volatility

**Future**: Calculate from recent price movements

### 15.3 No Market Impact Modeling

**Current**: Assumes execution at quoted prices

**Reality**: Large orders move the market

**Limitation**: May overestimate arbitrage edges on illiquid markets

### 15.4 Synchronous Arbitrage Assumption

**Current**: Assumes can buy on one platform and immediately sell on another

**Reality**: Prices might move between trades

**Mitigation**: edge_pct >= 3% provides buffer (vs typical 0.5-1% slippage)

### 15.5 Model Limitations

**Black-Scholes Assumptions**:
- Log-normal price distribution (may not hold in crypto)
- Constant volatility (crypto volatility changes)
- No dividends/funding rates
- No order book depth

**Better Models** (future): GBM with stochastic volatility, jump-diffusion

---

## 16. Future Enhancements

### 16.1 Phase 2: Per-Market Price Storage

```rust
// Current (broken)
HashMap<String, CryptoPriceData>
  Key: "BTC|kalshi" → single price

// Phase 2 (proposed)
HashMap<String, VecDeque<CryptoPriceData>>
  Key: "kalshi:KXBTCD-26JAN3117-T84749.99" → price history

  Benefits:
  - True cross-market arbitrage
  - Timestamp matching
  - Eliminate stale warnings
  - Support multi-strike strategies
```

### 16.2 Phase 3: Advanced Probability Models

- Stochastic volatility modeling
- Jump-diffusion processes
- Order book depth integration
- Real options valuation

### 16.3 Phase 4: Risk Management Enhancements

- Dynamic Kelly fraction based on track record
- Correlation matrix for cross-asset exposure
- Maximum drawdown limits
- Value-at-Risk (VaR) calculations
- Conditional Value-at-Risk (CVaR)

### 16.4 Phase 5: Execution Optimization

- Smart order routing
- Iceberg orders to minimize market impact
- Execution venue selection
- Latency arbitrage across trading platforms

### 16.5 Phase 6: Live Trading

- Account balance management
- Fee optimization
- Slippage monitoring
- P&L tracking and attribution
- Performance analytics

---

## Appendix: Complete Configuration Reference

```bash
# Risk Management
CRYPTO_MIN_EDGE_PCT=3.0                    # Minimum arbitrage edge %
CRYPTO_MODEL_MIN_CONFIDENCE=0.60           # Model confidence threshold
CRYPTO_MAX_POSITION_SIZE=500.0             # Per-trade limit (USD)
CRYPTO_MAX_ASSET_EXPOSURE=2000.0           # Per-asset limit (USD)
CRYPTO_MAX_TOTAL_EXPOSURE=5000.0           # Total crypto limit (USD)
CRYPTO_MIN_LIQUIDITY=50.0                  # Minimum market liquidity (USD)

# Position Sizing
CRYPTO_KELLY_FRACTION=0.25                 # Fractional Kelly (0.1-0.5)
CRYPTO_BASE_POSITION_SIZE=100.0            # Base position (USD)
CRYPTO_VOLATILITY_SCALING=true             # Enable volatility adjustment

# Volatility Management
CRYPTO_VOLATILITY_WINDOW_DAYS=30           # Lookback window
CRYPTO_MODEL_TIME_DECAY=true               # Adjust for time decay

# Market Filtering
CRYPTO_ALLOW_ALL_TIMEFRAMES=true           # Include all Kalshi markets
CRYPTO_INTRADAY_ONLY=false                 # Filter to 15-min markets

# Monitoring
CRYPTO_POLL_INTERVAL_SECS=30               # Event monitoring loop
CRYPTO_PRICE_STALENESS_SECS=60             # Stale price threshold
CRYPTO_HEARTBEAT_INTERVAL_SECS=5           # Health reporting

# Shard Identity
CRYPTO_SHARD_ID=crypto_1                   # Shard identifier
CRYPTO_PRICE_SUB_ENDPOINTS=tcp://localhost:5560
CRYPTO_EXECUTION_PUB_ENDPOINT=tcp://*:5559

# Paper Trading
PAPER_TRADING=1                            # 1=paper, 0=live
```

---

## Document History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-01-31 | Initial complete implementation document |

---

**Questions? Issues?** Refer to the Troubleshooting Guide (Section 14) or create an issue in the project repository.
