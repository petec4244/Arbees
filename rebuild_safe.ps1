# Safe rebuild - one service at a time with 30 second recovery pauses
# This prevents locking up the system

$services = @(
    "redis",
    "timescaledb",
    "crypto-spot-monitor",
    "kalshi_monitor",
    "polymarket_monitor",
    "crypto-zmq-bridge",
    "crypto_shard",
    "game_shard",
    "market-discovery-rust",
    "orchestrator",
    "execution_service",
    "signal_processor",
    "position_tracker",
    "api",
    "frontend",
    "signal-cli-rest-api",
    "notification_service_rust",
    "zmq_listener",
    "analytics_service"
)

$total = $services.Count
$completed = 0

foreach ($service in $services) {
    $completed++
    Write-Host "[$completed/$total] Building $service..." -ForegroundColor Green

    docker compose build --no-cache $service

    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: Failed to build $service" -ForegroundColor Red
        Write-Host "Continue? (Y/N)" -ForegroundColor Yellow
        $response = Read-Host
        if ($response -ne "Y" -and $response -ne "y") {
            exit 1
        }
    }

    Write-Host "$service completed. Pausing 30 seconds for system recovery..." -ForegroundColor Cyan
    Start-Sleep -Seconds 30
    Write-Host ""
}

Write-Host "All $total services rebuilt successfully!" -ForegroundColor Green
