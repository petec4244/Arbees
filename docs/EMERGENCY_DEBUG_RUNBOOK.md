# Emergency Debug Runbook

Operational guide for validating the emergency debug fixes for paper trading.

## Quick Reference

### Success Criteria

| Metric | Threshold | SQL Query |
|--------|-----------|-----------|
| Immediate exits | 0 trades < 10s hold | See Test 1 below |
| Bad entry prices | 0 trades > 85% entry | See Test 2 below |
| Net P&L | > $0 | See Test 3 below |
| Win rate | > 40% | See Test 4 below |
| Average hold time | > 30s | See Test 5 below |

---

## Pre-Flight Checks (PowerShell)

### 1. Stop Trading Services

```powershell
# Stop the trading pipeline
docker compose --profile full stop signal_processor execution_service position_tracker

# Verify they're stopped
docker compose ps | Select-String "signal|execution|position"
```

### 2. Backup Database

```powershell
# Create backup directory with timestamp
$backup_dir = ".\backups\$(Get-Date -Format 'yyyyMMdd_HHmmss')"
New-Item -ItemType Directory -Path $backup_dir -Force

# Export paper trades
docker exec arbees-timescaledb pg_dump -U arbees -d arbees -t paper_trades --inserts | Out-File "$backup_dir\paper_trades_backup.sql" -Encoding UTF8

# Export market prices (last 24 hours)
docker exec arbees-timescaledb pg_dump -U arbees -d arbees -t market_prices --inserts | Out-File "$backup_dir\market_prices_backup.sql" -Encoding UTF8

# Verify backups
Get-ChildItem $backup_dir
```

### 3. Start Services

```powershell
# Start full stack
docker compose --profile full up -d

# Verify all services running
docker compose ps
```

---

## Monitoring (PowerShell)

### View Service Logs

```powershell
# Signal Processor - watch for team matching decisions
docker compose logs -f signal_processor | Select-String "team|confidence|market_lookup"

# Execution Service - watch for fills
docker compose logs -f execution_service | Select-String "filled|rejected|SUCCESS"

# Position Tracker - watch for exit decisions
docker compose logs -f position_tracker | Select-String "exit|team|skip"
```

### View Trace Logs

The trace logs are written to `.cursor/debug.log` in NDJSON format.

```powershell
# View recent trace events
Get-Content ".\.cursor\debug.log" -Tail 50

# Filter for specific events
Get-Content ".\.cursor\debug.log" | Select-String "market_lookup_selected"
Get-Content ".\.cursor\debug.log" | Select-String "execution_filled"
Get-Content ".\.cursor\debug.log" | Select-String "exit_check_skipped"

# Watch for new events
Get-Content ".\.cursor\debug.log" -Wait -Tail 10
```

---

## Validation Queries (SQL)

Connect to database:

```powershell
docker exec -it arbees-timescaledb psql -U arbees -d arbees
```

### Test 1: No Immediate Exits

```sql
-- Count trades with hold time < 10 seconds (should be 0)
SELECT COUNT(*) as immediate_exits
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours'
  AND EXTRACT(EPOCH FROM (exit_time - entry_time)) < 10;

-- View details if any found
SELECT 
    trade_id,
    market_title,
    side,
    entry_price,
    exit_price,
    pnl,
    EXTRACT(EPOCH FROM (exit_time - entry_time)) as hold_seconds
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours'
  AND EXTRACT(EPOCH FROM (exit_time - entry_time)) < 10
ORDER BY entry_time DESC
LIMIT 10;
```

### Test 2: No Bad Entry Prices

```sql
-- Count trades with entry > 85% (should be 0)
SELECT COUNT(*) as bad_entries
FROM paper_trades
WHERE entry_time > NOW() - INTERVAL '24 hours'
  AND entry_price > 0.85;

-- View details if any found
SELECT 
    trade_id,
    market_title,
    side,
    entry_price,
    model_prob,
    edge_at_entry
FROM paper_trades
WHERE entry_time > NOW() - INTERVAL '24 hours'
  AND entry_price > 0.85
ORDER BY entry_price DESC
LIMIT 10;
```

### Test 3: Positive P&L

```sql
-- Overall P&L (should be > 0)
SELECT 
    COUNT(*) as total_trades,
    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
    SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
    SUM(pnl) as net_pnl,
    AVG(pnl) as avg_pnl
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours';
```

### Test 4: Win Rate > 40%

```sql
-- Win rate calculation
SELECT 
    ROUND(
        COUNT(*) FILTER (WHERE outcome = 'win') * 100.0 / 
        NULLIF(COUNT(*) FILTER (WHERE outcome IN ('win', 'loss')), 0),
        1
    ) as win_rate_pct
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours';
```

### Test 5: Average Hold Time > 30s

```sql
-- Average hold time
SELECT 
    AVG(EXTRACT(EPOCH FROM (exit_time - entry_time))) as avg_hold_seconds
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours';
```

### Full Summary Query

