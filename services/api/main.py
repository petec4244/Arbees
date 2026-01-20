"""
FastAPI backend for Arbees dashboard.

Features:
- REST API for opportunities, games, trades, monitoring
- WebSocket for real-time updates
- OpenTelemetry instrumentation ready
"""

import asyncio
import logging
from contextlib import asynccontextmanager
from datetime import datetime
from typing import Optional

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

from arbees_shared.db.connection import DatabaseClient, get_pool, close_pool
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.models.game import Sport

logger = logging.getLogger(__name__)

# Global state
db: Optional[DatabaseClient] = None
redis: Optional[RedisBus] = None
websocket_clients: set[WebSocket] = set()


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    global db, redis

    # Startup
    logger.info("Starting Arbees API")

    pool = await get_pool()
    db = DatabaseClient(pool)

    redis = RedisBus()
    await redis.connect()

    # Subscribe to signals for WebSocket broadcast
    await redis.subscribe("signals:new", broadcast_to_websockets)
    await redis.start_listening()

    yield

    # Shutdown
    logger.info("Shutting down Arbees API")
    await redis.disconnect()
    await close_pool()


app = FastAPI(
    title="Arbees API",
    description="Live sports arbitrage trading system",
    version="0.1.0",
    lifespan=lifespan,
)

# CORS
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# =============================================================================
# Root Health Check (for Docker)
# =============================================================================

@app.get("/health")
async def root_health():
    """Simple health check for Docker."""
    return {"status": "ok"}


# =============================================================================
# WebSocket
# =============================================================================

async def broadcast_to_websockets(data: dict) -> None:
    """Broadcast message to all WebSocket clients."""
    if not websocket_clients:
        return

    message = {"type": "signal", "data": data}
    disconnected = set()

    for ws in websocket_clients:
        try:
            await ws.send_json(message)
        except Exception:
            disconnected.add(ws)

    websocket_clients.difference_update(disconnected)


@app.websocket("/ws")
async def websocket_endpoint(websocket: WebSocket):
    """WebSocket endpoint for real-time updates."""
    await websocket.accept()
    websocket_clients.add(websocket)

    try:
        while True:
            # Handle incoming messages (subscriptions, etc.)
            data = await websocket.receive_json()
            msg_type = data.get("type")

            if msg_type == "subscribe":
                # Handle subscription requests
                game_id = data.get("game_id")
                if game_id and redis:
                    # Subscribe to game-specific updates
                    async def game_handler(payload: dict):
                        await websocket.send_json({
                            "type": "game_update",
                            "game_id": game_id,
                            "data": payload,
                        })
                    await redis.subscribe(f"game:{game_id}:state", game_handler)

            elif msg_type == "ping":
                await websocket.send_json({"type": "pong"})

    except WebSocketDisconnect:
        pass
    finally:
        websocket_clients.discard(websocket)


# =============================================================================
# Response Models
# =============================================================================

class OpportunityResponse(BaseModel):
    opportunity_id: str
    opportunity_type: str
    event_id: str
    sport: Optional[str]
    market_title: str
    platform_buy: str
    platform_sell: str
    buy_price: float
    sell_price: float
    edge_pct: float
    implied_profit: float
    liquidity_buy: float
    liquidity_sell: float
    is_risk_free: bool
    status: str
    created_at: datetime


class GameStateResponse(BaseModel):
    game_id: str
    sport: str
    home_team: Optional[str] = None
    away_team: Optional[str] = None
    home_score: int
    away_score: int
    period: int
    time_remaining: str
    status: str
    home_win_prob: Optional[float]


class SignalResponse(BaseModel):
    signal_id: str
    signal_type: str
    game_id: Optional[str]
    sport: Optional[str]
    team: Optional[str]
    direction: str
    model_prob: Optional[float]
    market_prob: Optional[float]
    edge_pct: float
    confidence: Optional[float]
    reason: Optional[str]
    created_at: datetime


class TradeResponse(BaseModel):
    trade_id: str
    game_id: Optional[str]
    sport: Optional[str]
    platform: str
    market_id: str
    side: str
    entry_price: float
    exit_price: Optional[float]
    size: float
    status: str
    outcome: str
    pnl: Optional[float]
    pnl_pct: Optional[float]
    entry_time: datetime
    exit_time: Optional[datetime]
    # Game info
    home_team: Optional[str] = None
    away_team: Optional[str] = None
    signal_type: Optional[str] = None
    edge_at_entry: Optional[float] = None
    model_prob: Optional[float] = None


