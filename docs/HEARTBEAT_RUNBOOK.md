# Heartbeat + Auto-Restart Runbook

This document describes how to verify, test, and troubleshoot the heartbeat and auto-restart system.

## Overview

Every Arbees service publishes periodic heartbeats to Redis. The Orchestrator's Supervisor monitors these heartbeats and automatically restarts unhealthy containers (with bounded retries and backoff).

### Key Components

- **HeartbeatPublisher** (`shared/arbees_shared/health/heartbeat.py`): Each service uses this to publish heartbeats.
- **Supervisor** (`services/orchestrator/supervisor.py`): Monitors heartbeats and restarts containers.
- **Redis Keys**: `health:hb:{service}:{instance}` (TTL-based liveness)
- **Redis Channel**: `health:heartbeats` (real-time pubsub)

### Configuration (Environment Variables)

| Variable | Default | Description |
|----------|---------|-------------|
| `HEARTBEAT_INTERVAL_SECS` | 10 | How often services publish heartbeats |
| `HEARTBEAT_TTL_SECS` | 35 | Redis key TTL (3x interval + buffer) |
| `HEARTBEAT_MISS_THRESHOLD` | 3 | Consecutive misses before action |
| `SUPERVISOR_ENABLED` | true | Enable/disable auto-restart |
| `MAX_RESTART_ATTEMPTS` | 3 | Max restart attempts before cooldown |
| `RESTART_BACKOFF_SECS` | 5,15,45 | Backoff between restart attempts |
| `RESTART_COOLDOWN_SECS` | 600 | Cooldown after max attempts exhausted |
| `SUPERVISOR_CHECK_INTERVAL_SECS` | 15 | How often supervisor checks health |

### Restart Allowlist (Stateless Services)

Only these services can be auto-restarted:
- `arbees-game-shard-1`
- `arbees-polymarket-monitor`
- `arbees-futures-monitor`
- `arbees-api`
- `arbees-frontend`
- `arbees-signal-processor`
- `arbees-execution-service`
- `arbees-position-tracker`
- `arbees-analytics`
- `arbees-market-discovery`

### Never Auto-Restart (Stateful/Critical)

- `arbees-timescaledb`
- `arbees-redis`
- `arbees-vpn`
- `arbees-orchestrator` (never restarts itself)

---

## Verification Steps

### 1. Check Heartbeat Keys in Redis

```bash
# Connect to Redis CLI
docker exec -it arbees-redis redis-cli

# List all heartbeat keys
KEYS health:hb:*

# Get a specific heartbeat
GET health:hb:game_shard:shard-1

# Check TTL (should be ~25-35 seconds if healthy)
TTL health:hb:game_shard:shard-1
```

Expected output:
```json
{
  "service": "game_shard",
  "instance_id": "shard-1",
  "status": "healthy",
  "started_at": "2026-01-23T19:00:00Z",
  "timestamp": "2026-01-23T19:05:30Z",
  "checks": {"redis_ok": true, "db_ok": true},
  "metrics": {"games_monitored": 5, "signals_generated": 12}
}
```

### 2. Subscribe to Heartbeat Channel

```bash
docker exec -it arbees-redis redis-cli SUBSCRIBE health:heartbeats
```

You should see heartbeats from all services every 10 seconds.

### 3. Check Orchestrator Logs for Supervisor

```bash
docker logs arbees-orchestrator 2>&1 | grep -i "supervisor\|health summary"
```

Expected:
```
INFO - Supervisor started (auto-restart enabled)
INFO - Health summary: 8 healthy, 0 degraded, 0 unhealthy, 0 missing
```

---

## Testing Auto-Restart

### Test 1: Kill a Stateless Container

```bash
# Kill game_shard (it's in the allowlist)
docker kill arbees-game-shard-1

# Watch orchestrator logs
docker logs -f arbees-orchestrator 2>&1 | grep -i "game-shard\|restart\|missing"
```

Expected sequence:
1. `Service arbees-game-shard-1 missing heartbeat (1 consecutive misses)`
2. `Service arbees-game-shard-1 missing heartbeat (2 consecutive misses)`
3. `Service arbees-game-shard-1 missing heartbeat (3 consecutive misses)`
4. `Attempting restart of arbees-game-shard-1 (attempt 1/3)`
5. `Successfully restarted arbees-game-shard-1`

