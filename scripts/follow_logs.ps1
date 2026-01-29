<#
.SYNOPSIS
    Follow multiple Arbees container logs simultaneously.

.DESCRIPTION
    Tails logs from multiple Docker containers. By default shows market monitors,
    game shard, and signal processor. Use flags to customize which services to follow.

.PARAMETER Core
    Follow only game_shard and signal_processor (the core trading pipeline)

.PARAMETER WithExec
    Include execution_service (implies -Core)

.PARAMETER All
    Follow all services including execution_service

.PARAMETER Monitors
    Follow only market monitors (kalshi + polymarket)

.EXAMPLE
    .\follow_logs.ps1
    Follow default: kalshi, polymarket, game_shard, signal_processor

.EXAMPLE
    .\follow_logs.ps1 -Core
    Follow only game_shard and signal_processor

.EXAMPLE
    .\follow_logs.ps1 -WithExec
    Follow game_shard, signal_processor, and execution_service

.EXAMPLE
    .\follow_logs.ps1 -All
    Follow all 5 services

.EXAMPLE
    .\follow_logs.ps1 -Monitors
    Follow only kalshi and polymarket monitors
#>

param(
    [Alias("c")]
    [switch]$Core,

    [Alias("e")]
    [switch]$WithExec,

    [Alias("a")]
    [switch]$All,

    [Alias("m")]
    [switch]$Monitors,

    [Alias("n")]
    [int]$Tail = 50
)

# Service names (must match docker-compose.yml)
$SVC_KALSHI = "kalshi_monitor"
$SVC_POLY = "polymarket_monitor"
$SVC_SHARD = "game_shard_rust"
$SVC_SIGNAL = "signal_processor_rust"
$SVC_EXEC = "execution_service"

# Build service list based on flags
$services = @()

if ($Monitors) {
    $services = @($SVC_KALSHI, $SVC_POLY)
    Write-Host "Following: Market Monitors only" -ForegroundColor Cyan
}
elseif ($Core) {
    $services = @($SVC_SHARD, $SVC_SIGNAL)
    Write-Host "Following: Core pipeline (game_shard + signal_processor)" -ForegroundColor Cyan
}
elseif ($WithExec) {
    $services = @($SVC_SHARD, $SVC_SIGNAL, $SVC_EXEC)
    Write-Host "Following: Core pipeline + Execution" -ForegroundColor Cyan
}
elseif ($All) {
    $services = @($SVC_KALSHI, $SVC_POLY, $SVC_SHARD, $SVC_SIGNAL, $SVC_EXEC)
    Write-Host "Following: All services" -ForegroundColor Cyan
}
else {
    # Default: monitors + core (no execution)
    $services = @($SVC_KALSHI, $SVC_POLY, $SVC_SHARD, $SVC_SIGNAL)
    Write-Host "Following: Default (monitors + core pipeline)" -ForegroundColor Cyan
}

Write-Host "Services: $($services -join ', ')" -ForegroundColor Gray
Write-Host "Tail: $Tail lines" -ForegroundColor Gray
Write-Host "Press Ctrl+C to stop" -ForegroundColor Yellow
Write-Host ""

# Run docker compose logs with follow
$serviceArgs = $services -join " "
$cmd = "docker logs -f $serviceArgs --tail=$Tail"

Write-Host "Running: $cmd" -ForegroundColor DarkGray
Write-Host ("=" * 60) -ForegroundColor DarkGray

# Change to project root and run
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    Invoke-Expression $cmd
}
finally {
    Pop-Location
}