class PerformanceResponse(BaseModel):
    total_trades: int
    winning_trades: int
    losing_trades: int
    win_rate: float
    total_pnl: float
    avg_pnl: float
    current_bankroll: float
    roi_pct: float


class EquityHistoryPoint(BaseModel):
    time: str
    equity: float
    peak: float
    drawdown_pct: float


class PerformanceBreakdownResponse(BaseModel):
    by_sport: dict
    by_signal_type: dict


class RiskMetricsResponse(BaseModel):
    daily_pnl: float
    daily_limit: float
    daily_limit_remaining: float
    daily_limit_pct: float
    exposure_by_sport: dict
    exposure_by_game: dict
    total_exposure: float
    max_exposure: float
    circuit_breaker_open: bool
    avg_latency_ms: float
    p95_latency_ms: float
    latency_status: str


# =============================================================================
# Opportunities Endpoints
# =============================================================================

@app.get("/api/opportunities", response_model=list[OpportunityResponse])
async def get_opportunities(
    min_edge: float = Query(1.0, ge=0),
    sport: Optional[str] = None,
    platform: Optional[str] = None,
    limit: int = Query(50, le=200),
):
    """Get arbitrage opportunities."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    query = """
        SELECT DISTINCT ON (event_id, platform_buy, platform_sell) *
        FROM arbitrage_opportunities
        WHERE status = 'active'
          AND edge_pct >= $1
          AND time > NOW() - INTERVAL '5 minutes'
    """
    params = [min_edge]

    if sport:
        query += f" AND sport = ${len(params) + 1}"
        params.append(sport)

    query += " ORDER BY event_id, platform_buy, platform_sell, time DESC"
    query += f" LIMIT ${len(params) + 1}"
    params.append(limit)

    rows = await pool.fetch(query, *params)
    return [OpportunityResponse(**dict(row)) for row in rows]


@app.get("/api/opportunities/stats")
async def get_opportunity_stats():
    """Get opportunity statistics."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    row = await pool.fetchrow("""
        SELECT
            COUNT(*) as total_active,
            AVG(edge_pct) as avg_edge,
            MAX(edge_pct) as max_edge,
            SUM(liquidity_buy + liquidity_sell) as total_liquidity
        FROM arbitrage_opportunities
        WHERE status = 'active'
          AND time > NOW() - INTERVAL '5 minutes'
    """)

    return dict(row) if row else {}


# =============================================================================
# Live Games Endpoints
# =============================================================================

@app.get("/api/live-games")
async def get_live_games(
    sport: Optional[str] = None,
    max_age_hours: int = Query(6, ge=1, le=24, description="Max age of games in hours"),
):
    """Get all live games from game_states with team names."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get latest state for each game with team names from games table
    # Filter out:
    # - Games with final/completed/scheduled status
    # - Games with no updates beyond max_age_hours (stale)
    # - Games in 'status_end_period' or 'halftime' with no updates in last 45 minutes (likely finished)
    # Use COALESCE to show game_id if team names are missing
    query = f"""
        SELECT DISTINCT ON (gs.game_id)
            gs.game_id, gs.sport, gs.home_score, gs.away_score, gs.period,
            gs.time_remaining, gs.status, gs.possession, gs.home_win_prob,
            gs.time as last_update,
            COALESCE(NULLIF(g.home_team, ''), 'Home ' || gs.game_id) as home_team,
            COALESCE(NULLIF(g.away_team, ''), 'Away ' || gs.game_id) as away_team,
            g.home_team_abbrev, g.away_team_abbrev
        FROM game_states gs
        LEFT JOIN games g ON gs.game_id = g.game_id
        WHERE gs.status NOT IN ('final', 'completed', 'scheduled', 'status_scheduled')
          AND gs.time > NOW() - INTERVAL '{max_age_hours} hours'
          AND NOT (gs.status IN ('status_end_period', 'halftime') AND gs.time < NOW() - INTERVAL '45 minutes')
    """
    params = []

    if sport:
        query += f" AND gs.sport = ${len(params) + 1}"
        params.append(sport)

    query += " ORDER BY gs.game_id, gs.time DESC"

    rows = await pool.fetch(query, *params)
    return [dict(row) for row in rows]


