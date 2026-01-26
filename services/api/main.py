"""
FastAPI backend for Arbees dashboard.

Features:
- REST API for opportunities, games, trades, monitoring
- WebSocket for real-time updates
- OpenTelemetry instrumentation ready
"""

import asyncio
import logging
import os
from contextlib import asynccontextmanager
from datetime import datetime
from typing import Optional

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

from arbees_shared.db.connection import DatabaseClient, get_pool, close_pool
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.models.game import Sport
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus

logger = logging.getLogger(__name__)

# Global state
db: Optional[DatabaseClient] = None
redis: Optional[RedisBus] = None
websocket_clients: set[WebSocket] = set()
heartbeat_publisher: Optional[HeartbeatPublisher] = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Application lifespan manager."""
    global db, redis, heartbeat_publisher

    # Startup
    logger.info("Starting Arbees API")

    pool = await get_pool()
    db = DatabaseClient(pool)

    redis = RedisBus()
    await redis.connect()

    # Subscribe to signals for WebSocket broadcast
    await redis.subscribe("signals:new", broadcast_to_websockets)

    # Subscribe to futures signals for real-time updates
    await redis.subscribe("futures:signals:new", broadcast_futures_signal)

    await redis.start_listening()

    # Start heartbeat publisher
    heartbeat_publisher = HeartbeatPublisher(
        service="api",
        instance_id=os.environ.get("HOSTNAME", "api-1"),
    )
    await heartbeat_publisher.start()
    heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
    heartbeat_publisher.update_checks({
        "redis_ok": True,
        "db_ok": True,
    })

    yield

    # Shutdown
    logger.info("Shutting down Arbees API")
    if heartbeat_publisher:
        await heartbeat_publisher.stop()
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


async def broadcast_futures_signal(data: dict) -> None:
    """Broadcast futures signal to all WebSocket clients."""
    if not websocket_clients:
        return

    message = {"type": "futures_signal", "data": data}
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
    cooldown_until: Optional[datetime] = None


class GameHistoryPoint(BaseModel):
    time: datetime
    home_win_prob: float



class UpcomingGameResponse(BaseModel):
    """Response model for upcoming games."""
    game_id: str
    sport: str
    home_team: str
    away_team: str
    home_team_abbrev: Optional[str] = None
    away_team_abbrev: Optional[str] = None
    scheduled_time: datetime
    venue: Optional[str] = None
    broadcast: Optional[str] = None
    time_category: str  # "imminent" | "soon" | "upcoming" | "future"
    time_until_start: str  # "15 min" | "2h 15m" | "Today at 7:00 PM" | "Tomorrow at 1:00 PM"
    minutes_until_start: int


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
    piggybank_balance: float = 0.0


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
    include_final: bool = False,
):
    """Get all live games from game_states with team names."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Filters logic
    # $1 = sport (if present)
    # $2 = include_final (bool)
    
    # Base query logic
    base_filter = f"""
        WHERE l.last_update > NOW() - INTERVAL '{max_age_hours} hours'
          -- Any game that hasn't updated recently is not "live" for the UI.
          -- If include_final is True, we allow older updates (up to max_age_hours)
          AND (
             $2 = true OR (
                lower(COALESCE(l.status, '')) IN ('halftime', 'status_end_period', 'end_period')
                OR l.last_update >= NOW() - INTERVAL '15 minutes'
             )
          )
    """

    if not include_final:
         base_filter += """
          AND NOT (
            lower(COALESCE(l.status, '')) IN ('final', 'completed', 'scheduled', 'status_scheduled')
            OR lower(COALESCE(l.status, '')) LIKE '%final%'
            OR lower(COALESCE(l.status, '')) LIKE '%complete%'
          )
          AND NOT (l.status IN ('status_end_period', 'halftime') AND l.last_update < NOW() - INTERVAL '45 minutes')
          AND NOT (
            lower(COALESCE(l.status, '')) LIKE '%end_period%'
            AND (l.time_remaining LIKE '0:%' OR l.time_remaining IN ('0', '0.0', '0:00'))
            AND l.last_update < NOW() - INTERVAL '10 minutes'
          )
        """
    else:
        # If including final games, just filter out scheduled
        base_filter += """
          AND NOT (
            lower(COALESCE(l.status, '')) IN ('scheduled', 'status_scheduled')
          )
        """

    query = f"""
        WITH latest AS (
            SELECT DISTINCT ON (gs.game_id)
                gs.game_id, gs.sport, gs.home_score, gs.away_score, gs.period,
                gs.time_remaining, gs.status, gs.possession, gs.home_win_prob,
                gs.time as last_update
            FROM game_states gs
            ORDER BY gs.game_id, gs.time DESC
        )
        SELECT
            l.game_id, l.sport, l.home_score, l.away_score, l.period,
            l.time_remaining, l.status, l.possession, l.home_win_prob,
            l.last_update, g.cooldown_until,
            COALESCE(NULLIF(g.home_team, ''), 'Home ' || l.game_id) as home_team,
            COALESCE(NULLIF(g.away_team, ''), 'Away ' || l.game_id) as away_team,
            g.home_team_abbrev, g.away_team_abbrev
        FROM latest l
        LEFT JOIN games g ON l.game_id = g.game_id
        {base_filter}
    """
    
    params = []
    
    # Param 1: Sport (optional)
    if sport:
        query += f" AND l.sport = $1"
        params.append(sport)
    
    # Param 2: include_final logic in base_filter uses $2 if sport is present? 
    # Wait, simple numbering is best.
    # Actually, asyncpg uses $1, $2, etc. based on the order in the list passed to fetch.
    
    # Let's rebuild params carefully.
    
    query_params = []
    
    # Re-construct query with correct placeholders
    query = f"""
        WITH latest AS (
            SELECT DISTINCT ON (gs.game_id)
                gs.game_id, gs.sport, gs.home_score, gs.away_score, gs.period,
                gs.time_remaining, gs.status, gs.possession, gs.home_win_prob,
                gs.time as last_update
            FROM game_states gs
            ORDER BY gs.game_id, gs.time DESC
        )
        SELECT
            l.game_id, l.sport, l.home_score, l.away_score, l.period,
            l.time_remaining, l.status, l.possession, l.home_win_prob,
            l.last_update, g.cooldown_until,
            COALESCE(NULLIF(g.home_team, ''), 'Home ' || l.game_id) as home_team,
            COALESCE(NULLIF(g.away_team, ''), 'Away ' || l.game_id) as away_team,
            g.home_team_abbrev, g.away_team_abbrev
        FROM latest l
        LEFT JOIN games g ON l.game_id = g.game_id
        WHERE l.last_update > NOW() - INTERVAL '{max_age_hours} hours'
    """
    
    # Include Final Check (always pass as param)
    query += " AND ($1 = true OR (" # $1 is include_final
    query += "   lower(COALESCE(l.status, '')) IN ('halftime', 'status_end_period', 'end_period')"
    query += "   OR l.last_update >= NOW() - INTERVAL '15 minutes'"
    query += " ))"
    query_params.append(include_final)
    
    if not include_final:
        query += """
          AND NOT (
            lower(COALESCE(l.status, '')) IN ('final', 'completed', 'scheduled', 'status_scheduled')
            OR lower(COALESCE(l.status, '')) LIKE '%final%'
            OR lower(COALESCE(l.status, '')) LIKE '%complete%'
          )
          AND NOT (l.status IN ('status_end_period', 'halftime') AND l.last_update < NOW() - INTERVAL '45 minutes')
          AND NOT (
            lower(COALESCE(l.status, '')) LIKE '%end_period%'
            AND (l.time_remaining LIKE '0:%' OR l.time_remaining IN ('0', '0.0', '0:00'))
            AND l.last_update < NOW() - INTERVAL '10 minutes'
          )
        """
    else:
        query += """
          AND NOT (
            lower(COALESCE(l.status, '')) IN ('scheduled', 'status_scheduled')
          )
        """

    if sport:
        query += f" AND l.sport = ${len(query_params) + 1}"
        query_params.append(sport)

    query += " ORDER BY l.last_update DESC"

    rows = await pool.fetch(query, *query_params)
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


