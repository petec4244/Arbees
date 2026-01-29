# Arbees Fault Tolerance Chaos Testing Guide

**Purpose**: Validate all fault tolerance improvements before live trading.
**Status**: Ready for testing
**Last Updated**: 2026-01-28

---

## Pre-Testing Setup

### 1. Environment Configuration

Ensure these environment variables are set in `docker-compose.yml`:

```yaml
# Redis Reconnection
REDIS_RECONNECT_MAX_FAILURES: "10"
REDIS_RECONNECT_BASE_DELAY_MS: "1000"
REDIS_RECONNECT_MAX_DELAY_MS: "60000"
REDIS_RECONNECT_JITTER_PCT: "0.1"

# Critical Alerting
CRITICAL_ALERTS_ENABLED: "true"
SLACK_WEBHOOK_URL: "${SLACK_WEBHOOK_URL}"  # Set in .env
ALERT_RATE_LIMIT_SECS: "300"
ALERT_LOG_PATH: "/var/log/arbees/critical_alerts.log"

# Database Health
DB_HEALTH_CHECK_ENABLED: "true"
DB_HEALTH_CHECK_INTERVAL_SECS: "30"
DB_HEALTH_ALERT_THRESHOLD: "3"
DB_POOL_MAX_CONNECTIONS: "20"
DB_POOL_MIN_CONNECTIONS: "5"
```

### 2. Start Monitoring

**Terminal 1** - Follow all logs:
```powershell
.\scripts\follow_logs.ps1
```

**Terminal 2** - Watch critical alerts:
```powershell
docker exec orchestrator_rust tail -f /var/log/arbees/critical_alerts.log
```

**Terminal 3** - Commands (this terminal)

---

## Test Scenarios

### ✅ Test 1: Redis Restart (CRITICAL)

**Objective**: Verify services automatically reconnect to Redis with no message loss.

**Steps**:
```powershell
# 1. Start system
docker-compose up -d

# 2. Wait for system to stabilize (30 seconds)
Start-Sleep -Seconds 30

# 3. Note current reconnect stats in logs (should be 0)
docker logs orchestrator_rust --tail 5 | Select-String "reconnect"

# 4. Stop Redis
docker-compose stop redis

# 5. Wait 30 seconds - observe reconnection attempts in logs
Start-Sleep -Seconds 30

# 6. Restart Redis
docker-compose start redis

# 7. Wait 60 seconds for all services to reconnect
Start-Sleep -Seconds 60
```

**Expected Results**:
- ✅ All services log "Redis disconnected, reconnecting..."
- ✅ All services reconnect within 60 seconds
- ✅ Logs show "Successfully connected (total reconnects: N)"
- ✅ Exponential backoff visible: 1s → 2s → 4s → 8s...
- ✅ Heartbeats resume after reconnection
- ✅ No service crashes or exits

**Verification**:
```powershell
# Check reconnection stats
docker logs orchestrator_rust --tail 100 | Select-String "Successfully connected"
docker logs signal_processor_rust --tail 100 | Select-String "Successfully connected"
docker logs market_discovery_rust --tail 100 | Select-String "Successfully connected"

# Verify services are healthy
docker-compose ps
```

---

### ✅ Test 2: Game Shard Crash (CRITICAL)

**Objective**: Verify games are reassigned when a shard crashes.

**Steps**:
```powershell
# 1. System should be running with at least 2 shards and some games assigned
docker-compose ps game_shard_rust_shard_01 game_shard_rust_shard_02

# 2. Check which games are assigned to shard_01
docker logs orchestrator_rust --tail 100 | Select-String "shard_01.*game"

# 3. Kill shard_01
docker-compose stop game_shard_rust_shard_01

# 4. Wait 65 seconds (shard timeout + 5s buffer)
Start-Sleep -Seconds 65
```