@app.get("/api/live-games/{game_id}/state", response_model=GameStateResponse)
async def get_game_state(game_id: str):
    """Get current game state with team names."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    row = await pool.fetchrow("""
        SELECT DISTINCT ON (gs.game_id)
            gs.game_id, gs.sport, gs.home_score, gs.away_score, gs.period,
            gs.time_remaining, gs.status, gs.home_win_prob,
            g.home_team, g.away_team
        FROM game_states gs
        LEFT JOIN games g ON gs.game_id = g.game_id
        WHERE gs.game_id = $1
        ORDER BY gs.game_id, gs.time DESC
    """, game_id)

    if not row:
        raise HTTPException(status_code=404, detail="Game not found")

    return GameStateResponse(**dict(row))


@app.get("/api/live-games/{game_id}/plays")
async def get_game_plays(
    game_id: str,
    limit: int = Query(20, le=100),
):
    """Get recent plays for a game."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    plays = await db.get_recent_plays(game_id, limit)
    return plays


@app.get("/api/live-games/{game_id}/prices")
async def get_game_prices(game_id: str):
    """Get market prices for a game."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    rows = await pool.fetch("""
        SELECT DISTINCT ON (platform) *
        FROM market_prices
        WHERE game_id = $1
        ORDER BY platform, time DESC
    """, game_id)

    return [dict(row) for row in rows]


@app.get("/api/live-games/{game_id}/signals", response_model=list[SignalResponse])
async def get_game_signals(
    game_id: str,
    limit: int = Query(20, le=100),
):
    """Get trading signals for a game."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    rows = await pool.fetch("""
        SELECT * FROM trading_signals
        WHERE game_id = $1
        ORDER BY time DESC
        LIMIT $2
    """, game_id, limit)

    return [SignalResponse(**dict(row)) for row in rows]


# =============================================================================
# Paper Trading Endpoints
# =============================================================================

@app.get("/api/paper-trading/status")
async def get_paper_trading_status():
    """Get paper trading status and metrics."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    stats = await db.get_performance_stats(days=30)

    # Get current bankroll
    pool = await get_pool()
    bankroll = await pool.fetchrow("""
        SELECT * FROM bankroll
        WHERE account_name = 'default'
    """)

    # Create default bankroll if not exists
    if not bankroll:
        await pool.execute("""
            INSERT INTO bankroll (account_name, initial_balance, current_balance, peak_balance, trough_balance)
            VALUES ('default', 1000, 1000, 1000, 1000)
            ON CONFLICT (account_name) DO NOTHING
        """)
        bankroll = await pool.fetchrow("""
            SELECT * FROM bankroll WHERE account_name = 'default'
        """)

    # Get open positions summary
    open_positions = await pool.fetch("""
        SELECT
            pt.game_id,
            pt.sport,
            pt.side,
            pt.entry_price,
            pt.size,
            pt.time,
            g.home_team,
            g.away_team
        FROM paper_trades pt
        LEFT JOIN games g ON pt.game_id = g.game_id
        WHERE pt.status = 'open'
        ORDER BY pt.time DESC
    """)

    # Calculate reserved balance (sum of open position costs)
    reserved_balance = 0.0
    for pos in open_positions:
        if pos["side"] == "buy":
            reserved_balance += float(pos["size"]) * float(pos["entry_price"])
        else:
            reserved_balance += float(pos["size"]) * (1.0 - float(pos["entry_price"]))

    bankroll_dict = dict(bankroll) if bankroll else {"initial_balance": 1000, "current_balance": 1000}
    bankroll_dict["reserved_balance"] = reserved_balance
    bankroll_dict["available_balance"] = float(bankroll_dict.get("current_balance", 1000)) - reserved_balance

    return {
        "stats": stats,
        "bankroll": bankroll_dict,
        "open_positions": [dict(row) for row in open_positions],
        "open_positions_count": len(open_positions),
    }