@app.get("/api/live-games/{game_id}/history", response_model=list[GameHistoryPoint])
async def get_game_history(
    game_id: str,
    limit: int = Query(100, le=500),
):
    """Get historical win probabilities for a game chart."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()
    # Get history, filtering out null probabilities
    rows = await pool.fetch("""
        SELECT time, home_win_prob
        FROM game_states
        WHERE game_id = $1
          AND home_win_prob IS NOT NULL
        ORDER BY time ASC
    """, game_id)

    # Simple downsampling if too many points
    if len(rows) > limit:
        step = len(rows) // limit
        rows = rows[::step]

    return [GameHistoryPoint(time=row["time"], home_win_prob=row["home_win_prob"]) for row in rows]


# =============================================================================
# Upcoming Games Endpoints
# =============================================================================

def _categorize_time(minutes_until_start: int) -> str:
    """Categorize time until game start.

    Categories:
    - imminent: < 30 minutes
    - soon: 30 min - 2 hours
    - upcoming: 2 - 24 hours
    - future: > 24 hours
    """
    if minutes_until_start < 30:
        return "imminent"
    elif minutes_until_start < 120:  # 2 hours
        return "soon"
    elif minutes_until_start < 1440:  # 24 hours
        return "upcoming"
    else:
        return "future"


def _format_time_until(scheduled_time: datetime, minutes_until: int) -> str:
    """Format time until game start as human-readable string."""
    if minutes_until < 0:
        return "Started"

    if minutes_until < 60:
        return f"{minutes_until} min"
    elif minutes_until < 120:
        hours = minutes_until // 60
        mins = minutes_until % 60
        return f"{hours}h {mins}m" if mins > 0 else f"{hours} hour"
    elif minutes_until < 1440:  # Same day
        hours = minutes_until // 60
        mins = minutes_until % 60
        if mins > 0:
            return f"{hours}h {mins}m"
        return f"{hours} hours"
    elif minutes_until < 2880:  # Tomorrow
        # Format as "Tomorrow at HH:MM AM/PM"
        return f"Tomorrow at {scheduled_time.strftime('%I:%M %p')}"
    else:
        # Format as "Day, Mon DD at HH:MM AM/PM"
        return scheduled_time.strftime("%a, %b %d at %I:%M %p")


@app.get("/api/upcoming-games", response_model=list[UpcomingGameResponse])
async def get_upcoming_games(
    sport: Optional[str] = Query(None, description="Filter by sport (nfl, nba, nhl, etc.)"),
    hours_ahead: int = Query(24, ge=1, le=168, description="Hours ahead to look (1-168)"),
    limit: int = Query(50, le=200, description="Maximum number of games to return"),
):
    """Get upcoming scheduled games with time categorization.

    Returns games that are scheduled but haven't started yet, sorted by start time.
    Each game includes:
    - time_category: "imminent" (<30min), "soon" (30min-2h), "upcoming" (2-24h), "future" (>24h)
    - time_until_start: Human-readable time until game starts
    - minutes_until_start: Minutes until game starts (for sorting/filtering)
    """
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Query scheduled games from the games table
    # Include games where status is 'scheduled' or NULL and scheduled_time is in the future
    query = """
        SELECT
            g.game_id,
            g.sport,
            COALESCE(g.home_team, 'TBD') as home_team,
            COALESCE(g.away_team, 'TBD') as away_team,
            g.home_team_abbrev,
            g.away_team_abbrev,
            g.scheduled_time,
            g.venue,
            g.broadcast,
            EXTRACT(EPOCH FROM (g.scheduled_time - NOW())) / 60 AS minutes_until_start
        FROM games g
        WHERE (g.status IS NULL OR g.status IN ('scheduled', 'status_scheduled', 'pregame'))
          AND g.scheduled_time > NOW()
          AND g.scheduled_time < NOW() + INTERVAL '%s hours'
    """ % hours_ahead

    params = []

    if sport:
        query += f" AND LOWER(g.sport) = ${len(params) + 1}"
        params.append(sport.lower())

    query += f"""
        ORDER BY g.scheduled_time ASC
        LIMIT ${len(params) + 1}
    """
    params.append(limit)

    rows = await pool.fetch(query, *params)

    results = []
    for row in rows:
        minutes_until = int(row["minutes_until_start"] or 0)
        scheduled_time = row["scheduled_time"]

        # Skip if game already started (shouldn't happen but safety check)
        if minutes_until < -5:
            continue

        time_category = _categorize_time(minutes_until)
        time_until_start = _format_time_until(scheduled_time, minutes_until)

        results.append(UpcomingGameResponse(
            game_id=row["game_id"],
            sport=row["sport"],
            home_team=row["home_team"],
            away_team=row["away_team"],
            home_team_abbrev=row["home_team_abbrev"],
            away_team_abbrev=row["away_team_abbrev"],
            scheduled_time=scheduled_time,
            venue=row["venue"],
            broadcast=row["broadcast"],
            time_category=time_category,
            time_until_start=time_until_start,
            minutes_until_start=max(0, minutes_until),
        ))

    return results


@app.get("/api/upcoming-games/stats")
async def get_upcoming_games_stats(
    hours_ahead: int = Query(24, ge=1, le=168),
):
    """Get statistics about upcoming games."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    row = await pool.fetchrow("""
        SELECT
            COUNT(*) as total_games,
            COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 < 30) as imminent,
            COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 >= 30
                                AND EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 < 120) as soon,
            COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 >= 120
                                AND EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 < 1440) as upcoming,
            COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (scheduled_time - NOW())) / 60 >= 1440) as future
        FROM games
        WHERE (status IS NULL OR status IN ('scheduled', 'status_scheduled', 'pregame'))
          AND scheduled_time > NOW()
          AND scheduled_time < NOW() + INTERVAL '%s hours'
    """ % hours_ahead)

    # Count by sport
    sport_rows = await pool.fetch("""
        SELECT
            sport,
            COUNT(*) as count
        FROM games
        WHERE (status IS NULL OR status IN ('scheduled', 'status_scheduled', 'pregame'))
          AND scheduled_time > NOW()
          AND scheduled_time < NOW() + INTERVAL '%s hours'
        GROUP BY sport
        ORDER BY count DESC
    """ % hours_ahead)

    by_sport = {r["sport"]: r["count"] for r in sport_rows}

    return {
        "total_games": row["total_games"] if row else 0,
        "by_category": {
            "imminent": row["imminent"] if row else 0,
            "soon": row["soon"] if row else 0,
            "upcoming": row["upcoming"] if row else 0,
            "future": row["future"] if row else 0,
        },
        "by_sport": by_sport,
    }


# =============================================================================
# Futures Monitoring Endpoints
# =============================================================================

class FuturesGameResponse(BaseModel):
    """Response model for futures games being monitored."""
    game_id: str
    sport: str
    home_team: str
    away_team: str
    scheduled_time: datetime
    hours_until_start: float
    has_kalshi: bool
    has_polymarket: bool
    opening_home_prob: Optional[float]
    current_home_prob: Optional[float]
    line_movement_pct: Optional[float]
    movement_direction: Optional[str]
    total_volume: float
    active_signals: int
    lifecycle_status: str


class FuturesPriceHistoryResponse(BaseModel):
    """Response model for futures price history."""
    time: datetime
    platform: str
    market_type: str
    yes_mid: Optional[float]
    spread_cents: Optional[float]
    volume: Optional[float]
    hours_until_start: Optional[float]


class FuturesSignalResponse(BaseModel):
    """Response model for futures signals."""
    signal_id: str
    game_id: str
    sport: str
    signal_type: str
    direction: str
    edge_pct: float
    confidence: Optional[float]
    hours_until_start: Optional[float]
    reason: Optional[str]
    executed: bool
    time: datetime


class FuturesStatsResponse(BaseModel):
    """Statistics for futures monitoring."""
    total_monitored: int
    games_with_markets: int
    active_signals: int
    avg_line_movement: float
    by_sport: dict


@app.get("/api/futures/games", response_model=list[FuturesGameResponse])
async def get_futures_games(
    sport: Optional[str] = Query(None, description="Filter by sport"),
    min_hours: float = Query(0, ge=0, description="Minimum hours until start"),
    max_hours: float = Query(48, ge=0, le=168, description="Maximum hours until start"),
    limit: int = Query(50, ge=1, le=200, description="Maximum games to return"),
):
    """Get games currently being monitored by FuturesMonitor.

    Returns games with pre-game market tracking data including:
    - Opening and current probabilities
    - Line movement tracking
    - Market discovery status
    - Active futures signals count
    """
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    rows = await pool.fetch("""
        SELECT
            fg.game_id,
            fg.sport,
            fg.home_team,
            fg.away_team,
            fg.scheduled_time,
            EXTRACT(EPOCH FROM (fg.scheduled_time - NOW())) / 3600 as hours_until_start,
            fg.kalshi_market_id IS NOT NULL as has_kalshi,
            fg.polymarket_market_id IS NOT NULL as has_polymarket,
            fg.opening_home_prob,
            fg.current_home_prob,
            fg.line_movement_pct,
            fg.movement_direction,
            COALESCE(fg.total_volume_kalshi, 0) + COALESCE(fg.total_volume_polymarket, 0) as total_volume,
            fg.lifecycle_status,
            (
                SELECT COUNT(*) FROM futures_signals fs
                WHERE fs.game_id = fg.game_id AND NOT fs.executed
            ) as active_signals
        FROM futures_games fg
        WHERE fg.lifecycle_status = 'futures_monitoring'
          AND fg.scheduled_time > NOW() + INTERVAL '%s hours'
          AND fg.scheduled_time < NOW() + INTERVAL '%s hours'
    """ % (min_hours, max_hours) + (
        " AND LOWER(fg.sport) = $1" if sport else ""
    ) + """
        ORDER BY fg.scheduled_time ASC
        LIMIT %s
    """ % limit, *([sport.lower()] if sport else []))

    return [
        FuturesGameResponse(
            game_id=row["game_id"],
            sport=row["sport"],
            home_team=row["home_team"],
            away_team=row["away_team"],
            scheduled_time=row["scheduled_time"],
            hours_until_start=float(row["hours_until_start"] or 0),
            has_kalshi=row["has_kalshi"],
            has_polymarket=row["has_polymarket"],
            opening_home_prob=float(row["opening_home_prob"]) if row["opening_home_prob"] else None,
            current_home_prob=float(row["current_home_prob"]) if row["current_home_prob"] else None,
            line_movement_pct=float(row["line_movement_pct"]) if row["line_movement_pct"] else None,
            movement_direction=row["movement_direction"],
            total_volume=float(row["total_volume"] or 0),
            active_signals=int(row["active_signals"] or 0),
            lifecycle_status=row["lifecycle_status"],
        )
        for row in rows
    ]


@app.get("/api/futures/games/{game_id}/prices", response_model=list[FuturesPriceHistoryResponse])
async def get_futures_price_history(
    game_id: str,
    platform: Optional[str] = Query(None, description="Filter by platform (kalshi, polymarket)"),
    limit: int = Query(200, ge=1, le=1000, description="Maximum price points to return"),
):
    """Get price history for a futures game.

    Returns chronological price snapshots from both platforms,
    useful for charting line movement over time.
    """
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    query = """
        SELECT time, platform, market_type, yes_mid, spread_cents, volume, hours_until_start
        FROM futures_price_history
        WHERE game_id = $1
    """
    params = [game_id]

    if platform:
        query += " AND platform = $2"
        params.append(platform.lower())

    query += " ORDER BY time DESC LIMIT %s" % limit

    rows = await pool.fetch(query, *params)

    return [
        FuturesPriceHistoryResponse(
            time=row["time"],
            platform=row["platform"],
            market_type=row["market_type"],
            yes_mid=float(row["yes_mid"]) if row["yes_mid"] else None,
            spread_cents=float(row["spread_cents"]) if row["spread_cents"] else None,
            volume=float(row["volume"]) if row["volume"] else None,
            hours_until_start=float(row["hours_until_start"]) if row["hours_until_start"] else None,
        )
        for row in rows
    ]


@app.get("/api/futures/signals", response_model=list[FuturesSignalResponse])
async def get_futures_signals(
    sport: Optional[str] = Query(None, description="Filter by sport"),
    signal_type: Optional[str] = Query(None, description="Filter by signal type"),
    min_edge: float = Query(0, ge=0, description="Minimum edge percentage"),
    executed: Optional[bool] = Query(None, description="Filter by execution status"),
    limit: int = Query(50, ge=1, le=200, description="Maximum signals to return"),
):
    """Get active futures trading signals.

    Returns pre-game signals with edge calculations based on:
    - Cross-platform price discrepancies
    - Line movement from opening
    """
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    query = """
        SELECT
            fs.signal_id, fs.game_id, fs.sport, fs.signal_type,
            fs.direction, fs.edge_pct, fs.confidence,
            fs.hours_until_start, fs.reason, fs.executed, fs.time
        FROM futures_signals fs
        WHERE fs.edge_pct >= $1
          AND fs.time > NOW() - INTERVAL '24 hours'
    """
    params = [min_edge]
    param_idx = 2

    if sport:
        query += f" AND LOWER(fs.sport) = ${param_idx}"
        params.append(sport.lower())
        param_idx += 1

    if signal_type:
        query += f" AND fs.signal_type = ${param_idx}"
        params.append(signal_type)
        param_idx += 1

    if executed is not None:
        query += f" AND fs.executed = ${param_idx}"
        params.append(executed)
        param_idx += 1

    query += f" ORDER BY fs.edge_pct DESC, fs.time DESC LIMIT ${param_idx}"
    params.append(limit)

    rows = await pool.fetch(query, *params)

    return [
        FuturesSignalResponse(
            signal_id=row["signal_id"],
            game_id=row["game_id"],
            sport=row["sport"],
            signal_type=row["signal_type"],
            direction=row["direction"],
            edge_pct=float(row["edge_pct"]),
            confidence=float(row["confidence"]) if row["confidence"] else None,
            hours_until_start=float(row["hours_until_start"]) if row["hours_until_start"] else None,
            reason=row["reason"],
            executed=row["executed"],
            time=row["time"],
        )
        for row in rows
    ]


@app.get("/api/futures/stats", response_model=FuturesStatsResponse)
async def get_futures_stats():
    """Get statistics for futures monitoring.

    Returns aggregate metrics about games being monitored,
    market discovery rates, and signal generation.
    """
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get overall stats
    row = await pool.fetchrow("""
        SELECT
            COUNT(*) as total_monitored,
            COUNT(*) FILTER (WHERE kalshi_market_id IS NOT NULL OR polymarket_market_id IS NOT NULL) as games_with_markets,
            AVG(ABS(line_movement_pct)) as avg_line_movement
        FROM futures_games
        WHERE lifecycle_status = 'futures_monitoring'
    """)

    # Count active signals
    signals_row = await pool.fetchrow("""
        SELECT COUNT(*) as active_signals
        FROM futures_signals
        WHERE NOT executed
          AND time > NOW() - INTERVAL '24 hours'
    """)

    # Breakdown by sport
    sport_rows = await pool.fetch("""
        SELECT sport, COUNT(*) as count
        FROM futures_games
        WHERE lifecycle_status = 'futures_monitoring'
        GROUP BY sport
        ORDER BY count DESC
    """)

    by_sport = {r["sport"]: r["count"] for r in sport_rows}

    return FuturesStatsResponse(
        total_monitored=int(row["total_monitored"] or 0) if row else 0,
        games_with_markets=int(row["games_with_markets"] or 0) if row else 0,
        active_signals=int(signals_row["active_signals"] or 0) if signals_row else 0,
        avg_line_movement=float(row["avg_line_movement"] or 0) if row else 0,
        by_sport=by_sport,
    )


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
            pt.edge_at_entry,
            g.home_team,
            g.away_team,
            gs.home_win_prob as current_prob
        FROM paper_trades pt
        LEFT JOIN games g ON pt.game_id = g.game_id
        LEFT JOIN LATERAL (
            SELECT home_win_prob
            FROM game_states
            WHERE game_id = pt.game_id
            ORDER BY time DESC
            LIMIT 1
        ) gs ON true
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

    bankroll_dict = dict(bankroll) if bankroll else {"initial_balance": 1000, "current_balance": 1000, "piggybank_balance": 0}
    bankroll_dict["reserved_balance"] = reserved_balance
    bankroll_dict["available_balance"] = float(bankroll_dict.get("current_balance", 1000)) - reserved_balance
    # Ensure piggybank_balance is included
    if "piggybank_balance" not in bankroll_dict:
        bankroll_dict["piggybank_balance"] = 0.0
    # Calculate total balance (trading + piggybank)
    bankroll_dict["total_balance"] = float(bankroll_dict.get("current_balance", 0)) + float(bankroll_dict.get("piggybank_balance", 0))

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

    # Get bankroll info (piggybank)
    bankroll_row = await pool.fetchrow("SELECT piggybank_balance FROM bankroll WHERE account_name = 'default'")
    piggybank_balance = float(bankroll_row["piggybank_balance"]) if bankroll_row and bankroll_row["piggybank_balance"] else 0.0

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
        piggybank_balance=piggybank_balance,
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
    if db and db._pool:
        try:
            # Simple query to check connection
            await db._pool.fetchval("SELECT 1")
            db_ok = True
        except Exception:
            pass

    # Check Redis connection
    redis_ok = False
    if redis and redis._client:
        try:
            await redis._client.ping()
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
# Historical Games Endpoints
# =============================================================================

class ArchivedGameResponse(BaseModel):
    """Response model for archived games."""
    archive_id: int
    game_id: str
    sport: str
    home_team: str
    away_team: str
    final_home_score: int
    final_away_score: int
    ended_at: datetime
    archived_at: datetime
    total_trades: int
    winning_trades: int
    losing_trades: int
    total_pnl: float
    win_rate: float
    capture_rate: float


class ArchivedTradeResponse(BaseModel):
    """Response model for archived trades."""
    trade_id: str
    signal_type: Optional[str]
    platform: str
    market_type: Optional[str]
    side: str
    entry_price: float
    exit_price: Optional[float]
    size: float
    opened_at: datetime
    closed_at: Optional[datetime]
    outcome: Optional[str]
    pnl: Optional[float]
    edge_at_entry: Optional[float]


class ArchivedSignalResponse(BaseModel):
    """Response model for archived signals."""
    signal_id: str
    signal_type: str
    direction: str
    team: Optional[str]
    model_prob: Optional[float]
    market_prob: Optional[float]
    edge_pct: float
    generated_at: datetime
    was_executed: bool


class ArchivedGameDetailResponse(ArchivedGameResponse):
    """Detailed response including trades and signals."""
    trades: list[ArchivedTradeResponse]
    signals: list[ArchivedSignalResponse]


class HistoricalSummaryResponse(BaseModel):
    """Summary statistics for historical games."""
    total_games: int
    total_trades: int
    total_pnl: float
    overall_win_rate: float
    total_wins: int
    total_losses: int


class HistoricalGamesListResponse(BaseModel):
    """Paginated list of historical games."""
    games: list[ArchivedGameResponse]
    total: int
    page: int
    page_size: int


@app.get("/api/historical/games", response_model=HistoricalGamesListResponse)
async def get_historical_games(
    sport: Optional[str] = Query(None, description="Filter by sport"),
    from_date: Optional[str] = Query(None, description="Start date (YYYY-MM-DD)"),
    to_date: Optional[str] = Query(None, description="End date (YYYY-MM-DD)"),
    outcome: Optional[str] = Query(None, description="Filter: profitable, loss, breakeven"),
    sort_by: str = Query("ended_at", description="Sort by: ended_at, total_pnl, win_rate"),
    sort_order: str = Query("desc", description="Sort order: asc, desc"),
    page: int = Query(1, ge=1, description="Page number"),
    page_size: int = Query(20, ge=1, le=100, description="Items per page"),
):
    """Get archived games with filters and pagination."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Build query
    base_query = """
        SELECT
            archive_id, game_id, sport, home_team, away_team,
            final_home_score, final_away_score, ended_at, archived_at,
            total_trades, winning_trades, losing_trades, total_pnl,
            total_signals_generated, total_signals_executed,
            CASE WHEN total_trades > 0 THEN winning_trades::float / total_trades ELSE 0 END as win_rate,
            CASE WHEN total_signals_generated > 0 THEN total_signals_executed::float / total_signals_generated ELSE 0 END as capture_rate
        FROM archived_games
        WHERE 1=1
    """
    params = []
    param_idx = 1

    if sport:
        base_query += f" AND LOWER(sport) = ${param_idx}"
        params.append(sport.lower())
        param_idx += 1

    if from_date:
        base_query += f" AND ended_at >= ${param_idx}::date"
        params.append(from_date)
        param_idx += 1

    if to_date:
        base_query += f" AND ended_at <= ${param_idx}::date + INTERVAL '1 day'"
        params.append(to_date)
        param_idx += 1

    if outcome == "profitable":
        base_query += " AND total_pnl > 0"
    elif outcome == "loss":
        base_query += " AND total_pnl < 0"
    elif outcome == "breakeven":
        base_query += " AND total_pnl = 0"

    # Count total
    count_query = f"SELECT COUNT(*) FROM ({base_query}) sq"
    total = await pool.fetchval(count_query, *params)

    # Validate sort column
    valid_sort_columns = {"ended_at", "total_pnl", "win_rate", "total_trades"}
    if sort_by not in valid_sort_columns:
        sort_by = "ended_at"
    sort_dir = "DESC" if sort_order.lower() == "desc" else "ASC"

    # Add sorting and pagination
    base_query += f" ORDER BY {sort_by} {sort_dir}"
    base_query += f" LIMIT ${param_idx} OFFSET ${param_idx + 1}"
    params.extend([page_size, (page - 1) * page_size])

    rows = await pool.fetch(base_query, *params)

    games = [
        ArchivedGameResponse(
            archive_id=row["archive_id"],
            game_id=row["game_id"],
            sport=row["sport"],
            home_team=row["home_team"],
            away_team=row["away_team"],
            final_home_score=row["final_home_score"],
            final_away_score=row["final_away_score"],
            ended_at=row["ended_at"],
            archived_at=row["archived_at"],
            total_trades=row["total_trades"],
            winning_trades=row["winning_trades"],
            losing_trades=row["losing_trades"],
            total_pnl=float(row["total_pnl"] or 0),
            win_rate=float(row["win_rate"] or 0),
            capture_rate=float(row["capture_rate"] or 0),
        )
        for row in rows
    ]

    return HistoricalGamesListResponse(
        games=games,
        total=total or 0,
        page=page,
        page_size=page_size,
    )


