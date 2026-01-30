# Optimized Docker Build Script for Arbees
#
# This script helps you build services efficiently with proper caching
#
# Usage:
#   .\build.ps1                    # Show help
#   .\build.ps1 all                # Build all services (with caching)
#   .\build.ps1 orchestrator       # Build just orchestrator
#   .\build.ps1 shard              # Build just game shards
#   .\build.ps1 clean              # Clean Docker cache
#   .\build.ps1 up                 # Build and start everything

param(
    [Parameter(Position=0)]
    [string]$Command = "help",

    [switch]$NoCache = $false,
    [switch]$Parallel = $false
)

# Enable BuildKit for better caching (10x faster!)
$env:DOCKER_BUILDKIT = "1"
$env:COMPOSE_DOCKER_CLI_BUILD = "1"

function Write-Section {
    param([string]$Message)
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Host $Message -ForegroundColor Cyan
    Write-Host "========================================`n" -ForegroundColor Cyan
}

function Build-Service {
    param(
        [string]$ServiceName,
        [bool]$UseNoCache = $false
    )

    Write-Host "Building $ServiceName..." -ForegroundColor Green
    $timer = [System.Diagnostics.Stopwatch]::StartNew()

    if ($UseNoCache) {
        docker-compose build --no-cache $ServiceName
    } else {
        docker-compose build $ServiceName
    }

    $timer.Stop()
    Write-Host "✓ $ServiceName built in $($timer.Elapsed.TotalSeconds.ToString('0.0'))s" -ForegroundColor Green
}

switch ($Command.ToLower()) {
    "help" {
        Write-Section "Arbees Optimized Build System"
        Write-Host "Usage: .\build.ps1 COMMAND [options]"
        Write-Host ""
        Write-Host "Commands:" -ForegroundColor Yellow
        Write-Host "  all           Build all Rust services (with caching)"
        Write-Host "  orchestrator  Build orchestrator"
        Write-Host "  discovery     Build market-discovery-rust"
        Write-Host "  shard         Build game_shard"
        Write-Host "  signal        Build signal_processor"
        Write-Host "  execution     Build execution_service"
        Write-Host "  position      Build position_tracker"
        Write-Host "  notification  Build notification_service_rust"
        Write-Host "  zmq           Build zmq_listener"
        Write-Host ""
        Write-Host "  up            Build all and start services"
        Write-Host "  clean         Remove all Docker build cache"
        Write-Host "  prune         Deep clean (removes all unused images)"
        Write-Host ""
        Write-Host "Options:" -ForegroundColor Yellow
        Write-Host "  -NoCache      Force rebuild without cache"
        Write-Host "  -Parallel     Build services in parallel (faster but more CPU)"
        Write-Host ""
        Write-Host "Examples:" -ForegroundColor Yellow
        Write-Host "  .\build.ps1 all                    # Build everything (fast with cache)"
        Write-Host "  .\build.ps1 orchestrator           # Build just orchestrator"
        Write-Host "  .\build.ps1 all -NoCache           # Force full rebuild"
        Write-Host "  .\build.ps1 up                     # Build and start"
        Write-Host ""
        Write-Host "Performance Tips:" -ForegroundColor Green
        Write-Host "  * First build: ~5-10 min per service"
        Write-Host "  * With cache (code changes): 30-60 sec per service (10x faster!)"
        Write-Host "  * With cache (no changes): 5-10 sec per service (50x faster!)"
        Write-Host "  * Use -Parallel to build multiple services at once"
    }

    "all" {
        Write-Section "Building All Rust Services"
        $services = @(
            "orchestrator",
            "market-discovery-rust",
            "game_shard",
            "signal_processor",
            "execution_service",
            "position_tracker",
            "notification_service_rust",
            "zmq_listener"
        )

        if ($Parallel) {
            Write-Host "Building in parallel..." -ForegroundColor Yellow
            docker-compose build --parallel
        } else {
            foreach ($service in $services) {
                Build-Service -ServiceName $service -UseNoCache $NoCache
            }
        }
    }

    "orchestrator" {
        Write-Section "Building Orchestrator"
        Build-Service -ServiceName "orchestrator" -UseNoCache $NoCache
    }

    "discovery" {
        Write-Section "Building Market Discovery"
        Build-Service -ServiceName "market-discovery-rust" -UseNoCache $NoCache
    }

    "shard" {
        Write-Section "Building Game Shard"
        Build-Service -ServiceName "game_shard" -UseNoCache $NoCache
    }

    "signal" {
        Write-Section "Building Signal Processor"
        Build-Service -ServiceName "signal_processor" -UseNoCache $NoCache
    }

    "execution" {
        Write-Section "Building Execution Service"
        Build-Service -ServiceName "execution_service" -UseNoCache $NoCache
    }

    "position" {
        Write-Section "Building Position Tracker"
        Build-Service -ServiceName "position_tracker" -UseNoCache $NoCache
    }

    "notification" {
        Write-Section "Building Notification Service"
        Build-Service -ServiceName "notification_service_rust" -UseNoCache $NoCache
    }

    "zmq" {
        Write-Section "Building ZMQ Listener"
        Build-Service -ServiceName "zmq_listener" -UseNoCache $NoCache
    }

    "up" {
        Write-Section "Building and Starting All Services"
        $timer = [System.Diagnostics.Stopwatch]::StartNew()

        if ($Parallel) {
            docker-compose up -d --build --parallel
        } else {
            docker-compose up -d --build
        }

        $timer.Stop()
        Write-Host "`n✓ All services built and started in $($timer.Elapsed.TotalMinutes.ToString('0.0')) minutes" -ForegroundColor Green
        Write-Host "Run 'docker-compose ps' to check status" -ForegroundColor Yellow
    }

    "clean" {
        Write-Section "Cleaning Docker Build Cache"
        Write-Host "This will remove build cache but keep images" -ForegroundColor Yellow
        $confirm = Read-Host "Continue? (y/N)"
        if ($confirm -eq "y") {
            docker builder prune -f
            Write-Host "✓ Build cache cleaned" -ForegroundColor Green
        }
    }

    "prune" {
        Write-Section "Deep Clean (Remove All Unused Images)"
        Write-Host "WARNING: This will remove ALL unused Docker images!" -ForegroundColor Red
        $confirm = Read-Host "Are you sure? (y/N)"
        if ($confirm -eq "y") {
            docker system prune -a -f
            Write-Host "✓ Deep clean complete" -ForegroundColor Green
        }
    }

    default {
        Write-Host "Unknown command: $Command" -ForegroundColor Red
        Write-Host "Run '.\build.ps1 help' for usage information" -ForegroundColor Yellow
    }
}

Write-Host ""