@app.get("/api/paper-trading/trades", response_model=list[TradeResponse])
async def get_paper_trades(
    status: Optional[str] = None,
    limit: int = Query(50, le=200),
):
    """Get paper trade history with game info."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    query = """
        SELECT
            pt.*,
            g.home_team,
            g.away_team
        FROM paper_trades pt
        LEFT JOIN games g ON pt.game_id = g.game_id
    """
    params = []

    if status:
        query += f" WHERE pt.status = ${len(params) + 1}"
        params.append(status)

    query += f" ORDER BY pt.time DESC LIMIT ${len(params) + 1}"
    params.append(limit)

    rows = await pool.fetch(query, *params)
    return [TradeResponse(**dict(row)) for row in rows]


@app.get("/api/paper-trading/performance", response_model=PerformanceResponse)
async def get_paper_trading_performance(
    days: int = Query(30, ge=1, le=365),
):
    """Get paper trading performance metrics."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    stats = await db.get_performance_stats(days=days)

    total = int(stats.get("total_trades") or 0)
    winning = int(stats.get("winning_trades") or 0)
    losing = int(stats.get("losing_trades") or 0)

    pool = await get_pool()
    bankroll = await pool.fetchrow("""
        SELECT * FROM bankroll WHERE account_name = 'default'
    """)

    # Create default bankroll if not exists
    if not bankroll:
        await pool.execute("""
            INSERT INTO bankroll (account_name, initial_balance, current_balance, peak_balance, trough_balance)
            VALUES ('default', 1000, 1000, 1000, 1000)
            ON CONFLICT (account_name) DO NOTHING
        """)
        initial = 1000.0
        current = 1000.0
    else:
        initial = float(bankroll["initial_balance"])
        current = float(bankroll["current_balance"])

    return PerformanceResponse(
        total_trades=total,
        winning_trades=winning,
        losing_trades=losing,
        win_rate=(winning / total * 100) if total > 0 else 0,
        total_pnl=float(stats.get("total_pnl", 0) or 0),
        avg_pnl=float(stats.get("avg_pnl", 0) or 0),
        current_bankroll=current,
        roi_pct=((current - initial) / initial * 100) if initial > 0 else 0,
    )


@app.get("/api/paper-trading/equity-history", response_model=list[EquityHistoryPoint])
async def get_equity_history(
    days: int = Query(30, ge=1, le=365),
):
    """Get equity history for charts with drawdown."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get bankroll for initial balance
    bankroll = await pool.fetchrow("""
        SELECT initial_balance, current_balance, peak_balance
        FROM bankroll WHERE account_name = 'default'
    """)
    initial = float(bankroll["initial_balance"]) if bankroll else 1000.0

    # Get daily P&L aggregated from closed trades
    rows = await pool.fetch("""
        SELECT
            DATE(exit_time) as trade_date,
            SUM(pnl) as daily_pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY DATE(exit_time)
        ORDER BY trade_date ASC
    """ % days)

    # Build equity curve with running total and peak tracking
    equity_history = []
    running_equity = initial
    peak_equity = initial

    for row in rows:
        running_equity += float(row["daily_pnl"] or 0)
        peak_equity = max(peak_equity, running_equity)
        drawdown_pct = ((peak_equity - running_equity) / peak_equity * 100) if peak_equity > 0 else 0

        equity_history.append(EquityHistoryPoint(
            time=row["trade_date"].isoformat(),
            equity=running_equity,
            peak=peak_equity,
            drawdown_pct=drawdown_pct,
        ))

    # If no trades, return current state
    if not equity_history:
        current_equity = float(bankroll["current_balance"]) if bankroll else 1000.0
        peak = float(bankroll["peak_balance"]) if bankroll else 1000.0
        drawdown = ((peak - current_equity) / peak * 100) if peak > 0 else 0
        equity_history.append(EquityHistoryPoint(
            time=datetime.utcnow().date().isoformat(),
            equity=current_equity,
            peak=peak,
            drawdown_pct=drawdown,
        ))

    return equity_history


@app.get("/api/paper-trading/performance/breakdown", response_model=PerformanceBreakdownResponse)
async def get_performance_breakdown(
    days: int = Query(30, ge=1, le=365),
):
    """Get performance breakdown by sport and signal type."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Breakdown by sport
    sport_rows = await pool.fetch("""
        SELECT
            sport,
            COUNT(*) as trades,
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            SUM(COALESCE(pnl, 0)) as pnl,
            AVG(COALESCE(edge_at_entry, 0)) as avg_edge
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY sport
        ORDER BY pnl DESC
    """ % days)

    by_sport = {}
    for row in sport_rows:
        sport = row["sport"] or "unknown"
        trades = int(row["trades"])
        wins = int(row["wins"])
        by_sport[sport] = {
            "trades": trades,
            "wins": wins,
            "pnl": float(row["pnl"] or 0),
            "win_rate": (wins / trades * 100) if trades > 0 else 0,
            "avg_edge": float(row["avg_edge"] or 0),
        }

    # Breakdown by signal type
    signal_rows = await pool.fetch("""
        SELECT
            signal_type,
            COUNT(*) as trades,
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            SUM(COALESCE(pnl, 0)) as pnl,
            AVG(COALESCE(edge_at_entry, 0)) as avg_edge
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY signal_type
        ORDER BY pnl DESC
    """ % days)

    by_signal_type = {}
    for row in signal_rows:
        signal_type = row["signal_type"] or "unknown"
        trades = int(row["trades"])
        wins = int(row["wins"])
        by_signal_type[signal_type] = {
            "trades": trades,
            "wins": wins,
            "pnl": float(row["pnl"] or 0),
            "win_rate": (wins / trades * 100) if trades > 0 else 0,
            "avg_edge": float(row["avg_edge"] or 0),
        }

    return PerformanceBreakdownResponse(
        by_sport=by_sport,
        by_signal_type=by_signal_type,
    )