### Test 2: Force Repeated Failures

```bash
# Set an invalid env var that causes crash on startup
docker exec arbees-game-shard-1 sh -c "echo 'invalid' > /tmp/crash"

# Kill the container
docker kill arbees-game-shard-1
```

After 3 failed restarts, you should see:
```
ERROR - Service arbees-game-shard-1 exhausted restart attempts (3/3)
ERROR - Published alert: {"type": "SERVICE_RESTART_FAILED", ...}
```

### Test 3: Verify Deny List

```bash
# Try to kill Redis (it's in the deny list)
docker kill arbees-redis

# Check orchestrator logs
docker logs arbees-orchestrator 2>&1 | grep -i "redis"
```

Expected:
```
WARNING - Service arbees-redis is unhealthy but in deny list - alerting only
```

---

## Checking Restart Attempt State

```bash
# Check restart attempts for a container
docker exec -it arbees-redis redis-cli GET health:restart:arbees-game-shard-1
```

Output:
```json
{
  "attempt_count": 2,
  "last_attempt_at": "2026-01-23T19:10:00Z",
  "last_failure_reason": "missing_heartbeat",
  "cooldown_until": null
}
```

### Reset Restart Attempts

```bash
# Clear restart state for a container
docker exec -it arbees-redis redis-cli DEL health:restart:arbees-game-shard-1
```

---

## Monitoring Alerts

Subscribe to system alerts:

```bash
docker exec -it arbees-redis redis-cli SUBSCRIBE system:alerts
```

Alert types:
- `HEALTH_SUMMARY`: Periodic summary of all service health
- `SERVICE_RESTART_FAILED`: Container restart failed or exhausted attempts

---

## Troubleshooting

### Heartbeats Not Appearing

1. Check if service is running: `docker ps | grep arbees-`
2. Check service logs for errors: `docker logs arbees-game-shard-1`
3. Verify Redis connectivity: `docker exec arbees-game-shard-1 python -c "import redis; r = redis.from_url('redis://redis:6379'); print(r.ping())"`

### Supervisor Not Restarting Containers

1. Check if supervisor is enabled: `docker logs arbees-orchestrator | grep -i supervisor`
2. Verify Docker socket is mounted: `docker exec arbees-orchestrator ls -la /var/run/docker.sock`
3. Check if container is in allowlist: Review `RESTART_ALLOW_SERVICES` in `supervisor.py`

### Container Stuck in Restart Loop

1. Check restart attempt state in Redis
2. Manually clear the restart state: `DEL health:restart:{container_name}`
3. Fix the underlying issue (check container logs)
4. Manually start the container: `docker start {container_name}`

---

## Disabling Auto-Restart

To disable auto-restart temporarily:

```bash
# In .env or docker-compose override
SUPERVISOR_ENABLED=false
```

Or set `MAX_RESTART_ATTEMPTS=0` to make supervisor alert-only.

---

## Service-Specific Health Checks

Each service reports different checks and metrics:

| Service | Checks | Metrics |
|---------|--------|---------|
| orchestrator | redis_ok, db_ok, discovery_rust_ok | shards_total, shards_healthy, games_assigned |
| game_shard | redis_ok, db_ok, kalshi_ws_ok, polymarket_via_redis | games_monitored, signals_generated, circuit_breaker_ok |
| polymarket_monitor | redis_ok, vpn_ok, ws_ok | subscribed_markets, prices_published, last_price_age_s |
| futures_monitor | redis_ok, db_ok, kalshi_ok, polymarket_ok | games_monitored, markets_cached |
| signal_processor | redis_ok, db_ok | signals_received, signals_approved, approval_rate_pct |
| execution_service | redis_ok, db_ok, paper_trading | executions_total, success_rate_pct, bankroll |
| position_tracker | redis_ok, db_ok | positions_open, positions_opened_total, bankroll |
| analytics_service | redis_ok, db_ok, archiver_ok, ml_analyzer_ok | archiver_pending, scheduled_jobs |
| market_discovery_rust | redis_ok | (none currently) |
