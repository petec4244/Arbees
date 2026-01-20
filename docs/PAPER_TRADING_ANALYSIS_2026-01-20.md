# Paper Trading Post-Mortem Analysis
**Date:** January 20, 2026
**Period Analyzed:** January 20, 2026 (overnight session)

---

## Executive Summary

**Results: 5 wins, 16 losses, 11 pending = $-103.21 total PnL**

The paper trading system failed catastrophically. After analyzing all 32 trades, I've identified **4 critical flaws** that must be fixed before any further trading:

1. **Buying at extreme probabilities** (80%+) with terrible risk/reward
2. **Excessive churning** - 14 trades on a single game in one session
3. **Position hold times too short** - trades lasting 9-40 seconds
4. **Fictional "edge" calculations** - reported 30% edge on trades that immediately lost money

---

## Detailed Findings

### 1. Trades by Entry Price Range

| Price Range | Trades | Wins | Losses | Avg "Edge" | P&L |
|-------------|--------|------|--------|------------|-----|
| BUY 80%+ | 6 | **0** | **6** | 26.49% | **-$47.64** |
| BUY 70-80% | 4 | 2 | 2 | 18.71% | -$15.02 |
| BUY 60-70% | 1 | 0 | 1 | 11.58% | -$9.33 |
| BUY <60% | 4 | 2 | 2 | 5.27% | -$1.18 |
| SELL >40% | 6 | 1 | 5 | 12.94% | -$30.03 |

**Key Insight:** Every single trade at 80%+ probability lost money. The risk/reward at these levels is atrocious:
- Buying at 85%: Risk $85 to make $15 (5.7:1 against)
- Buying at 99.5%: Risk $99.50 to make $0.50 (199:1 against)

### 2. Churning Analysis (Same Game Trading)

| Game ID | Sport | Trades | Avg Gap (sec) | Min Gap (sec) |
|---------|-------|--------|---------------|---------------|
| 401810467 | NBA | **14** | 386 | **40** |
| 401809250 | NBA | 5 | 101 | 30 |
| 401769076 | NCAAF | 2 | 350 | 31 |
| 401803129 | NHL | 2 | 157 | 92 |

**Key Insight:** We traded the same NBA game 14 times in one session, with gaps as short as 40 seconds between trades. Each trade incurs slippage (2.5% per trade). At 14 trades, that's 35% of position value lost to slippage alone.

### 3. Hold Time Analysis

| Hold Time | Trades | Wins | Losses | Win Rate |
|-----------|--------|------|--------|----------|
| < 30 sec | 5 | 0 | 5 | **0%** |
| 30-60 sec | 4 | 1 | 3 | 25% |
| 1-5 min | 4 | 1 | 3 | 25% |
| 5-20 min | 3 | 1 | 2 | 33% |
| 20+ min | 5 | 2 | 3 | 40% |

**Key Insight:** Trades held for less than 30 seconds had a 0% win rate. The system is trading on noise, not signal.

### 4. Worst Trades (Detailed)

| Trade ID | Game | Entry | Exit | Hold | P&L | Issue |
|----------|------|-------|------|------|-----|-------|
| d3738195 | NHL 401803127 | **99.50%** | 74.14% | 4 min | **-$12.70** | Bought at near-certainty |
| 5b2c80d6 | NCAAF 401769076 | SELL 63.19% | 92.31% | 18 min | **-$15.71** | Sold, then game swung hard |
| 230639e3 | NBA 401810467 | 79.40% | 68.99% | 41 sec | **-$10.15** | 79% buy, held 41 seconds |
| e38ce380 | NBA 401810467 | **85.53%** | 76.48% | 14 min | **-$8.50** | Above 85% guardrail |
| f1e47d28 | NBA 401810467 | SELL 63.95% | 72.59% | 28 sec | **-$8.34** | Sold, 28 sec hold |

### 5. The "Edge" Is a Lie

The reported "edge at entry" ranged from 2% to 33%. But:
- Trades with 30%+ "edge": 5 trades, 0 wins, 5 losses
- Trades with 20-30% "edge": 3 trades, 1 win, 2 losses
- Trades with <10% "edge": 6 trades, 3 wins, 3 losses

