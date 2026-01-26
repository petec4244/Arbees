# Build services individually to avoid large build context issues
# This prevents Docker from scanning the entire workspace at once

param(
    [switch]$BuildAll = $false
)

$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Individual Service Builder" -ForegroundColor Cyan
Write-Host "Prevents memory issues by building one at a time" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Enable BuildKit
$env:DOCKER_BUILDKIT = "1"
$env:COMPOSE_DOCKER_CLI_BUILD = "1"

# Services to build (in dependency order)
$services = @(
    "market-discovery-rust",
    "orchestrator",
    "game_shard",
    "signal_processor",
    "execution_service",
    "position_tracker",
    "notification_service_rust",
    "api",
    "analytics_service",
    "signal-cli-rest-api",
    "polymarket_monitor",
    "frontend"
)

if ($BuildAll) {
    Write-Host "Building all services individually..." -ForegroundColor Yellow
    Write-Host ""
    
    foreach ($service in $services) {
        Write-Host "[Building $service]..." -ForegroundColor Cyan
        docker compose build $service
        
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  ❌ Failed to build $service" -ForegroundColor Red
            Write-Host "  Continuing with next service..." -ForegroundColor Yellow
        } else {
            Write-Host "  ✅ Successfully built $service" -ForegroundColor Green
        }
        Write-Host ""
        
        # Small delay to prevent overwhelming Docker
        Start-Sleep -Seconds 2
    }
    
    Write-Host "============================================" -ForegroundColor Green
    Write-Host "All services built!" -ForegroundColor Green
    Write-Host "============================================" -ForegroundColor Green
} else {
    Write-Host "Usage: .\build-individual-services.ps1 -BuildAll" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "This script builds services one at a time to prevent" -ForegroundColor Gray
    Write-Host "Docker from scanning the entire workspace simultaneously." -ForegroundColor Gray
    Write-Host ""
    Write-Host "Services that can be built:" -ForegroundColor Gray
    foreach ($service in $services) {
        Write-Host "  - $service" -ForegroundColor Gray
    }
}