**Expected Results**:
- ✅ Orchestrator marks shard as Dead after 60 seconds
- ✅ Games automatically reassigned to shard_02
- ✅ Logs show "Reassigning game X from shard_01 to shard_02"
- ✅ No game monitoring gaps (continuous coverage)

**Verification**:
```powershell
# Check for dead service detection
docker logs orchestrator_rust --tail 100 | Select-String "marked as Dead"

# Check for reassignment logs
docker logs orchestrator_rust --tail 100 | Select-String "Reassigning game"

# Verify games moved to healthy shard
docker logs game_shard_rust_shard_02 --tail 100 | Select-String "add_game"
```

---

### ✅ Test 3: Shard Restart & Resync (CRITICAL)

**Objective**: Verify restart detection and automatic game resync.

**Steps**:
```powershell
# 1. Restart a shard
docker-compose restart game_shard_rust_shard_01

# 2. Watch logs for restart detection
Start-Sleep -Seconds 10
```

**Expected Results**:
- ✅ Orchestrator detects restart (different process_id or started_at)
- ✅ Logs "Detected restart: shard_01"
- ✅ Games resynced within 10 seconds
- ✅ Shard receives all its assigned games
- ✅ Circuit breaker reset on restart

**Verification**:
```powershell
# Check restart detection
docker logs orchestrator_rust --tail 100 | Select-String "Detected restart"

# Check resync
docker logs orchestrator_rust --tail 100 | Select-String "Resync complete"

# Verify games received
docker logs game_shard_rust_shard_01 --tail 100 | Select-String "add_game"
```

---

### ✅ Test 4: Database Connection Drop (HIGH)

**Objective**: Verify database health checks detect failures and retry logic works.

**Steps**:
```powershell
# 1. Simulate network partition to database
docker network disconnect arbees_default timescaledb

# 2. Wait 90 seconds (3 health check failures at 30s interval)
Start-Sleep -Seconds 90

# 3. Restore connection
docker network connect arbees_default timescaledb

# 4. Wait 30 seconds for recovery
Start-Sleep -Seconds 30
```

**Expected Results**:
- ✅ Health check failures logged: "Database health check failed (attempt 1/3)"
- ✅ Critical alert sent after 3 consecutive failures
- ✅ Services continue running (degraded mode)
- ✅ Health checks resume after reconnection
- ✅ Recovery logged: "Database connection recovered"

**Verification**:
```powershell
# Check health check failures
docker logs orchestrator_rust --tail 100 | Select-String "health check failed"

# Check critical alert
Get-Content C:\arbees_data\logs\critical_alerts.log | Select-String "DatabaseConnectivityIssue"

# Check recovery
docker logs orchestrator_rust --tail 100 | Select-String "recovered"
```

---

### ✅ Test 5: All Shards Dead (CRITICAL)

**Objective**: Verify critical alert when all game monitoring stops.

**Steps**:
```powershell
# 1. Stop all game shards
docker-compose stop game_shard_rust_shard_01 game_shard_rust_shard_02

# 2. Wait 65 seconds (shard timeout + 5s)
Start-Sleep -Seconds 65

# 3. Check system monitor runs (every 30s)
Start-Sleep -Seconds 35
```

**Expected Results**:
- ✅ System monitor detects all shards unhealthy
- ✅ Critical alert sent: "AllShardsUnhealthy"
- ✅ Alert appears in Slack (if configured)
- ✅ Alert logged to file
- ✅ Orchestrator continues running (graceful degradation)

**Verification**:
```powershell
# Check system monitor alert
docker logs orchestrator_rust --tail 100 | Select-String "All.*shards are unhealthy"

# Check critical alert file
Get-Content C:\arbees_data\logs\critical_alerts.log | Select-String "AllShardsUnhealthy"

# Verify orchestrator still running
docker-compose ps orchestrator_rust
```

---

### ✅ Test 6: Zombie Game Cleanup (MEDIUM)

