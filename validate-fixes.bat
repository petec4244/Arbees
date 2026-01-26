@echo off
REM Validate that all three bug fixes are working

echo ==========================================
echo VALIDATING BUG FIXES
echo ==========================================
echo.

echo [1/3] Checking Kelly Sizing (Sell trades should NOT be $1)...
echo.
docker exec arbees-timescaledb psql -U arbees -d arbees -c "SELECT side, COUNT(*) as trades, ROUND(AVG(size)::numeric, 2) as avg_size, ROUND(MIN(size)::numeric, 2) as min_size, ROUND(MAX(size)::numeric, 2) as max_size FROM paper_trades WHERE time > NOW() - INTERVAL '15 minutes' GROUP BY side ORDER BY side;"
echo.

REM Check if sell trades are still $1
docker exec arbees-timescaledb psql -U arbees -d arbees -t -c "SELECT CASE WHEN AVG(size) > 5.0 THEN '✅ FIXED' ELSE '❌ STILL BROKEN' END FROM paper_trades WHERE side = 'sell' AND time > NOW() - INTERVAL '15 minutes';" 2>nul
if errorlevel 1 echo ⚠️  No sell trades yet, waiting for data...

echo.
echo ==========================================
echo.

echo [2/3] Checking Signal Generation (Only ONE team per game)...
echo.
echo Recent signals (should show only ONE team per game_id):
docker logs arbees-game-shard-rust --tail=20 2>nul | findstr /C:"SIGNAL:"
echo.

echo Trades per game (should be low, not 10-30 per game):
docker exec arbees-timescaledb psql -U arbees -d arbees -c "SELECT game_id, COUNT(*) as num_trades FROM paper_trades WHERE time > NOW() - INTERVAL '30 minutes' GROUP BY game_id HAVING COUNT(*) > 5 ORDER BY COUNT(*) DESC LIMIT 5;"
echo.

echo.
echo ==========================================
echo.

echo [3/3] Checking Position Holding (Should see 'holding_for_settlement')...
echo.
docker logs arbees-position-tracker-rust --tail=50 2>nul | findstr /C:"holding_for_settlement" /C:"holding_winner"
if errorlevel 1 (
    echo ⚠️  No holding messages yet - positions may not be at settlement threshold
    echo    This is OK if no positions are near 0.85+ or 0.15- yet
) else (
    echo ✅ Found holding messages!
)

echo.
echo ==========================================
echo.

echo [SUMMARY] Checking overall win rate...
echo.
docker exec arbees-timescaledb psql -U arbees -d arbees -c "SELECT outcome, COUNT(*) as trades, ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) as pct FROM paper_trades WHERE time > NOW() - INTERVAL '30 minutes' AND status = 'closed' GROUP BY outcome ORDER BY outcome;"

echo.
echo ==========================================
echo Expected Results:
echo   ✅ Sell avg_size: $10-40 (NOT $1.00)
echo   ✅ 1-3 trades per game (NOT 10-30)
echo   ✅ Win rate: 60-80%% (NOT 50%%)
echo ==========================================

pause