# =============================================================================
# Risk Endpoints
# =============================================================================

@app.get("/api/risk/metrics", response_model=RiskMetricsResponse)
async def get_risk_metrics():
    """Get current risk metrics for monitoring."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get today's P&L
    today_pnl_row = await pool.fetchrow("""
        SELECT COALESCE(SUM(pnl), 0) as daily_pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND DATE(exit_time) = CURRENT_DATE
    """)
    daily_pnl = float(today_pnl_row["daily_pnl"]) if today_pnl_row else 0.0

    # Risk limits (configurable, using defaults for now)
    daily_limit = 100.0  # $100 daily loss limit
    max_exposure = 200.0  # $200 max exposure per sport

    # Get exposure by sport (open positions)
    sport_exposure_rows = await pool.fetch("""
        SELECT
            sport,
            SUM(CASE WHEN side = 'buy' THEN size * entry_price ELSE size * (1 - entry_price) END) as exposure
        FROM paper_trades
        WHERE status = 'open'
        GROUP BY sport
    """)

    exposure_by_sport = {}
    total_exposure = 0.0
    for row in sport_exposure_rows:
        sport = row["sport"] or "unknown"
        exposure = float(row["exposure"] or 0)
        exposure_by_sport[sport] = {
            "exposure": exposure,
            "limit": max_exposure,
            "pct": (exposure / max_exposure * 100) if max_exposure > 0 else 0,
        }
        total_exposure += exposure

    # Get exposure by game (open positions)
    game_exposure_rows = await pool.fetch("""
        SELECT
            pt.game_id,
            g.home_team,
            g.away_team,
            SUM(CASE WHEN pt.side = 'buy' THEN pt.size * pt.entry_price ELSE pt.size * (1 - pt.entry_price) END) as exposure
        FROM paper_trades pt
        LEFT JOIN games g ON pt.game_id = g.game_id
        WHERE pt.status = 'open'
        GROUP BY pt.game_id, g.home_team, g.away_team
        ORDER BY exposure DESC
        LIMIT 10
    """)

    game_limit = 50.0  # $50 per game limit
    exposure_by_game = {}
    for row in game_exposure_rows:
        game_id = row["game_id"]
        exposure = float(row["exposure"] or 0)
        game_name = f"{row['away_team']} @ {row['home_team']}" if row["home_team"] else game_id
        exposure_by_game[game_id] = {
            "name": game_name,
            "exposure": exposure,
            "limit": game_limit,
            "pct": (exposure / game_limit * 100) if game_limit > 0 else 0,
        }

    # Get latency metrics
    latency_row = await pool.fetchrow("""
        SELECT
            AVG(total_latency_ms) as avg_latency,
            percentile_cont(0.95) WITHIN GROUP (ORDER BY total_latency_ms) as p95_latency
        FROM latency_metrics
        WHERE time > NOW() - INTERVAL '1 hour'
    """)

    avg_latency = float(latency_row["avg_latency"]) if latency_row and latency_row["avg_latency"] else 0.0
    p95_latency = float(latency_row["p95_latency"]) if latency_row and latency_row["p95_latency"] else 0.0

    # Determine latency status
    if avg_latency > 5000:
        latency_status = "critical"
    elif avg_latency > 1000:
        latency_status = "warning"
    else:
        latency_status = "good"

    # Circuit breaker logic
    circuit_breaker_open = daily_pnl <= -daily_limit or avg_latency > 10000

    return RiskMetricsResponse(
        daily_pnl=daily_pnl,
        daily_limit=daily_limit,
        daily_limit_remaining=daily_limit + daily_pnl,  # Since daily_pnl can be negative
        daily_limit_pct=abs(min(daily_pnl, 0) / daily_limit * 100) if daily_limit > 0 else 0,
        exposure_by_sport=exposure_by_sport,
        exposure_by_game=exposure_by_game,
        total_exposure=total_exposure,
        max_exposure=max_exposure * len(exposure_by_sport) if exposure_by_sport else max_exposure,
        circuit_breaker_open=circuit_breaker_open,
        avg_latency_ms=avg_latency,
        p95_latency_ms=p95_latency,
        latency_status=latency_status,
    )


@app.get("/api/risk/events")
async def get_risk_events(
    limit: int = Query(50, le=200),
):
    """Get recent risk events and decisions."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get recent trades with their risk status
    rows = await pool.fetch("""
        SELECT
            time,
            trade_id,
            game_id,
            side,
            size,
            status,
            signal_type,
            edge_at_entry
        FROM paper_trades
        ORDER BY time DESC
        LIMIT $1
    """, limit)

    events = []
    for row in rows:
        # Simulate risk events based on trade data
        event_type = "APPROVED" if row["status"] != "cancelled" else "REJECTED"
        reason = row["signal_type"] or "manual"
        message = f"${row['size']:.2f} {row['side']} trade"
        if row["edge_at_entry"]:
            message += f" (edge: {row['edge_at_entry']:.1f}%)"

        events.append({
            "time": row["time"].isoformat() if row["time"] else None,
            "event_type": event_type,
            "reason": reason,
            "message": message,
            "trade_id": row["trade_id"],
            "game_id": row["game_id"],
        })

    return events