**Objective**: Verify orphaned games are removed from shards.

**Steps**:
```powershell
# 1. Manually inject a fake game assignment to shard (simulate stale state)
# This requires direct Redis access - for now, verify the logic through logs

# Instead, test by reassigning a game then checking heartbeat cleanup
# The shard will report a game that's not in orchestrator's assignment map

# 2. Watch heartbeat processing
docker logs orchestrator_rust --tail 100 -f | Select-String "zombie"
```

**Expected Results**:
- ✅ Zombie games detected in heartbeat handler
- ✅ Logs: "Zombie games detected on shard X: [game_id]"
- ✅ Remove commands sent to shard
- ✅ Shard stops monitoring the orphaned game

**Verification**:
```powershell
# Check for zombie detection
docker logs orchestrator_rust --tail 200 | Select-String "Zombie games"

# Check remove commands sent
docker logs orchestrator_rust --tail 200 | Select-String "Removing zombie game"
```

---

### ✅ Test 7: Network Partition & Recovery (HIGH)

**Objective**: Verify system handles temporary network issues gracefully.

**Steps**:
```powershell
# 1. Disconnect orchestrator from Redis network
docker network disconnect arbees_default orchestrator_rust

# 2. Wait 30 seconds - observe reconnection attempts
Start-Sleep -Seconds 30

# 3. Reconnect
docker network connect arbees_default orchestrator_rust

# 4. Wait 60 seconds for recovery
Start-Sleep -Seconds 60
```

**Expected Results**:
- ✅ Orchestrator logs reconnection attempts
- ✅ Exponential backoff visible in logs
- ✅ All subscriptions re-established after reconnect
- ✅ No data loss
- ✅ System returns to normal operation

**Verification**:
```powershell
# Check reconnection
docker logs orchestrator_rust --tail 100 | Select-String "reconnect"

# Check subscription restoration
docker logs orchestrator_rust --tail 100 | Select-String "Subscribed to"

# Verify heartbeats flowing
docker logs orchestrator_rust --tail 50 | Select-String "heartbeat"
```

---

### ✅ Test 8: High Load + Failure (CRITICAL)

**Objective**: Verify fault tolerance under load.

**Steps**:
```powershell
# 1. Start system with multiple games assigned (10+)
# Ensure orchestrator is discovering and assigning games

# 2. While under load, kill Redis
docker-compose stop redis

# 3. Wait 30 seconds
Start-Sleep -Seconds 30

# 4. Restart Redis
docker-compose start redis

# 5. Verify recovery
Start-Sleep -Seconds 60
```

**Expected Results**:
- ✅ Services handle concurrent reconnections
- ✅ No deadlocks or race conditions
- ✅ All games resume monitoring after recovery
- ✅ No duplicate game assignments
- ✅ No message loss

**Verification**:
```powershell
# Check for errors
docker logs orchestrator_rust --tail 200 | Select-String "error|panic|deadlock"

# Verify game count matches
docker logs orchestrator_rust --tail 100 | Select-String "game count"
```

---

### ✅ Test 9: Database Query Retry (HIGH)

**Objective**: Verify retry logic handles transient database failures.

**Steps**:
```powershell
# 1. Inject artificial load on TimescaleDB to simulate slow queries
docker exec timescaledb psql -U arbees -c "SELECT pg_sleep(5);"

# 2. While slow, trigger a write operation (game state update)
# Watch logs for retry behavior
```

**Expected Results**:
- ✅ First attempt may timeout
- ✅ Retry logged: "Retrying in Xms"
- ✅ Operation succeeds after retry
- ✅ No data loss

**Verification**:
```powershell
# Check for retry logs
docker logs game_shard_rust_shard_01 --tail 100 | Select-String "Retrying"
```

---

## Success Criteria Checklist

Before going live, ALL of these must be ✅:

