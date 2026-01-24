# Unified Team Matching Architecture

## Overview

All team name matching in Arbees is performed by a single source of truth: the **Rust-based market-discovery-rust service** via Redis RPC.

This ensures consistent matching results across:
- Kalshi market discovery (Orchestrator)
- Polymarket discovery (already Rust)
- Entry price validation (SignalProcessor)
- Exit price validation (PositionTracker)

## Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Orchestrator  │     │ SignalProcessor │     │PositionTracker  │
└────────┬────────┘     └────────┬────────┘     └────────┬────────┘
         │                       │                       │
         │    team:match:request │                       │
         └───────────────────────┼───────────────────────┘
                                 │
                                 ▼
                    ┌────────────────────────┐
                    │        Redis           │
                    │    (pub/sub bus)       │
                    └────────────┬───────────┘
                                 │
                                 ▼
                    ┌────────────────────────┐
                    │  market-discovery-rust │
                    │  (Rust team matcher)   │
                    └────────────┬───────────┘
                                 │
                                 │ team:match:response:{request_id}
                                 ▼
                    ┌────────────────────────┐
                    │        Redis           │
                    └────────────────────────┘
```

## RPC Contract

### Request Channel
`team:match:request`

### Response Channel
`team:match:response:{request_id}`

### Request Payload
```json
{
    "request_id": "uuid-v4-string",
    "target_team": "Boston Celtics",
    "candidate_team": "Celtics win tonight",
    "sport": "nba"
}
```

### Response Payload
```json
{
    "request_id": "uuid-v4-string",
    "is_match": true,
    "confidence": 0.85,
    "method": "High",
    "reason": "Mascot match: celtics"
}
```

## Python Client Usage

```python
from arbees_shared.team_matching import TeamMatchingClient

# Initialize and connect
client = TeamMatchingClient()
await client.connect()

# Match teams
result = await client.match_teams(
    target_team="Boston Celtics",
    candidate_team="Celtics",
    sport="nba",
    timeout=2.0,
)

# Check result
if result and result.is_match and result.confidence >= 0.7:
    # High confidence match
    print(f"Match found: {result}")
elif result is None:
    # Service unavailable - fail closed
    print("Team matching unavailable")

# Cleanup
await client.disconnect()
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TEAM_MATCH_MIN_CONFIDENCE` | `0.7` | Minimum confidence threshold for accepting a match |
| `TEAM_MATCH_RPC_TIMEOUT_SECS` | `2.0` | Timeout for RPC calls to the Rust service |
| `REDIS_URL` | `redis://redis:6379` | Redis connection URL |

## Fail-Closed Behavior

The system uses **fail-closed** semantics:

- **On timeout**: Returns `None`, callers reject entries or skip exits
- **On service unavailable**: Returns `None`, callers reject entries or skip exits
- **On low confidence**: Returns result with `is_match=False`, callers reject entries or skip exits

This prevents wrong-team trades from occurring when the matching service is unavailable.

## Confidence Levels

| Level | Score Range | Description |
|-------|-------------|-------------|
| `Exact` | 1.0 | Exact normalized string match |
| `High` | 0.85+ | Alias match, mascot match, or strong word overlap |
| `Medium` | 0.6-0.85 | Partial word overlap or fuzzy match |
| `Low` | < 0.6 | Weak fuzzy match only |
| `None` | 0.0 | No match found |

## Operational Smoke Tests

### Redis CLI Test

```bash
# Terminal 1: Subscribe to responses
redis-cli PSUBSCRIBE "team:match:response:*"

# Terminal 2: Publish request
redis-cli PUBLISH team:match:request '{"request_id":"test-123","target_team":"Boston Celtics","candidate_team":"Celtics","sport":"nba"}'

# Expected response in Terminal 1:
# {"request_id":"test-123","is_match":true,"confidence":0.85,"method":"High","reason":"Mascot match: celtics"}
```

### Docker Service Test

```bash
# Start services
docker-compose --profile full up -d

# Check logs for team matching
docker logs arbees-market-discovery 2>&1 | grep -i "team match"

# Check for RPC timeouts
docker logs arbees-signal-processor 2>&1 | grep -i "rpc_timeout"
```

## Migration from Old Matchers

The following Python implementations have been deprecated:

1. **`arbees_shared.utils.team_validator`** - Deprecated with warning
2. **Orchestrator's `_get_team_aliases()` / `_match_team_in_text()`** - Kept for backward compatibility, but not used for primary matching

All new code should use `arbees_shared.team_matching.TeamMatchingClient`.

## Troubleshooting

### RPC Timeouts

If you see frequent RPC timeouts:

1. Check that `market-discovery-rust` is running:
   ```bash
   docker ps | grep market-discovery
   ```

2. Check Redis connectivity:
   ```bash
   docker exec arbees-redis redis-cli ping
   ```

3. Check service logs:
   ```bash
   docker logs arbees-market-discovery --tail 100
   ```

### Wrong Team Matches

If wrong team matches occur:

1. Check the confidence threshold:
   ```bash
   echo $TEAM_MATCH_MIN_CONFIDENCE  # Should be >= 0.7
   ```

2. Review trace logs for low-confidence matches:
   ```bash
   grep "market_lookup_selected" .cursor/debug.log | jq -r 'select(.confidence < 0.7)'
   ```

3. Test specific team pairs via Redis CLI to see raw matching results.
