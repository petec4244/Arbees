# Risk Management Layer - January 20, 2026

## Summary

Implemented comprehensive risk management layer that gates all trade execution.

## Components Created

### 1. RiskController (`shared/arbees_shared/risk/controller.py`)

Core risk management class with four enforcement mechanisms:

#### Max Daily Loss
- **Default**: $100
- **Behavior**: Blocks all trading when daily P&L hits -$100
- **Calculation**: Realized (closed trades) + Unrealized (conservative 2% adverse estimate)

#### Exposure Limits
- **Per-game**: $50 max (prevents overconcentration)
- **Per-sport**: $200 max (enforces diversification)

#### Position Correlation
- Detects conflicting positions on same game:
  - BUY home team + BUY away team (both can't win)
  - SELL home team + SELL away team (one must win)
- Uses game metadata to identify teams

#### Latency Circuit Breaker
- Rejects signals > 5 seconds old
- Circuit breaker triggers at 10 second latency
- 60 second cooldown before re-enabling
- Manual override: `force_open_circuit_breaker()`, `force_close_circuit_breaker()`

### 2. Integration with Position Manager

Updated `services/position_manager/position_manager.py`:
- Initializes RiskController on startup
- Calls `evaluate_trade()` before every execution
- Tracks rejection count in metrics
- Logs risk status every 60 seconds

## Files Modified

1. `shared/arbees_shared/risk/__init__.py` (NEW)
2. `shared/arbees_shared/risk/controller.py` (NEW)
3. `services/position_manager/position_manager.py` (MODIFIED)
4. `.env.example` (MODIFIED)
5. `.env` (MODIFIED)

## Environment Variables

```bash
MAX_DAILY_LOSS=100.0         # Stop trading after losing $100/day
MAX_GAME_EXPOSURE=50.0       # Max $ per game
MAX_SPORT_EXPOSURE=200.0     # Max $ per sport
MAX_LATENCY_MS=5000.0        # Reject signals > 5 seconds old
```

## Risk Decision Flow

```
Signal → Edge check → Prob guardrails → Position check → RiskController
                                                              ↓
                                                    Circuit breaker?
                                                    Latency OK?
                                                    Daily loss OK?
                                                    Game exposure OK?
                                                    Sport exposure OK?
                                                    No correlation?
                                                              ↓
                                                    APPROVED → Execute
                                                    REJECTED → Log & skip
```

## Rejection Reasons (RiskRejection enum)

- `daily_loss_limit_reached`
- `game_exposure_limit_reached`
- `sport_exposure_limit_reached`
- `correlated_position_detected`
- `circuit_breaker_open`
- `latency_too_high`

## Monitoring

Heartbeat now includes:
- `risk_rejected` count
- `daily_pnl`
- `circuit_breaker` status

Risk status report logged every 60 seconds with:
- Daily P&L vs limit
- Exposure by sport (with % of limit)
- Exposure by game (top 5)
- Circuit breaker status