### Redis Fault Tolerance
- [ ] Services reconnect within 60 seconds of Redis restart
- [ ] Exponential backoff with jitter observed in logs
- [ ] Circuit breaker opens after 10 consecutive failures
- [ ] No service crashes or permanent exits
- [ ] Zero message loss during reconnection window

### Game Shard Management
- [ ] Dead shards detected within 65 seconds
- [ ] Games automatically reassigned to healthy shards
- [ ] Restart detection works reliably
- [ ] Game resync completes within 10 seconds
- [ ] Zombie games removed automatically

### Critical Alerting
- [ ] "All shards unhealthy" alert sent and received
- [ ] Redis connectivity alert sent on failure
- [ ] Database connectivity alert sent on failure
- [ ] Alerts appear in Slack/webhook (if configured)
- [ ] Alert rate limiting works (5 min default)
- [ ] Alerts logged to file as fallback

### Database Resilience
- [ ] Health checks detect failures within 90 seconds
- [ ] Retry logic handles transient failures
- [ ] Services continue operating in degraded mode
- [ ] Recovery detected and logged
- [ ] Connection pool statistics tracked

### System Monitor
- [ ] Runs every 30 seconds
- [ ] Detects all critical failure conditions
- [ ] Integrates with alert system
- [ ] No false positives

### Graceful Degradation
- [ ] System continues operating with reduced capacity
- [ ] No cascading failures
- [ ] Recovery is automatic (no manual intervention)
- [ ] State consistency maintained

---

## Post-Testing Validation

### Log Analysis
```powershell
# 1. Check for any unexpected errors
docker logs orchestrator_rust 2>&1 | Select-String "error|panic|fatal"
docker logs game_shard_rust_shard_01 2>&1 | Select-String "error|panic|fatal"

# 2. Verify reconnection metrics
docker logs orchestrator_rust | Select-String "total reconnects"

# 3. Check critical alerts log
Get-Content C:\arbees_data\logs\critical_alerts.log
```

### Database Integrity
```powershell
# Connect to database and verify data consistency
docker exec -it timescaledb psql -U arbees -d arbees_db

# Check for orphaned records
SELECT COUNT(*) FROM game_states WHERE updated_at < NOW() - INTERVAL '5 minutes';

# Check paper trades consistency
SELECT status, COUNT(*) FROM paper_trades GROUP BY status;
```

### Performance Metrics
- Reconnection time: < 60 seconds
- Game reassignment time: < 10 seconds
- Alert delivery time: < 30 seconds
- Database retry success rate: > 95%

---

## Rollback Plan

If critical issues found during testing:

1. **Revert to previous deployment**:
   ```powershell
   git checkout main
   docker-compose down
   docker-compose up -d --build
   ```

2. **Disable specific features**:
   - Set `CRITICAL_ALERTS_ENABLED=false` to disable alerting
   - Set `DB_HEALTH_CHECK_ENABLED=false` to disable health monitoring
   - Remove auto-reassignment by commenting out reassignment logic

3. **Manual intervention**:
   - Restart services manually if reconnection fails
   - Reassign games manually via Redis commands
   - Monitor logs closely for issues

---

## Production Deployment Checklist

Before enabling for live trading:

- [ ] All chaos tests passed
- [ ] Slack webhook configured and tested
- [ ] Alert log directory exists with proper permissions
- [ ] Database pool tuned for production load
- [ ] Reconnection intervals appropriate for production
- [ ] Rate limits configured (alerts, resyncs, reassignments)
- [ ] Monitoring dashboards updated
- [ ] Runbook updated with failure response procedures
- [ ] Team trained on new alert types
- [ ] Gradual rollout plan defined (shadow mode → paper → live)

---

## Notes

- Run tests in a staging environment first
- Document any unexpected behaviors
- Save log files for analysis
- Test during both low and high activity periods
- Verify with multiple concurrent failures (e.g., Redis + shard crash)

**Remember**: The goal is zero manual intervention for transient failures!