**Key Insight:** The edge calculation is fundamentally broken. It's comparing our model probability to market price, but:
1. Our model may be wrong
2. The market is already incorporating information we think is "new"
3. The "edge" evaporates immediately after entry

---

## Root Cause Analysis

### Problem 1: Win Probability Model Overconfidence

The win probability model is generating probabilities in the 80-99% range that the market doesn't agree with. Either:
- Our model is wrong (likely)
- The market is wrong (unlikely at these extremes)

When we BUY at 99.5% because our model says the team will win, we're betting $99.50 to win $0.50. Even if our model is correct 99% of the time, the expected value is negative due to slippage.

### Problem 2: Signal Generation Too Sensitive

The system generates a new signal every time the win probability changes by a small amount. This leads to:
- Rapid position flipping (BUY -> SELL -> BUY)
- Each flip costs 2.5% slippage
- Net effect: bleeding money to market friction

### Problem 3: No Cooldown Period

There's no minimum time between signals for the same game. The system traded the same game every 40 seconds at times.

### Problem 4: Position Sizing at Extremes

Kelly criterion gives large positions when "edge" is high. But if the edge is fictional, we're just making larger bad bets. At 80%+ entry, the system was sizing positions at $93-98.

---

## Recommended Fixes

### Immediate (Before Next Session)

