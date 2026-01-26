# Docker Compose Memory Leak / Lockup Troubleshooting Guide

## Problem
Running `docker compose --profile full --profile vpn up` (or similar commands) causes the machine to lock up, likely due to memory leaks or recursive scanning issues.

## Root Causes Identified

1. **Large Build Context**: All services use `context: .` which sends the entire workspace to Docker daemon
   - **CRITICAL**: Docker transfers 1.6GB+ of context data even with `.dockerignore`
   - Docker must scan entire directory tree to determine exclusions
   - On Windows, this recursive scanning causes memory exhaustion
2. **Recursive Volume Mounts**: Multiple services mount `./:/app` causing Docker to watch/scan entire directory tree
3. **Windows Docker Desktop**: File watching on Windows can cause memory issues with large workspaces
4. **Missing Resource Limits**: No memory limits set, allowing containers to consume all available RAM

## CRITICAL FIX: Build Context Size

The `.dockerignore` file has been enhanced with aggressive exclusions, but Docker still scans everything. **The real solution is to build services individually or use smaller build contexts.**

## Fixes Applied

### 1. Enhanced .dockerignore
Added exclusions for:
- Documentation files (*.md)
- Scripts directory
- Test files
- Reports directory
- Cursor workspace files
- Docker files (to prevent recursion)
- Large data directories

### 2. Docker Compose Optimizations
- Added BuildKit support for faster, more efficient builds
- Added resource limits to prevent memory exhaustion
- Configured common build args

## Solutions

### Immediate Fix: Use Staged Startup

Instead of starting everything at once, start services in stages:

```powershell
# Stage 1: Infrastructure only
docker compose up -d timescaledb redis

# Wait for health checks
Start-Sleep -Seconds 10

# Stage 2: VPN (if needed)
docker compose --profile vpn up -d vpn

# Wait for VPN health check
Start-Sleep -Seconds 30

# Stage 3: Core services
docker compose --profile full up -d market-discovery-rust orchestrator

# Stage 4: Remaining services
docker compose --profile full up -d
```

### Alternative: Use Rebuild Scripts

Use the provided scripts that handle staged startup:
- `rebuild-all.ps1` - PowerShell script with staged startup
- `rebuild-all.bat` - Batch script with staged startup

### Long-term Fix: Optimize Volume Mounts

For production, consider removing development volume mounts (`./:/app`) and instead:
1. Copy only necessary files into containers during build
2. Use named volumes for persistent data
3. Mount only specific directories needed for runtime

### Windows-Specific Optimizations

1. **Increase Docker Desktop Memory**:
   - Open Docker Desktop → Settings → Resources
   - Increase Memory limit to at least 4GB (8GB recommended)
   - Increase Swap to 2GB

2. **Disable File Watching** (if not needed for development):
   ```yaml
   # In docker-compose.yml, add to services that don't need hot reload:
   volumes:
     - type: bind
       source: .
       target: /app
       bind:
         propagation: cached  # Reduces file watching overhead
   ```

3. **Use WSL2 Backend** (if available):
   - Docker Desktop → Settings → General
   - Enable "Use the WSL 2 based engine"
   - This provides better performance on Windows

### Build Optimization

Use BuildKit for faster builds:
```powershell
$env:DOCKER_BUILDKIT=1
$env:COMPOSE_DOCKER_CLI_BUILD=1
docker compose build
```

### Monitor Resource Usage

While building/starting:
```powershell
# Monitor Docker resource usage
docker stats

# Check Docker Desktop resource usage in Task Manager
# Look for "com.docker.backend" and "com.docker.proxy"
```

## Prevention

1. **Always use staged startup** for full stack
2. **Monitor .dockerignore** - ensure large directories are excluded
3. **Use resource limits** in docker-compose.yml
4. **Avoid mounting entire workspace** unless necessary for development
5. **Use BuildKit** for all builds

## Emergency Recovery

If machine locks up:

1. **Force stop Docker Desktop**:
   - Task Manager → End "Docker Desktop" process
   - Or: `taskkill /F /IM "Docker Desktop.exe"`

2. **Clean up**:
   ```powershell
   docker system prune -a --volumes
   ```

3. **Restart Docker Desktop** and try staged startup

## Verification

After applying fixes, verify:
```powershell
# Check build context size (should be reasonable)
docker build --dry-run -f services/api/Dockerfile .

# Check running containers
docker ps

# Check resource usage
docker stats --no-stream
```
