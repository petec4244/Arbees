# Safe Docker Compose Startup Script
# Prevents memory leaks by starting services in stages

param(
    [switch]$SkipVpn = $false,
    [switch]$Build = $false
)

$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Safe Docker Compose Startup" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Enable BuildKit for better performance
$env:DOCKER_BUILDKIT = "1"
$env:COMPOSE_DOCKER_CLI_BUILD = "1"

# Step 1: Build if requested
if ($Build) {
    Write-Host "[1/5] Building containers..." -ForegroundColor Yellow
    docker compose --profile full --profile vpn build 2>$null
    if ($LASTEXITCODE -ne 0) {
        docker-compose --profile full --profile vpn build
    }
    Write-Host "  Done!" -ForegroundColor Green
    Write-Host ""
}

# Step 2: Stop any existing containers
Write-Host "[2/5] Stopping existing containers..." -ForegroundColor Yellow
docker compose --profile full --profile vpn down 2>$null
if ($LASTEXITCODE -ne 0) {
    docker-compose --profile full --profile vpn down
}
Write-Host "  Done!" -ForegroundColor Green
Write-Host ""

# Step 3: Start infrastructure (DB, Redis)
Write-Host "[3/5] Starting infrastructure (TimescaleDB, Redis)..." -ForegroundColor Yellow
docker compose up -d timescaledb redis 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose up -d timescaledb redis
}

Write-Host "  Waiting for health checks..." -ForegroundColor Gray
$maxWait = 60
$waited = 0
while ($waited -lt $maxWait) {
    $dbStatus = docker inspect arbees-timescaledb --format="{{.State.Health.Status}}" 2>$null
    $redisStatus = docker inspect arbees-redis --format="{{.State.Health.Status}}" 2>$null
    
    if ($dbStatus -eq "healthy" -and $redisStatus -eq "healthy") {
        Write-Host "  Infrastructure is healthy!" -ForegroundColor Green
        break
    }
    
    Write-Host "    DB: $dbStatus, Redis: $redisStatus (waited ${waited}s)" -ForegroundColor Gray
    Start-Sleep -Seconds 5
    $waited += 5
}

if ($waited -ge $maxWait) {
    Write-Host "  Warning: Infrastructure did not become healthy within ${maxWait}s" -ForegroundColor Red
}
Write-Host ""

# Step 4: Start VPN if needed
if (-not $SkipVpn) {
    Write-Host "[4/5] Starting VPN..." -ForegroundColor Yellow
    docker compose --profile vpn up -d vpn 2>$null
    if ($LASTEXITCODE -ne 0) {
        docker compose --profile vpn up -d vpn
    }
    
    Write-Host "  Waiting for VPN health check..." -ForegroundColor Gray
    $maxWait = 120
    $waited = 0
    while ($waited -lt $maxWait) {
        $status = docker inspect arbees-vpn --format="{{.State.Health.Status}}" 2>$null
        if ($status -eq "healthy") {
            Write-Host "  VPN is healthy!" -ForegroundColor Green
            break
        }
        Write-Host "    VPN status: $status (waited ${waited}s)" -ForegroundColor Gray
        Start-Sleep -Seconds 5
        $waited += 5
    }
    if ($waited -ge $maxWait) {
        Write-Host "  Warning: VPN did not become healthy within ${maxWait}s" -ForegroundColor Red
    }
    Write-Host ""
}

# Step 5: Start remaining services in batches
Write-Host "[5/5] Starting application services..." -ForegroundColor Yellow

# Batch 1: Discovery services
Write-Host "  Starting discovery services..." -ForegroundColor Gray
docker compose --profile full up -d market-discovery-rust orchestrator 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full up -d market-discovery-rust orchestrator
}
Start-Sleep -Seconds 5

# Batch 2: Core game services
Write-Host "  Starting core game services..." -ForegroundColor Gray
docker compose --profile full up -d game_shard signal_processor execution_service position_tracker 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full up -d game_shard signal_processor execution_service position_tracker
}
Start-Sleep -Seconds 5

# Batch 3: Supporting services
Write-Host "  Starting supporting services..." -ForegroundColor Gray
docker compose --profile full up -d api analytics_service notification_service_rust signal-cli-rest-api 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full up -d api analytics_service notification_service_rust signal-cli-rest-api
}
Start-Sleep -Seconds 5

# Batch 4: VPN-dependent services
if (-not $SkipVpn) {
    Write-Host "  Starting VPN-dependent services..." -ForegroundColor Gray
    docker compose --profile vpn up -d polymarket_monitor 2>$null
    if ($LASTEXITCODE -ne 0) {
        docker compose --profile vpn up -d polymarket_monitor
    }
}

# Batch 5: Frontend (if needed)
Write-Host "  Starting frontend..." -ForegroundColor Gray
docker compose --profile full up -d frontend 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full up -d frontend
}

Write-Host "  Done!" -ForegroundColor Green
Write-Host ""

# Final status
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Container Status:" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
docker compose --profile full --profile vpn ps 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full --profile vpn ps
}

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "Startup complete!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
Write-Host ""
Write-Host "Monitor logs with:" -ForegroundColor Gray
Write-Host "  docker compose logs -f [service_name]" -ForegroundColor Gray
Write-Host ""
Write-Host "Check resource usage with:" -ForegroundColor Gray
Write-Host "  docker stats" -ForegroundColor Gray
