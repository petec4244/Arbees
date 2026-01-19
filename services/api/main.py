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
    home_team: str
    away_team: str
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


class PerformanceResponse(BaseModel):
    total_trades: int
    winning_trades: int
    losing_trades: int
    win_rate: float
    total_pnl: float
    avg_pnl: float
    current_bankroll: float
    roi_pct: float


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
):
    """Get all live games."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    games = await db.get_live_games(sport)
    return games


@app.get("/api/live-games/{game_id}/state", response_model=GameStateResponse)
async def get_game_state(game_id: str):
    """Get current game state."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    state = await db.get_latest_game_state(game_id)
    if not state:
        raise HTTPException(status_code=404, detail="Game not found")

    return GameStateResponse(**state)


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

    return {
        "stats": stats,
        "bankroll": dict(bankroll) if bankroll else None,
    }


@app.get("/api/paper-trading/trades", response_model=list[TradeResponse])
async def get_paper_trades(
    status: Optional[str] = None,
    limit: int = Query(50, le=200),
):
    """Get paper trade history."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    query = "SELECT * FROM paper_trades"
    params = []

    if status:
        query += f" WHERE status = ${len(params) + 1}"
        params.append(status)

    query += f" ORDER BY time DESC LIMIT ${len(params) + 1}"
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

    total = stats.get("total_trades", 0)
    winning = stats.get("winning_trades", 0)
    losing = stats.get("losing_trades", 0)

    pool = await get_pool()
    bankroll = await pool.fetchrow("""
        SELECT * FROM bankroll WHERE account_name = 'default'
    """)

    initial = float(bankroll["initial_balance"]) if bankroll else 1000.0
    current = float(bankroll["current_balance"]) if bankroll else 1000.0

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
