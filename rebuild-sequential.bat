@echo off
setlocal

:: Disable BuildKit to avoid Windows-specific build context issues
set DOCKER_BUILDKIT=0
set COMPOSE_DOCKER_CLI_BUILD=0

echo ============================================
echo Arbees - Sequential Container Rebuild Script
echo ============================================
echo.
echo This script builds containers ONE AT A TIME to prevent
echo system lockups from parallel Rust compilation.
echo.

cd /d "%~dp0"

echo [1/5] Stopping all containers...
docker-compose --profile full --profile vpn down
echo.

echo [2/5] Building infrastructure containers (fast, no Rust)...
docker-compose build timescaledb redis 2>nul
echo   Infrastructure uses pre-built images, skipping build.
echo.

echo [3/5] Building Python containers (one at a time)...
echo   Building api...
docker-compose build api
if %ERRORLEVEL% neq 0 echo   WARNING: api build had issues

echo   Building analytics_service...
docker-compose build analytics_service
if %ERRORLEVEL% neq 0 echo   WARNING: analytics_service build had issues

echo   Building kalshi_monitor...
docker-compose build kalshi_monitor
if %ERRORLEVEL% neq 0 echo   WARNING: kalshi_monitor build had issues

echo   Building polymarket_monitor...
docker-compose build polymarket_monitor
if %ERRORLEVEL% neq 0 echo   WARNING: polymarket_monitor build had issues

echo   Building frontend...
docker-compose build frontend
if %ERRORLEVEL% neq 0 echo   WARNING: frontend build had issues
echo.

echo [4/5] Building Rust containers (ONE AT A TIME - memory intensive)...
echo.
echo   NOTE: Each Rust build may use 2-4GB RAM. Building sequentially
echo   to prevent system lockups.
echo.

echo   [Rust 1/8] Building market-discovery-rust...
docker-compose build market-discovery-rust
if %ERRORLEVEL% neq 0 (
    echo   ERROR: market-discovery-rust build failed!
    goto :rust_error
)

echo   [Rust 2/8] Building orchestrator...
docker-compose build orchestrator
if %ERRORLEVEL% neq 0 (
    echo   ERROR: orchestrator build failed!
    goto :rust_error
)

echo   [Rust 3/8] Building game_shard...
docker-compose build game_shard
if %ERRORLEVEL% neq 0 (
    echo   ERROR: game_shard build failed!
    goto :rust_error
)

echo   [Rust 4/8] Building signal_processor...
docker-compose build signal_processor
if %ERRORLEVEL% neq 0 (
    echo   ERROR: signal_processor build failed!
    goto :rust_error
)

echo   [Rust 5/8] Building execution_service...
docker-compose build execution_service
if %ERRORLEVEL% neq 0 (
    echo   ERROR: execution_service build failed!
    goto :rust_error
)

echo   [Rust 6/8] Building position_tracker...
docker-compose build position_tracker
if %ERRORLEVEL% neq 0 (
    echo   ERROR: position_tracker build failed!
    goto :rust_error
)

echo   [Rust 7/8] Building notification_service_rust...
docker-compose build notification_service_rust
if %ERRORLEVEL% neq 0 (
    echo   ERROR: notification_service_rust build failed!
    goto :rust_error
)

echo   [Rust 8/8] Building zmq_listener...
docker-compose build zmq_listener
if %ERRORLEVEL% neq 0 (
    echo   ERROR: zmq_listener build failed!
    goto :rust_error
)

echo.
echo   All Rust containers built successfully!
echo.

echo [5/5] Starting all services...
echo   Starting infrastructure (DB, Redis)...
docker-compose up -d timescaledb redis
timeout /t 10 /nobreak >nul

echo   Starting VPN...
docker-compose --profile vpn up -d vpn
echo   Waiting for VPN to become healthy...
:vpn_wait
timeout /t 5 /nobreak >nul
docker inspect arbees-vpn --format="{{.State.Health.Status}}" 2>nul | findstr /C:"healthy" >nul
if %ERRORLEVEL% neq 0 (
    echo     VPN not ready yet, waiting...
    goto vpn_wait
)
echo     VPN is healthy!

echo   Starting all services...
docker-compose --profile full --profile vpn up -d
echo.

echo ============================================
echo Waiting for services to become healthy...
echo ============================================
timeout /t 15 /nobreak >nul

echo.
echo Final container status:
docker-compose --profile full --profile vpn ps

echo.
echo ============================================
echo Sequential rebuild complete!
echo ============================================
goto :end

:rust_error
echo.
echo ============================================
echo BUILD FAILED - See error above
echo ============================================
echo.
echo Try running the failed build manually with:
echo   docker-compose build [service_name] --progress=plain
echo.
echo To see detailed output.

:end
endlocal
