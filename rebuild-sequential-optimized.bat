@echo off
REM Sequential build script using optimized Dockerfile (docker-compose.override.yml)
REM This uses cargo-chef for dependency caching (10x faster on subsequent builds)

echo ========================================
echo Arbees Sequential Build (Optimized)
echo ========================================
echo.
echo Using: Dockerfile.rust-optimized with dependency caching
echo Speed: First build ~5-10 min, subsequent ~30-60 sec per service
echo.

REM Enable BuildKit for better caching
set DOCKER_BUILDKIT=1
set COMPOSE_DOCKER_CLI_BUILD=1

echo [1/8] Building orchestrator...
docker-compose build orchestrator
if %errorlevel% neq 0 (
    echo ERROR: orchestrator build failed
    exit /b 1
)
echo ✓ orchestrator complete
echo.

echo [2/8] Building market-discovery-rust...
docker-compose build market-discovery-rust
if %errorlevel% neq 0 (
    echo ERROR: market-discovery-rust build failed
    exit /b 1
)
echo ✓ market-discovery-rust complete
echo.

echo [3/8] Building game_shard...
docker-compose build game_shard
if %errorlevel% neq 0 (
    echo ERROR: game_shard build failed
    exit /b 1
)
echo ✓ game_shard complete
echo.

echo [4/8] Building signal_processor...
docker-compose build signal_processor
if %errorlevel% neq 0 (
    echo ERROR: signal_processor build failed
    exit /b 1
)
echo ✓ signal_processor complete
echo.

echo [5/8] Building execution_service...
docker-compose build execution_service
if %errorlevel% neq 0 (
    echo ERROR: execution_service build failed
    exit /b 1
)
echo ✓ execution_service complete
echo.

echo [6/8] Building position_tracker...
docker-compose build position_tracker
if %errorlevel% neq 0 (
    echo ERROR: position_tracker build failed
    exit /b 1
)
echo ✓ position_tracker complete
echo.

echo [7/8] Building notification_service_rust...
docker-compose build notification_service_rust
if %errorlevel% neq 0 (
    echo ERROR: notification_service_rust build failed
    exit /b 1
)
echo ✓ notification_service_rust complete
echo.

echo [8/8] Building zmq_listener...
docker-compose build zmq_listener
if %errorlevel% neq 0 (
    echo ERROR: zmq_listener build failed
    exit /b 1
)
echo ✓ zmq_listener complete
echo.

echo ========================================
echo All services built successfully!
echo ========================================
echo.
echo Next steps:
echo   docker-compose up -d         Start all services
echo   docker-compose ps            Check service status
echo.
