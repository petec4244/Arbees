@echo off
REM Emergency rebuild script for Windows - fixes all three bugs

echo ==========================================
echo EMERGENCY REBUILD - FIXING ALL 3 BUGS
echo ==========================================
echo.
echo Bug #1: Kelly sizing for sell signals
echo Bug #2: Only emit strongest edge per game
echo Bug #3: Hold winning positions to settlement
echo.
echo All code fixes are already applied!
echo Just need to rebuild Docker containers...
echo.

REM Stop all services
echo Stopping all services...
docker-compose down

REM Remove old containers
echo Cleaning old containers...
docker-compose rm -f

REM Rebuild all Rust services with no cache
echo Rebuilding Rust services (this may take 5-10 minutes)...
docker-compose build --no-cache arbees_rust_core game_shard signal_processor position_tracker execution_service orchestrator market-discovery-rust

REM Start everything
echo Starting services...
docker-compose up -d

REM Wait for services to be ready
echo Waiting for services to start...
timeout /t 10 /nobreak > nul

REM Check service status
echo.
echo ==========================================
echo SERVICE STATUS
echo ==========================================
docker-compose ps

echo.
echo ==========================================
echo VALIDATING FIXES
echo ==========================================
echo.
echo Checking recent trades...
timeout /t 5 /nobreak > nul

docker exec arbees-timescaledb psql -U arbees -d arbees -c "SELECT side, COUNT(*) as trades, AVG(size) as avg_size, MIN(size) as min_size, MAX(size) as max_size FROM paper_trades WHERE time > NOW() - INTERVAL '10 minutes' GROUP BY side;"

echo.
echo ==========================================
echo WATCHING LOGS - Press Ctrl+C to exit
echo ==========================================
echo.
echo Looking for:
echo   ✅ SIGNAL: Team X to win/lose (only ONE per game)
echo   ✅ OPEN: ... - $XX.XX (sell trades should NOT be $1)
echo   ✅ holding_for_settlement (don't close winners)
echo.

REM Follow logs from key services
docker-compose logs -f --tail=50 game_shard signal_processor position_tracker