# =============================================================================
# Monitoring Endpoints
# =============================================================================

@app.get("/api/monitoring/health")
async def health_check():
    """Health check endpoint."""
    return {
        "status": "healthy",
        "timestamp": datetime.utcnow().isoformat(),
        "database": db is not None,
        "redis": redis is not None,
        "websocket_clients": len(websocket_clients),
    }


@app.get("/api/monitoring/status")
async def get_system_status():
    """Detailed system status for frontend."""
    # Check DB connection
    db_ok = False
    if db:
        try:
            # Simple query to check connection
            await db.pool.fetchval("SELECT 1")
            db_ok = True
        except Exception:
            pass

    # Check Redis connection
    redis_ok = False
    if redis and redis.redis:
        try:
            await redis.redis.ping()
            redis_ok = True
        except Exception:
            pass

    # Get shard count (mock for now or query redis)
    shards = 0
    if redis_ok:
        # In a real implementation we would count active heartbeats
        # For now return 1 if redis is up
        shards = 1

    return {
        "redis": redis_ok,
        "timescaledb": db_ok,
        "shards": shards,
    }


@app.get("/api/monitoring/signals")
async def get_active_signals(
    min_edge: float = Query(1.0, ge=0),
    limit: int = Query(50, le=200),
):
    """Get active trading signals."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    signals = await db.get_active_signals(min_edge=min_edge, limit=limit)
    return signals


@app.get("/api/monitoring/latency")
async def get_latency_metrics(
    game_id: Optional[str] = None,
    limit: int = Query(100, le=500),
):
    """Get latency metrics."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    query = """
        SELECT
            game_id,
            AVG(espn_latency_ms) as avg_espn_latency,
            AVG(market_latency_ms) as avg_market_latency,
            AVG(total_latency_ms) as avg_total_latency,
            percentile_cont(0.95) WITHIN GROUP (ORDER BY total_latency_ms) as p95_latency,
            COUNT(*) as sample_count
        FROM latency_metrics
        WHERE time > NOW() - INTERVAL '1 hour'
    """
    params = []

    if game_id:
        query += f" AND game_id = ${len(params) + 1}"
        params.append(game_id)

    query += " GROUP BY game_id"

    rows = await pool.fetch(query, *params)
    return [dict(row) for row in rows]


# =============================================================================
# Main
# =============================================================================

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8000)
