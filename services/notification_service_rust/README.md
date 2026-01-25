## `notification_service_rust`

Consumes Redis pub/sub `notification:events` and sends phone notifications via Signal.

### Architecture

`trading_service -> Redis (notification:events) -> notification_service_rust -> signal-cli-rest-api -> Signal`

### Local setup (Docker)

- Start the services:

```bash
docker-compose --profile full up -d redis signal-cli-rest-api notification_service_rust
```

- Link the Signal account/device (recommended): open the QR code in a browser on the host:

`http://localhost:9922/v1/qrcodelink?device_name=arbees`

Then in Signal mobile: **Settings → Linked devices → +** and scan the QR.

### Environment

Required (in `.env`):
- `SIGNAL_SENDER_NUMBER`: the Signal account number registered/linked in `signal-cli-rest-api`
- `SIGNAL_RECIPIENTS`: comma-separated list of recipient numbers and/or group ids (`group.*`)

Optional:
- `QUIET_HOURS_ENABLED` (default `true`)
- `QUIET_HOURS_START` / `QUIET_HOURS_END` (default `22:00` / `07:00`)
- `QUIET_HOURS_TIMEZONE` (default `America/New_York`)
- `QUIET_HOURS_MIN_PRIORITY` (default `CRITICAL`)
- `RATE_LIMIT_MAX_PER_MINUTE` (default `10`)
- `RATE_LIMIT_BYPASS_CRITICAL` (default `true`)

### Manual test

Publish a test event:

```bash
docker exec arbees-redis redis-cli PUBLISH notification:events "{\"type\":\"trade_entry\",\"priority\":\"INFO\",\"data\":{\"game_id\":\"test\",\"sport\":\"nba\",\"team\":\"Lakers\",\"side\":\"buy\",\"price\":0.55,\"size\":25.5}}"
```

Expected:
- `notification_service_rust` logs receipt and Signal send success
- the configured recipient(s) receive a Signal message

