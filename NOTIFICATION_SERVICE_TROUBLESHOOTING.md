# Notification Service Troubleshooting Guide

## Current Status

The `notification_service_rust` container is running and connected to Redis, but notifications are not being sent because:

1. **Signal device is not linked** - No Signal account is registered in `signal-cli-rest-api`
2. **Phone number format** - May need `+` prefix
3. **JSON parsing issues** - Manual test messages via redis-cli may not match expected format

## Issues Found

### Issue 1: Signal Device Not Linked

**Symptom:**
```bash
docker exec arbees-signal-cli-rest-api curl -s http://localhost:8080/v1/accounts
# Returns: []
```

**Solution:**
1. Get the QR code link:
   ```bash
   # Open in browser on host machine:
   http://localhost:9922/v1/qrcodelink?device_name=arbees
   ```

2. Link device in Signal mobile app:
   - Open Signal mobile app
   - Go to **Settings → Linked devices → +**
   - Scan the QR code from the browser

3. Verify registration:
   ```bash
   docker exec arbees-signal-cli-rest-api curl -s http://localhost:8080/v1/accounts
   # Should return: [{"number":"+16034984244"}]
   ```

### Issue 2: Phone Number Format

**Current:** `SIGNAL_SENDER_NUMBER=16034984244`  
**Should be:** `SIGNAL_SENDER_NUMBER=+16034984244`

**Fix:**
Update `.env` file:
```bash
SIGNAL_SENDER_NUMBER=+16034984244
SIGNAL_RECIPIENTS=+16034984244
```

Then restart:
```bash
docker compose --profile full restart notification_service_rust
```

### Issue 3: JSON Parsing Errors

**Symptom:**
```
[WARN] notification event: invalid JSON: key must be a string at line 1 column 2
```

**Cause:**
Manual `redis-cli PUBLISH` commands send raw JSON strings, but Redis may encode them differently than the Rust `RedisBus.publish()` method which uses `serde_json::to_string()`.

**Solution:**
Use the proper NotificationEvent format when testing manually. The expected format is:
```json
{
  "type": "trade_entry",
  "priority": "CRITICAL",
  "data": {
    "message": "Test notification"
  }
}
```

**Test command (PowerShell issue - quotes get stripped):**
```bash
# PowerShell strips quotes, so use Python script instead:
python scripts/test-notification.py

# Or use the PowerShell script (uses here-string to preserve quotes):
.\scripts\test-notification.ps1
```

**Note:** Manual `redis-cli PUBLISH` from PowerShell strips JSON quotes. Use the Python script (`scripts/test-notification.py`) for reliable testing.

## Verification Steps

### 1. Check Signal CLI Status
```bash
# Check if device is linked
docker exec arbees-signal-cli-rest-api curl -s http://localhost:8080/v1/accounts

# Check API health
docker exec arbees-signal-cli-rest-api curl -s http://localhost:8080/v1/about
```

### 2. Check Notification Service Logs
```bash
docker compose --profile full logs -f notification_service_rust
```

### 3. Test Notification Flow

**Step 1:** Verify Redis connectivity
```bash
docker exec arbees-redis redis-cli PING
# Should return: PONG
```

**Step 2:** Send test notification
```bash
docker exec arbees-redis redis-cli PUBLISH "notification:events" '{"type":"trade_entry","priority":"CRITICAL","data":{"message":"Test notification"}}'
```

**Step 3:** Check logs for:
- `[INFO] Sent notification: type=TradeEntry priority=Critical`
- Or errors if Signal API call fails

### 4. Test Signal API Directly

Once device is linked, test sending directly:
```bash
docker exec arbees-signal-cli-rest-api curl -s -X POST http://localhost:8080/v2/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+16034984244",
    "recipients": ["+16034984244"],
    "message": "Direct test from troubleshooting"
  }'
```

## Expected Behavior

After fixing all issues:

1. **Notification service starts:**
   ```
   [INFO] Starting Rust Notification Service...
   [INFO] Config: redis_url=redis://redis:6379 recipients=1 quiet_hours=true rate_limit=10/min
   [INFO] Connected to Redis
   [INFO] Subscribed to notification:events
   ```

2. **Receives notification:**
   ```
   [INFO] Sent notification: type=TradeEntry priority=Critical
   ```

3. **Signal message received** on configured recipient phone number

## Common Errors

### Error: "Signal API non-2xx: 400"
- **Cause:** Device not linked or invalid phone number format
- **Fix:** Link device via QR code and ensure phone numbers have `+` prefix

### Error: "invalid JSON: key must be a string"
- **Cause:** Manual redis-cli publish with malformed JSON
- **Fix:** Use proper JSON format matching NotificationEvent structure

### Error: "SIGNAL_SENDER_NUMBER must be set"
- **Cause:** Environment variable not set
- **Fix:** Add `SIGNAL_SENDER_NUMBER=+16034984244` to `.env` and restart container

### Error: "SIGNAL_RECIPIENTS must be set"
- **Cause:** Environment variable not set or empty
- **Fix:** Add `SIGNAL_RECIPIENTS=+16034984244` to `.env` and restart container

## Next Steps

1. **Link Signal device** via QR code (most critical)
2. **Update phone number format** in `.env` (add `+` prefix)
3. **Restart notification service** after changes
4. **Test with proper notification event** format
5. **Monitor logs** to verify successful sends