@app.get("/api/historical/games/{game_id}", response_model=ArchivedGameDetailResponse)
async def get_historical_game_detail(game_id: str):
    """Get detailed view of an archived game including trades and signals."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get game
    game_row = await pool.fetchrow("""
        SELECT
            archive_id, game_id, sport, home_team, away_team,
            final_home_score, final_away_score, ended_at, archived_at,
            total_trades, winning_trades, losing_trades, total_pnl,
            total_signals_generated, total_signals_executed,
            CASE WHEN total_trades > 0 THEN winning_trades::float / total_trades ELSE 0 END as win_rate,
            CASE WHEN total_signals_generated > 0 THEN total_signals_executed::float / total_signals_generated ELSE 0 END as capture_rate
        FROM archived_games
        WHERE game_id = $1
    """, game_id)

    if not game_row:
        raise HTTPException(status_code=404, detail="Game not found in archives")

    # Get trades
    trade_rows = await pool.fetch("""
        SELECT
            trade_id, signal_type, platform, market_type, side,
            entry_price, exit_price, size, opened_at, closed_at,
            outcome, pnl, edge_at_entry
        FROM archived_trades
        WHERE game_id = $1
        ORDER BY opened_at ASC
    """, game_id)

    trades = [
        ArchivedTradeResponse(
            trade_id=row["trade_id"],
            signal_type=row["signal_type"],
            platform=row["platform"],
            market_type=row["market_type"],
            side=row["side"],
            entry_price=float(row["entry_price"]),
            exit_price=float(row["exit_price"]) if row["exit_price"] else None,
            size=float(row["size"]),
            opened_at=row["opened_at"],
            closed_at=row["closed_at"],
            outcome=row["outcome"],
            pnl=float(row["pnl"]) if row["pnl"] else None,
            edge_at_entry=float(row["edge_at_entry"]) if row["edge_at_entry"] else None,
        )
        for row in trade_rows
    ]

    # Get signals
    signal_rows = await pool.fetch("""
        SELECT
            signal_id, signal_type, direction, team,
            model_prob, market_prob, edge_pct, generated_at, was_executed
        FROM archived_signals
        WHERE game_id = $1
        ORDER BY generated_at ASC
    """, game_id)

    signals = [
        ArchivedSignalResponse(
            signal_id=row["signal_id"],
            signal_type=row["signal_type"],
            direction=row["direction"],
            team=row["team"],
            model_prob=float(row["model_prob"]) if row["model_prob"] else None,
            market_prob=float(row["market_prob"]) if row["market_prob"] else None,
            edge_pct=float(row["edge_pct"]),
            generated_at=row["generated_at"],
            was_executed=row["was_executed"],
        )
        for row in signal_rows
    ]

    return ArchivedGameDetailResponse(
        archive_id=game_row["archive_id"],
        game_id=game_row["game_id"],
        sport=game_row["sport"],
        home_team=game_row["home_team"],
        away_team=game_row["away_team"],
        final_home_score=game_row["final_home_score"],
        final_away_score=game_row["final_away_score"],
        ended_at=game_row["ended_at"],
        archived_at=game_row["archived_at"],
        total_trades=game_row["total_trades"],
        winning_trades=game_row["winning_trades"],
        losing_trades=game_row["losing_trades"],
        total_pnl=float(game_row["total_pnl"] or 0),
        win_rate=float(game_row["win_rate"] or 0),
        capture_rate=float(game_row["capture_rate"] or 0),
        trades=trades,
        signals=signals,
    )


@app.get("/api/historical/summary", response_model=HistoricalSummaryResponse)
async def get_historical_summary(
    from_date: Optional[str] = Query(None, description="Start date (YYYY-MM-DD)"),
    to_date: Optional[str] = Query(None, description="End date (YYYY-MM-DD)"),
):
    """Get aggregate statistics for historical games."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    query = """
        SELECT
            COUNT(*) as total_games,
            COALESCE(SUM(total_trades), 0) as total_trades,
            COALESCE(SUM(total_pnl), 0) as total_pnl,
            COALESCE(SUM(winning_trades), 0) as total_wins,
            COALESCE(SUM(losing_trades), 0) as total_losses
        FROM archived_games
        WHERE 1=1
    """
    params = []
    param_idx = 1

    if from_date:
        query += f" AND ended_at >= ${param_idx}::date"
        params.append(from_date)
        param_idx += 1

    if to_date:
        query += f" AND ended_at <= ${param_idx}::date + INTERVAL '1 day'"
        params.append(to_date)
        param_idx += 1

    row = await pool.fetchrow(query, *params)

    total_trades = int(row["total_trades"] or 0)
    total_wins = int(row["total_wins"] or 0)

    return HistoricalSummaryResponse(
        total_games=int(row["total_games"] or 0),
        total_trades=total_trades,
        total_pnl=float(row["total_pnl"] or 0),
        overall_win_rate=(total_wins / total_trades) if total_trades > 0 else 0,
        total_wins=total_wins,
        total_losses=int(row["total_losses"] or 0),
    )


@app.get("/api/historical/by-sport")
async def get_historical_by_sport(
    days: int = Query(30, ge=1, le=365, description="Days to look back"),
):
    """Get historical performance breakdown by sport."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    rows = await pool.fetch("""
        SELECT
            sport,
            COUNT(*) as games,
            SUM(total_trades) as trades,
            SUM(winning_trades) as wins,
            SUM(losing_trades) as losses,
            SUM(total_pnl) as pnl,
            AVG(CASE WHEN total_trades > 0 THEN winning_trades::float / total_trades ELSE 0 END) as avg_win_rate
        FROM archived_games
        WHERE ended_at > NOW() - INTERVAL '%s days'
        GROUP BY sport
        ORDER BY pnl DESC
    """ % days)

    return [
        {
            "sport": row["sport"],
            "games": row["games"],
            "trades": int(row["trades"] or 0),
            "wins": int(row["wins"] or 0),
            "losses": int(row["losses"] or 0),
            "pnl": float(row["pnl"] or 0),
            "win_rate": float(row["avg_win_rate"] or 0),
        }
        for row in rows
    ]


@app.get("/api/historical/daily-summary")
async def get_historical_daily_summary(
    days: int = Query(30, ge=1, le=365, description="Days to look back"),
):
    """Get daily P&L summary for trend charts."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    rows = await pool.fetch("""
        SELECT
            DATE(ended_at) as game_date,
            COUNT(*) as games,
            SUM(total_trades) as trades,
            SUM(winning_trades) as wins,
            SUM(total_pnl) as pnl
        FROM archived_games
        WHERE ended_at > NOW() - INTERVAL '%s days'
        GROUP BY DATE(ended_at)
        ORDER BY game_date ASC
    """ % days)

    return [
        {
            "date": row["game_date"].isoformat(),
            "games": row["games"],
            "trades": int(row["trades"] or 0),
            "wins": int(row["wins"] or 0),
            "pnl": float(row["pnl"] or 0),
            "win_rate": (int(row["wins"] or 0) / int(row["trades"])) if row["trades"] else 0,
        }
        for row in rows
    ]


# =============================================================================
# ML Insights Endpoints
# =============================================================================

class MLReportSummaryResponse(BaseModel):
    """Summary of an ML analysis report."""
    report_date: str
    generated_at: datetime
    total_trades: int
    total_pnl: float
    win_rate: float
    best_sport: Optional[str]
    worst_sport: Optional[str]
    model_accuracy: Optional[float]
    recommendations_count: int


class MLReportDetailResponse(BaseModel):
    """Detailed ML analysis report."""
    report_date: str
    generated_at: datetime
    total_games: int
    total_trades: int
    total_pnl: float
    win_rate: float
    best_sport: Optional[str]
    best_sport_win_rate: Optional[float]
    worst_sport: Optional[str]
    worst_sport_win_rate: Optional[float]
    signals_generated: int
    signals_executed: int
    missed_opportunity_reasons: dict
    recommendations: list[dict]
    model_accuracy: Optional[float]
    feature_importance: dict
    report_markdown: Optional[str]


class MLModelInfoResponse(BaseModel):
    """Information about the ML model."""
    model_exists: bool
    trained_at: Optional[datetime]
    training_samples: int
    accuracy: Optional[float]
    top_features: list[tuple[str, float]]
    last_retrain_days_ago: Optional[int]
    next_retrain_days: Optional[int]


class MLHistoricalInsightsResponse(BaseModel):
    """Historical ML insights over a period."""
    period_days: int
    total_trades: int
    total_wins: int
    total_pnl: float
    win_rate: float
    by_sport: dict
    by_signal_type: dict
    by_edge_range: dict


@app.get("/api/ml/reports", response_model=list[MLReportSummaryResponse])
async def get_ml_reports(
    limit: int = Query(30, ge=1, le=365, description="Number of reports to return"),
):
    """Get list of ML analysis reports."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    rows = await pool.fetch("""
        SELECT
            report_date,
            generated_at,
            total_trades,
            total_pnl,
            win_rate,
            best_sport,
            worst_sport,
            model_accuracy,
            jsonb_array_length(COALESCE(recommendations, '[]'::jsonb)) as recommendations_count
        FROM ml_analysis_reports
        ORDER BY report_date DESC
        LIMIT $1
    """, limit)

    return [
        MLReportSummaryResponse(
            report_date=row["report_date"].isoformat(),
            generated_at=row["generated_at"],
            total_trades=row["total_trades"] or 0,
            total_pnl=float(row["total_pnl"] or 0),
            win_rate=float(row["win_rate"] or 0),
            best_sport=row["best_sport"],
            worst_sport=row["worst_sport"],
            model_accuracy=float(row["model_accuracy"]) if row["model_accuracy"] else None,
            recommendations_count=row["recommendations_count"] or 0,
        )
        for row in rows
    ]


@app.get("/api/ml/reports/{report_date}", response_model=MLReportDetailResponse)
async def get_ml_report_detail(report_date: str):
    """Get detailed ML analysis report for a specific date."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    row = await pool.fetchrow("""
        SELECT *
        FROM ml_analysis_reports
        WHERE report_date = $1::date
    """, report_date)

    if not row:
        raise HTTPException(status_code=404, detail=f"No report found for {report_date}")

    return MLReportDetailResponse(
        report_date=row["report_date"].isoformat(),
        generated_at=row["generated_at"],
        total_games=row["total_games"] or 0,
        total_trades=row["total_trades"] or 0,
        total_pnl=float(row["total_pnl"] or 0),
        win_rate=float(row["win_rate"] or 0),
        best_sport=row["best_sport"],
        best_sport_win_rate=float(row["best_sport_win_rate"]) if row["best_sport_win_rate"] else None,
        worst_sport=row["worst_sport"],
        worst_sport_win_rate=float(row["worst_sport_win_rate"]) if row["worst_sport_win_rate"] else None,
        signals_generated=row["signals_generated"] or 0,
        signals_executed=row["signals_executed"] or 0,
        missed_opportunity_reasons=row["missed_opportunity_reasons"] or {},
        recommendations=row["recommendations"] or [],
        model_accuracy=float(row["model_accuracy"]) if row["model_accuracy"] else None,
        feature_importance=row["feature_importance"] or {},
        report_markdown=row["report_markdown"],
    )


@app.get("/api/ml/reports/latest", response_model=MLReportDetailResponse)
async def get_latest_ml_report():
    """Get the most recent ML analysis report."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    row = await pool.fetchrow("""
        SELECT *
        FROM ml_analysis_reports
        ORDER BY report_date DESC
        LIMIT 1
    """)

    if not row:
        raise HTTPException(status_code=404, detail="No ML reports found")

    return MLReportDetailResponse(
        report_date=row["report_date"].isoformat(),
        generated_at=row["generated_at"],
        total_games=row["total_games"] or 0,
        total_trades=row["total_trades"] or 0,
        total_pnl=float(row["total_pnl"] or 0),
        win_rate=float(row["win_rate"] or 0),
        best_sport=row["best_sport"],
        best_sport_win_rate=float(row["best_sport_win_rate"]) if row["best_sport_win_rate"] else None,
        worst_sport=row["worst_sport"],
        worst_sport_win_rate=float(row["worst_sport_win_rate"]) if row["worst_sport_win_rate"] else None,
        signals_generated=row["signals_generated"] or 0,
        signals_executed=row["signals_executed"] or 0,
        missed_opportunity_reasons=row["missed_opportunity_reasons"] or {},
        recommendations=row["recommendations"] or [],
        model_accuracy=float(row["model_accuracy"]) if row["model_accuracy"] else None,
        feature_importance=row["feature_importance"] or {},
        report_markdown=row["report_markdown"],
    )


@app.get("/api/ml/insights", response_model=MLHistoricalInsightsResponse)
async def get_ml_insights(
    days: int = Query(30, ge=1, le=365, description="Days to analyze"),
):
    """Get aggregated ML insights over a time period."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get performance by sport from paper_trades
    sport_rows = await pool.fetch("""
        SELECT
            sport,
            COUNT(*) as trades,
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            SUM(COALESCE(pnl, 0)) as pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY sport
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
            "win_rate": (wins / trades) if trades > 0 else 0,
        }

    # Get performance by signal type
    signal_rows = await pool.fetch("""
        SELECT
            signal_type,
            COUNT(*) as trades,
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            SUM(COALESCE(pnl, 0)) as pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY signal_type
    """ % days)

    by_signal_type = {}
    for row in signal_rows:
        sig_type = row["signal_type"] or "unknown"
        trades = int(row["trades"])
        wins = int(row["wins"])
        by_signal_type[sig_type] = {
            "trades": trades,
            "wins": wins,
            "pnl": float(row["pnl"] or 0),
            "win_rate": (wins / trades) if trades > 0 else 0,
        }

    # Get performance by edge range
    edge_rows = await pool.fetch("""
        SELECT
            CASE
                WHEN edge_at_entry < 1 THEN '0-1%%'
                WHEN edge_at_entry < 2 THEN '1-2%%'
                WHEN edge_at_entry < 3 THEN '2-3%%'
                WHEN edge_at_entry < 5 THEN '3-5%%'
                ELSE '5%%+'
            END as edge_range,
            COUNT(*) as trades,
            COUNT(*) FILTER (WHERE outcome = 'win') as wins,
            SUM(COALESCE(pnl, 0)) as pnl
        FROM paper_trades
        WHERE status = 'closed'
          AND exit_time >= NOW() - INTERVAL '%s days'
        GROUP BY edge_range
    """ % days)

    by_edge_range = {}
    for row in edge_rows:
        edge_range = row["edge_range"]
        trades = int(row["trades"])
        wins = int(row["wins"])
        by_edge_range[edge_range] = {
            "trades": trades,
            "wins": wins,
            "pnl": float(row["pnl"] or 0),
            "win_rate": (wins / trades) if trades > 0 else 0,
        }

    # Calculate totals
    total_trades = sum(s["trades"] for s in by_sport.values())
    total_wins = sum(s["wins"] for s in by_sport.values())
    total_pnl = sum(s["pnl"] for s in by_sport.values())

    return MLHistoricalInsightsResponse(
        period_days=days,
        total_trades=total_trades,
        total_wins=total_wins,
        total_pnl=total_pnl,
        win_rate=(total_wins / total_trades) if total_trades > 0 else 0,
        by_sport=by_sport,
        by_signal_type=by_signal_type,
        by_edge_range=by_edge_range,
    )


@app.get("/api/ml/recommendations")
async def get_ml_recommendations(
    days: int = Query(7, ge=1, le=30, description="Days to look back for recommendations"),
):
    """Get recent parameter optimization recommendations."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    rows = await pool.fetch("""
        SELECT
            report_date,
            recommendations
        FROM ml_analysis_reports
        WHERE recommendations IS NOT NULL
          AND jsonb_array_length(recommendations) > 0
          AND report_date >= CURRENT_DATE - INTERVAL '%s days'
        ORDER BY report_date DESC
    """ % days)

    all_recommendations = []
    for row in rows:
        report_date = row["report_date"].isoformat()
        for rec in (row["recommendations"] or []):
            all_recommendations.append({
                "report_date": report_date,
                **rec,
            })

    return all_recommendations


@app.get("/api/ml/parameter-history")
async def get_parameter_history(
    parameter: Optional[str] = Query(None, description="Filter by parameter name"),
    limit: int = Query(50, ge=1, le=200, description="Number of records to return"),
):
    """Get history of parameter changes."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    query = """
        SELECT
            parameter_name,
            old_value,
            new_value,
            changed_at,
            change_reason,
            triggered_by
        FROM parameter_history
    """
    params = []
    param_idx = 1

    if parameter:
        query += f" WHERE parameter_name = ${param_idx}"
        params.append(parameter)
        param_idx += 1

    query += f" ORDER BY changed_at DESC LIMIT ${param_idx}"
    params.append(limit)

    rows = await pool.fetch(query, *params)

    return [
        {
            "parameter_name": row["parameter_name"],
            "old_value": row["old_value"],
            "new_value": row["new_value"],
            "changed_at": row["changed_at"].isoformat() if row["changed_at"] else None,
            "change_reason": row["change_reason"],
            "triggered_by": row["triggered_by"],
        }
        for row in rows
    ]


@app.get("/api/ml/feature-importance")
async def get_feature_importance():
    """Get current feature importance from the trained model."""
    if not db:
        raise HTTPException(status_code=503, detail="Database not available")

    pool = await get_pool()

    # Get most recent report with feature importance
    row = await pool.fetchrow("""
        SELECT feature_importance, model_accuracy, generated_at
        FROM ml_analysis_reports
        WHERE feature_importance IS NOT NULL
          AND feature_importance != '{}'::jsonb
        ORDER BY report_date DESC
        LIMIT 1
    """)

    if not row or not row["feature_importance"]:
        return {
            "has_model": False,
            "features": [],
            "model_accuracy": None,
            "last_updated": None,
        }

    # Sort features by importance
    features = sorted(
        row["feature_importance"].items(),
        key=lambda x: x[1],
        reverse=True
    )

    return {
        "has_model": True,
        "features": [{"name": k, "importance": v} for k, v in features],
        "model_accuracy": float(row["model_accuracy"]) if row["model_accuracy"] else None,
        "last_updated": row["generated_at"].isoformat() if row["generated_at"] else None,
    }


@app.post("/api/ml/trigger-analysis")
async def trigger_ml_analysis(
    for_date: Optional[str] = Query(None, description="Date to analyze (YYYY-MM-DD), defaults to yesterday"),
):
    """Trigger an on-demand ML analysis (admin endpoint).

    Note: This is a lightweight trigger. The actual analysis runs asynchronously
    in the ML Analyzer service. Returns immediately after queuing the request.
    """
    if not redis:
        raise HTTPException(status_code=503, detail="Redis not available")

    from datetime import date as date_type, timedelta

    # Parse date or use yesterday
    if for_date:
        try:
            analysis_date = datetime.strptime(for_date, "%Y-%m-%d").date()
        except ValueError:
            raise HTTPException(status_code=400, detail="Invalid date format. Use YYYY-MM-DD")
    else:
        analysis_date = date_type.today() - timedelta(days=1)

    # Publish request to Redis for ML Analyzer to pick up
    await redis.publish("ml:analysis:request", {
        "date": analysis_date.isoformat(),
        "requested_at": datetime.utcnow().isoformat(),
        "requested_by": "api",
    })

    return {
        "status": "queued",
        "analysis_date": analysis_date.isoformat(),
        "message": "Analysis request queued. Check /api/ml/reports for results.",
    }


# =============================================================================
# Main
# =============================================================================

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8000)