```sql
-- Complete validation summary
SELECT 
    COUNT(*) as total_closed,
    COUNT(*) FILTER (WHERE outcome = 'win') as wins,
    COUNT(*) FILTER (WHERE outcome = 'loss') as losses,
    COUNT(*) FILTER (WHERE outcome = 'push') as pushes,
    ROUND(
        COUNT(*) FILTER (WHERE outcome = 'win') * 100.0 / 
        NULLIF(COUNT(*) FILTER (WHERE outcome IN ('win', 'loss')), 0),
        1
    ) as win_rate_pct,
    SUM(pnl) as net_pnl,
    AVG(entry_price) as avg_entry_price,
    AVG(EXTRACT(EPOCH FROM (exit_time - entry_time))) as avg_hold_seconds,
    COUNT(*) FILTER (WHERE entry_price > 0.85) as bad_entries,
    COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (exit_time - entry_time)) < 10) as immediate_exits
FROM paper_trades
WHERE status = 'closed'
  AND entry_time > NOW() - INTERVAL '24 hours';
```

---

## Run Validation Script

The automated validation script checks all criteria:

```powershell
# Set DATABASE_URL
$env:DATABASE_URL = "postgresql://arbees:YOUR_PASSWORD@localhost:5432/arbees"

# Run validation (default: last 24 hours)
python scripts/validate_paper_trading.py

# Custom time window
python scripts/validate_paper_trading.py --hours 12

# Custom thresholds
python scripts/validate_paper_trading.py --min-hold-seconds 15 --min-win-rate 45
```

---

## Troubleshooting

### Problem: Still seeing immediate exits

**Check:**
```powershell
# Verify min_hold_seconds is set
docker exec arbees-position-tracker printenv | Select-String "MIN_HOLD"

# Check logs for hold time warnings
docker compose logs position_tracker | Select-String "MINIMUM HOLD TIME"
```

**Fix:**
Add/update in docker-compose.yml under position_tracker environment:
```yaml
MIN_HOLD_SECONDS: "15.0"  # Increase if needed
```

### Problem: Still seeing bad entry prices

**Check:**
```powershell
# Check team matching confidence threshold
docker exec arbees-signal-processor printenv | Select-String "TEAM_MATCH"

# Look for rejections in logs
docker compose logs signal_processor | Select-String "market_lookup_rejected"
```

**Fix:**
Adjust confidence threshold in docker-compose.yml:
```yaml
TEAM_MATCH_MIN_CONFIDENCE: "0.65"  # Lower from 0.7 if too strict
```

### Problem: Not enough trades

**Check:**
```powershell
# Check rejection counts in logs
docker compose logs signal_processor | Select-String "rejected"

# Check what signals are being filtered
Get-Content ".\.cursor\debug.log" | Select-String "filter_rejected"
```

**Fix:**
Adjust signal thresholds in docker-compose.yml:
```yaml
MIN_EDGE_PCT: "1.5"  # Lower from 2.0
MAX_BUY_PROB: "0.98"  # Raise from 0.95
```

### Problem: Services crashing

**Check:**
```powershell
docker compose ps
docker compose logs signal_processor --tail 50
```

**Fix:**
```powershell
# Rebuild containers
docker compose build
docker compose --profile full up -d
```

---

## Emergency Stop

```powershell
# Stop all trading services immediately
docker compose --profile full stop

# Or stop entire stack
docker compose down

# Check nothing is running
docker ps
```

---

## Recovery

### Restore from Backup

```powershell
# Find latest backup
Get-ChildItem .\backups | Sort-Object LastWriteTime -Descending | Select-Object -First 1

# Restore paper trades (BE CAREFUL - this overwrites current data)
Get-Content ".\backups\TIMESTAMP\paper_trades_backup.sql" | docker exec -i arbees-timescaledb psql -U arbees -d arbees
```

### Reset Paper Trading State

```powershell
# Use the reset script
python scripts/reset_paper_trading.py

# Or manually close all open positions
docker exec -it arbees-timescaledb psql -U arbees -d arbees -c "UPDATE paper_trades SET status = 'cancelled' WHERE status = 'open';"
```

---

## Next Steps After Validation Passes

1. **Monitor for 24 hours** with enhanced logging
2. **Review trace logs** for any anomalies
3. **Document any tuning** changes made to thresholds
4. **Prepare for real execution** if all criteria consistently pass

---

## Key Configuration Parameters

| Service | Parameter | Default | Description |
|---------|-----------|---------|-------------|
| signal_processor | TEAM_MATCH_MIN_CONFIDENCE | 0.7 | Min confidence for team match at entry |
| position_tracker | EXIT_TEAM_MATCH_MIN_CONFIDENCE | 0.7 | Min confidence for team match at exit |
| position_tracker | MIN_HOLD_SECONDS | 10.0 | Minimum hold time before exit |
| position_tracker | PRICE_STALENESS_TTL | 30.0 | Max age of price data in seconds |
| position_tracker | TAKE_PROFIT_PCT | 3.0 | Take profit threshold % |
| position_tracker | DEFAULT_STOP_LOSS_PCT | 5.0 | Stop loss threshold % |
