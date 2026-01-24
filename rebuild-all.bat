@echo off
setlocal

echo ============================================
echo Arbees - Full Container Rebuild Script
echo ============================================
echo.

cd /d "%~dp0"

echo [1/4] Stopping all containers...
docker-compose --profile full --profile vpn down
if %ERRORLEVEL% neq 0 (
    echo Warning: docker-compose down had issues, trying docker compose...
    docker compose --profile full --profile vpn down
)
echo.

echo [2/4] Rebuilding all containers (no cache)...
docker-compose --profile full --profile vpn build --no-cache
if %ERRORLEVEL% neq 0 (
    echo Warning: docker-compose build had issues, trying docker compose...
    docker compose --profile full --profile vpn build --no-cache
)
echo.

echo [3/4] Starting infrastructure (DB, Redis, VPN)...
docker-compose up -d timescaledb redis
timeout /t 10 /nobreak >nul
docker-compose --profile vpn up -d vpn
echo Waiting for VPN to become healthy...
:vpn_wait
timeout /t 5 /nobreak >nul
docker inspect arbees-vpn --format="{{.State.Health.Status}}" 2>nul | findstr /C:"healthy" >nul
if %ERRORLEVEL% neq 0 (
    echo   VPN not ready yet, waiting...
    goto vpn_wait
)
echo   VPN is healthy!
echo.

echo [4/4] Starting all services...
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
echo Rebuild complete!
echo ============================================

endlocal
