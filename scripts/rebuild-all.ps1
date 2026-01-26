# Arbees - Full Container Rebuild Script (PowerShell)
# Usage: .\rebuild-all.ps1 [-NoCache] [-SkipVpnWait]

param(
    [switch]$NoCache = $true,
    [switch]$SkipVpnWait = $false
)

$ErrorActionPreference = "Continue"
# Change to project root (parent of scripts folder)
Set-Location (Split-Path -Parent $PSScriptRoot)

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Arbees - Full Container Rebuild Script" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Step 1: Stop all containers
Write-Host "[1/4] Stopping all containers..." -ForegroundColor Yellow
docker-compose --profile full --profile vpn down 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full --profile vpn down
}
Write-Host "  Done!" -ForegroundColor Green
Write-Host ""

# Step 2: Rebuild all containers
Write-Host "[2/4] Rebuilding all containers..." -ForegroundColor Yellow
if ($NoCache) {
    Write-Host "  (using --no-cache)" -ForegroundColor Gray
    docker-compose --profile full --profile vpn build --no-cache 2>$null
    if ($LASTEXITCODE -ne 0) {
        docker compose --profile full --profile vpn build --no-cache
    }
} else {
    docker-compose --profile full --profile vpn build 2>$null
    if ($LASTEXITCODE -ne 0) {
        docker compose --profile full --profile vpn build
    }
}
Write-Host "  Done!" -ForegroundColor Green
Write-Host ""

# Step 3: Start infrastructure first
Write-Host "[3/4] Starting infrastructure (DB, Redis, VPN)..." -ForegroundColor Yellow

Write-Host "  Starting TimescaleDB and Redis..." -ForegroundColor Gray
docker-compose up -d timescaledb redis 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose up -d timescaledb redis
}

Write-Host "  Waiting for DB and Redis to be healthy..." -ForegroundColor Gray
Start-Sleep -Seconds 10

Write-Host "  Starting VPN..." -ForegroundColor Gray
docker-compose --profile vpn up -d vpn 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile vpn up -d vpn
}

if (-not $SkipVpnWait) {
    Write-Host "  Waiting for VPN to become healthy..." -ForegroundColor Gray
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
}
Write-Host ""

# Step 4: Start all remaining services
Write-Host "[4/4] Starting all services..." -ForegroundColor Yellow
docker-compose --profile full --profile vpn up -d 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full --profile vpn up -d
}
Write-Host "  Done!" -ForegroundColor Green
Write-Host ""

# Wait for services to initialize
Write-Host "Waiting for services to initialize..." -ForegroundColor Gray
Start-Sleep -Seconds 15

# Show final status
Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Final Container Status:" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
docker-compose --profile full --profile vpn ps 2>$null
if ($LASTEXITCODE -ne 0) {
    docker compose --profile full --profile vpn ps
}

Write-Host ""
Write-Host "============================================" -ForegroundColor Green
Write-Host "Rebuild complete!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
