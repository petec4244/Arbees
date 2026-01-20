# Security Fixes - January 20, 2026

## Summary

Fixed hardcoded credentials and SQL injection vulnerability discovered during security audit.

## Issues Fixed

### 1. Hardcoded Database Password in docker-compose.yml
- **File:** `docker-compose.yml`
- **Issue:** `POSTGRES_PASSWORD: arbees_dev` hardcoded
- **Fix:** Changed to `${POSTGRES_PASSWORD:?required}` - requires env var

### 2. Hardcoded DATABASE_URL in docker-compose.yml
- **File:** `docker-compose.yml`
- **Issue:** `DATABASE_URL: postgresql://arbees:arbees_dev@...` repeated 4 times
- **Fix:** Changed to use `${POSTGRES_PASSWORD}` variable

### 3. Fallback Defaults in docker-compose.prod.yml
- **File:** `docker-compose.prod.yml`
- **Issue:** `${DB_PASSWORD:-arbees_prod}` had insecure fallback
- **Fix:** Removed fallbacks for sensitive values

### 4. Hardcoded Default in connection.py
- **File:** `shared/arbees_shared/db/connection.py`
- **Issue:** Default connection string with password
- **Fix:** Now raises `RuntimeError` if `DATABASE_URL` not set

### 5. SQL Injection Vulnerability
- **File:** `shared/arbees_shared/db/connection.py:467`
- **Issue:** `f" AND signal_type = '{signal_type}'"` - direct string interpolation
- **Fix:** Converted to parameterized query with `$1`, `$2` placeholders

### 6. Password Rotation
- **Action:** Rotated database password from `arbees_dev` to secure random value
- **New password stored in:** `.env` (gitignored)

## Files Modified

1. `docker-compose.yml`
2. `docker-compose.prod.yml`
3. `shared/arbees_shared/db/connection.py`
4. `.env.example`
5. `.env` (password added)

## Setup Required After These Changes

```bash
# Copy example and set secure password
cp .env.example .env
# Edit .env and set POSTGRES_PASSWORD

# Or generate one:
openssl rand -hex 24
```

## Verification

All services verified healthy after changes:
- arbees-api: healthy
- arbees-orchestrator: healthy
- arbees-game-shard-1: healthy
- arbees-position-manager: healthy
- arbees-timescaledb: healthy
- arbees-redis: healthy
- arbees-frontend: running

Data preserved: 32 paper trades intact.
