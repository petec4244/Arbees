@echo off
REM Sequential build script for development mode (docker-compose.dev.yml)
REM This mounts source code as volumes for instant code changes (no rebuild needed)

echo ========================================
echo Arbees Sequential Build (Development)
echo ========================================
echo.
echo Using: docker-compose.dev.yml with volume mounts
echo Note: First build takes longer, but code changes don't require rebuild!
echo.

REM Enable BuildKit
set DOCKER_BUILDKIT=1
set COMPOSE_DOCKER_CLI_BUILD=1

echo [1/8] Building orchestrator (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build orchestrator
if %errorlevel% neq 0 (
    echo ERROR: orchestrator build failed
    exit /b 1
)
echo ✓ orchestrator complete
echo.

echo [2/8] Building market-discovery-rust (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build market-discovery-rust
if %errorlevel% neq 0 (
    echo ERROR: market-discovery-rust build failed
    exit /b 1
)
echo ✓ market-discovery-rust complete
echo.

echo [3/8] Building game_shard (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build game_shard
if %errorlevel% neq 0 (
    echo ERROR: game_shard build failed
    exit /b 1
)
echo ✓ game_shard complete
echo.

echo [4/8] Building signal_processor (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build signal_processor
if %errorlevel% neq 0 (
    echo ERROR: signal_processor build failed
    exit /b 1
)
echo ✓ signal_processor complete
echo.

echo [5/8] Building execution_service (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build execution_service
if %errorlevel% neq 0 (
    echo ERROR: execution_service build failed
    exit /b 1
)
echo ✓ execution_service complete
echo.

echo [6/8] Building position_tracker (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build position_tracker
if %errorlevel% neq 0 (
    echo ERROR: position_tracker build failed
    exit /b 1
)
echo ✓ position_tracker complete
echo.

echo [7/8] Building notification_service_rust (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build notification_service_rust
if %errorlevel% neq 0 (
    echo ERROR: notification_service_rust build failed
    exit /b 1
)
echo ✓ notification_service_rust complete
echo.

echo [8/8] Building zmq_listener (dev mode)...
docker-compose -f docker-compose.yml -f docker-compose.dev.yml build zmq_listener
if %errorlevel% neq 0 (
    echo ERROR: zmq_listener build failed
    exit /b 1
)
echo ✓ zmq_listener complete
echo.

echo ========================================
echo All services built successfully (DEV MODE)!
echo ========================================
echo.
echo Source code is mounted as volumes - code changes take effect immediately!
echo.
echo Next steps:
echo   docker-compose -f docker-compose.yml -f docker-compose.dev.yml up -d
echo   docker-compose logs -f orchestrator
echo.