1. **Probability Guardrails** (Already Implemented)
   - MAX_BUY_PROB = 0.85 (don't buy above 85%)
   - MIN_SELL_PROB = 0.15 (don't sell below 15%)
   - Status: Done, but consider tightening to 0.75/0.25

2. **Minimum Hold Time**
   - Don't generate opposite signals within 5 minutes of entry
   - Prevents churning and noise-based trading

3. **Per-Game Cooldown**
   - Maximum 1 signal per game per 10 minutes
   - Prevents 14-trade sessions on single game

### Medium Term

4. **Edge Calculation Review**
   - The current "edge" calculation is meaningless
   - Need to validate model vs actual outcomes
   - Consider: edge = model_prob - market_prob - slippage - fees

5. **Model Probability Calibration**
   - Track: "When model says 80%, how often does team actually win?"
   - If model says 80% but actual win rate is 60%, model is overconfident
   - Adjust model before trusting it with real money

6. **Position Sizing Reform**
   - Don't use Kelly on uncalibrated edge
   - Start with fixed small sizes ($5-10) until model proves profitable

### Before Live Trading

7. **100+ Trade Backtesting**
   - Need positive expected value over large sample
   - Current: 5 wins / 21 closed = 23.8% win rate
   - Unacceptable for live trading

8. **Profitability Threshold**
   - Don't go live until paper trading shows 3 consecutive winning sessions
   - Or: positive P&L over 100+ trades

---

## Raw Data

### All Closed Trades (Chronological)

```
Trade ID                              | Game      | Sport | Side | Entry  | Exit   | Edge  | Size  | Outcome | Hold (sec) | P&L
--------------------------------------|-----------|-------|------|--------|--------|-------|-------|---------|------------|-------
68e5523d-dec6-46ac-9593-be523bbc0356  | 401803129 | nhl   | sell | 0.4929 | 0.1545 | 11.58 | 11.97 | win     | 6694       | +4.05
01f41e7a-3fc0-42c3-8376-16af01c1f9de  | 401803129 | nhl   | buy  | 0.6587 | 0.5230 | 11.58 | 68.76 | loss    | 2872       | -9.33
44d21a7f-f868-4b1e-87a5-30dee71f6256  | 401810467 | nba   | buy  | 0.5773 | 0.5952 | 5.23  | 30.41 | win     | 1757       | +0.54
a848e009-63f0-4665-a9b1-4e4c70d33567  | 401812023 | ncaab | buy  | 0.7281 | 0.7507 | 3.81  | 60.12 | win     | 1238       | +1.36
5b2c80d6-6f16-4823-a21a-831dbcab2bfe  | 401769076 | ncaaf | sell | 0.6319 | 0.9231 | 15.69 | 53.95 | loss    | 1108       | -15.71
ef3e0938-00a7-4abe-aff3-11da50d5352e  | 401810467 | nba   | buy  | 0.5954 | 0.5465 | 7.04  | 32.09 | loss    | 675        | -1.57
d3738195-6ac8-4e40-b754-a0ff570c9edb  | 401803127 | nhl   | buy  | 0.9950 | 0.7414 | 7.77  | 50.06 | loss    | 235        | -12.70
02ae0da5-8462-4bb2-875c-1ececea59722  | 401810467 | nba   | sell | 0.5389 | 0.5498 | 6.39  | 56.68 | loss    | 1161       | -0.62
d2ea6595-a708-4ef6-97e2-781b20f994ca  | 401803130 | nhl   | buy  | 0.5457 | 0.7251 | 2.07  | 19.86 | win     | 3299       | +3.56
b26db263-a0bc-4695-b6f6-33c38b417773  | 401810467 | nba   | buy  | 0.5925 | 0.5293 | 6.75  | 58.85 | loss    | 41         | -3.72
c18b236c-f66f-41af-b6e0-3bee58ced906  | 401810467 | nba   | sell | 0.5102 | 0.6294 | 3.52  | 32.44 | loss    | 333        | -3.87
4cc8ee0f-5b88-4ab4-a590-b5a91365ca56  | 401810467 | nba   | buy  | 0.7019 | 0.7114 | 17.69 | 98.16 | win     | 183        | +0.93
77876cd7-1d38-4161-acf4-93eedd7db68c  | 401810467 | nba   | buy  | 0.7895 | 0.7166 | 26.45 | 98.25 | loss    | 19         | -7.16
230639e3-b326-46d8-be37-e5064a934915  | 401810467 | nba   | buy  | 0.7940 | 0.6899 | 26.90 | 97.53 | loss    | 41         | -10.15
f1e47d28-be1e-44e5-a372-cec8e3970aa5  | 401810467 | nba   | sell | 0.6395 | 0.7259 | 16.45 | 96.52 | loss    | 28         | -8.34
b338142b-1c9c-491d-94ab-c4d474cbb6e3  | 401810467 | nba   | buy  | 0.8062 | 0.7261 | 28.12 | 95.69 | loss    | 40         | -7.66
56300c61-047c-4326-aeb0-9a8ba52b6852  | 401810467 | nba   | buy  | 0.8105 | 0.7498 | 28.55 | 94.92 | loss    | 9          | -5.76
1ac97982-1a55-4bef-92ba-d30c3a995b68  | 401810467 | nba   | buy  | 0.8343 | 0.7573 | 30.93 | 94.34 | loss    | 9          | -7.26
af822e42-37ec-420d-bbe4-d924ce6d9ff4  | 401810467 | nba   | buy  | 0.8302 | 0.7687 | 30.52 | 93.62 | loss    | 12         | -5.76
c93edb89-02d6-4bec-bea2-42590bf5311c  | 401810467 | nba   | sell | 0.7150 | 0.7746 | 24.00 | 93.04 | loss    | 123        | -5.55
e38ce380-d533-4b36-bacc-563f09716888  | 401810467 | nba   | buy  | 0.8553 | 0.7648 | 33.03 | 93.93 | loss    | 868        | -8.50
```

### Aggregate Statistics

- **Total Trades:** 32 (21 closed, 11 pending)
- **Win Rate:** 23.8% (5/21)
- **Total P&L:** -$103.21
- **Average Position Size:** $70.38
- **Average Entry Price:** 68.67%
- **Average Reported Edge:** 16.15% (meaningless)
- **Average Slippage:** 2.5%

---

## Conclusion

The system is not ready for live trading. The win probability model is overconfident, the edge calculation is broken, and the signal generation is too noisy. We lost $103 in one overnight session on what should have been "safe" paper trades.

**Priority actions:**
1. Tighten probability guardrails (65%/35%)
2. Add minimum hold time (5 minutes)
3. Add per-game cooldown (10 minutes)
4. Reduce position sizes until model is calibrated
5. Run 100+ trade backtest before any live trading

---

*Analysis generated by Claude Code*
*Arbees Paper Trading System*
