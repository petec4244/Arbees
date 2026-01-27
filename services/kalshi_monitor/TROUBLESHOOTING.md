# Kalshi Monitor Troubleshooting Guide

## Common Issues and Fixes

### 1. WebSocket Connection Failures

**Symptoms:**
- "Failed to connect to Kalshi WebSocket" errors
- "Not connected to Kalshi WebSocket" errors

**Causes:**
- Missing or invalid `KALSHI_API_KEY` environment variable
- Missing or invalid `KALSHI_PRIVATE_KEY` environment variable
- Network connectivity issues

**Fix:**
- Verify environment variables are set in `.env`:
  ```
  KALSHI_API_KEY=your_api_key
  KALSHI_PRIVATE_KEY="-----BEGIN RSA PRIVATE KEY-----..."
  ```
- Check that the private key is properly formatted (includes BEGIN/END markers)
- Verify network connectivity to `wss://api.elections.kalshi.com/trade-api/ws/v2`

### 2. No Market Assignments Received

**Symptoms:**
- Monitor starts but never receives assignments
- "Streaming prices for 0 Kalshi markets" message persists

**Causes:**
- Orchestrator not publishing `kalshi_assign` messages
- Redis subscription not working
- Wrong channel name

**Fix:**
- Verify orchestrator is running and finding Kalshi markets
- Check Redis logs for published messages:
  ```bash
  docker exec arbees-redis redis-cli MONITOR | grep market_assignments
  ```
- Verify the channel name matches: `orchestrator:market_assignments`

### 3. Price Updates Not Publishing

**Symptoms:**
- Subscribed to markets but no prices published
- "Unknown ticker" debug messages

**Causes:**
- Ticker not in `_ticker_to_info` mapping
- Market assignment received but subscription failed
- WebSocket disconnection during streaming

**Fix:**
- Check logs for subscription confirmations
- Verify ticker format matches what orchestrator sends
- Check WebSocket connection health

### 4. Import Errors

**Symptoms:**
- `ModuleNotFoundError` for `arbees_shared` or `markets.kalshi`

**Causes:**
- Missing dependencies in Docker image
- PYTHONPATH not set correctly

**Fix:**
- Verify Dockerfile installs all dependencies
- Check that `pyproject.toml` includes all required packages
- Ensure PYTHONPATH includes `/app` and `/app/shared`

## Debugging Steps

1. **Check logs:**
   ```bash
   docker logs arbees-kalshi-monitor -f
   ```

2. **Verify Redis connection:**
   ```bash
   docker exec arbees-kalshi-monitor python -c "from arbees_shared.messaging.redis_bus import RedisBus; import asyncio; rb = RedisBus(); asyncio.run(rb.connect()); print('Connected')"
   ```

3. **Test WebSocket connection:**
   ```bash
   docker exec arbees-kalshi-monitor python -c "from markets.kalshi.websocket.ws_client import KalshiWebSocketClient; import asyncio; ws = KalshiWebSocketClient(); asyncio.run(ws.connect()); print('Connected')"
   ```

4. **Check for assignments:**
   ```bash
   docker exec arbees-redis redis-cli PUBSUB CHANNELS orchestrator:market_assignments
   ```

## Recent Fixes Applied

1. **Fixed `_assignment_listener()` blocking issue:**
   - Added loop to keep task alive while running
   - Prevents early termination of the listener task

2. **Improved error handling:**
   - Added connection checks before streaming
   - Better error messages for WebSocket failures
   - Added exception handling for subscription failures

3. **Enhanced connection monitoring:**
   - Check connection status before streaming
   - Handle disconnections gracefully
   - Reconnection handled by KalshiWebSocketClient

## Architecture Notes

- **No VPN required:** Kalshi API is accessible from US
- **WebSocket streaming:** Uses `KalshiWebSocketClient` for real-time prices
- **Redis pub/sub:** Receives assignments from orchestrator, publishes prices to `game:{game_id}:price`
- **Same format as Polymarket:** Prices published in same format for consistency
