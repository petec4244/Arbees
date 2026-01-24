"""
GameShard service for monitoring multiple games via asyncio.

KEY IMPROVEMENT over container-per-game:
- Single process handles 10-20 games concurrently
- Shared connections to DB, Redis, markets
- Dynamic poll intervals (1s crunch time, 30s halftime)
- 10x lower memory footprint
- Instant game start (no container spawn)
"""

import asyncio
import logging
import os
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Optional

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.game import GameState, Play, Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.market_types import MarketType, ParsedMarket
from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from data_providers.espn.client import ESPNClient
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from markets.kalshi.hybrid_client import HybridKalshiClient
from markets.polymarket.hybrid_client import HybridPolymarketClient
from services.market_discovery.parser import parse_market
from arbees_shared.utils.trace_logger import trace_log
import arbees_core

logger = logging.getLogger(__name__)


@dataclass
class GameContext:
    """Context for a monitored game."""
    game_id: str
    sport: Sport
    espn_client: ESPNClient
    last_state: Optional[GameState] = None
    last_home_win_prob: Optional[float] = None
    # Multi-market type support: prices indexed by (MarketType, Platform)
    market_prices: dict[Platform, MarketPrice] = field(default_factory=dict)
    market_prices_by_type: dict[tuple[MarketType, Platform], MarketPrice] = field(default_factory=dict)
    # NEW: Team-specific prices for Polymarket moneyline (home team + away team)
    # Key: (Platform, team_name) -> MarketPrice
    # This allows matching signal.team to the correct contract
    market_prices_by_team: dict[tuple[Platform, str], MarketPrice] = field(default_factory=dict)
    # Legacy single market IDs (for backwards compatibility)
    market_ids: dict[Platform, str] = field(default_factory=dict)
    # NEW: Multiple market types per game (3-8x more arbitrage opportunities)
    market_ids_by_type: dict[MarketType, dict[Platform, str]] = field(default_factory=dict)
    # Track market titles for parsing
    market_titles: dict[str, str] = field(default_factory=dict)  # market_id -> title
    plays_detected: int = 0
    signals_generated: int = 0
    started_at: datetime = field(default_factory=datetime.utcnow)
    is_active: bool = True
    # Track active signals per market type for hysteresis
    active_signal: Optional[TradingSignal] = None  # Legacy
    active_signals_by_type: dict[MarketType, TradingSignal] = field(default_factory=dict)
    # NEW: Trading cooldown timestamp
    cooldown_until: Optional[datetime] = None


class GameShard:
    """
    GameShard handles multiple games concurrently via asyncio.

    Benefits over container-per-game:
    - Shared market client connections
    - Lower memory footprint
    - Instant game start
    - Better resource utilization
    """

    def __init__(
        self,
        shard_id: Optional[str] = None,
        max_games: int = 20,
        default_poll_interval: float = float(os.environ.get("POLL_INTERVAL", 1.0)),
        crunch_time_interval: float = float(os.environ.get("CRUNCH_TIME_INTERVAL", 0.5)),
        halftime_interval: float = float(os.environ.get("HALFTIME_INTERVAL", 30.0)),
        market_data_ttl: float = float(os.environ.get("MARKET_DATA_TTL", 4.0)),
        sync_delta_tolerance: float = float(os.environ.get("SYNC_DELTA_TOLERANCE", 2.0)),
        use_websocket_streaming: bool = True,
    ):
        """
        Initialize GameShard.

        Args:
            shard_id: Unique identifier for this shard
            max_games: Maximum concurrent games
            default_poll_interval: Normal poll interval in seconds
            crunch_time_interval: Poll interval for close games
            halftime_interval: Poll interval during halftime
            market_data_ttl: Max age of market data in seconds
            sync_delta_tolerance: Max allowed delta between game and market timestamps
            use_websocket_streaming: If True, use WebSocket for real-time prices (10-50ms latency)
        """
        self.shard_id = shard_id or os.environ.get("SHARD_ID", str(uuid.uuid4())[:8])
        self.max_games = max_games
        self.default_poll_interval = default_poll_interval
        self.crunch_time_interval = crunch_time_interval
        self.halftime_interval = halftime_interval
        self.market_data_ttl = market_data_ttl
        self.sync_delta_tolerance = sync_delta_tolerance
        self.use_websocket_streaming = use_websocket_streaming

        # VPN-based Polymarket mode: consume Polymarket prices from Redis
        # instead of connecting directly (requires polymarket_monitor service)
        # Phase 3: This should ALWAYS be true in production - direct mode is deprecated
        self.polymarket_via_redis = os.environ.get("POLYMARKET_VIA_REDIS", "true").lower() == "true"
        
        if not self.polymarket_via_redis:
            logger.warning(
                "POLYMARKET_VIA_REDIS=false is DEPRECATED. Direct Polymarket access "
                "from game_shard is not recommended. Use polymarket_monitor service instead."
            )

        # Connections (shared across all games)
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None

        # Hybrid clients for WebSocket streaming (10-50ms latency vs 500-3000ms polling)
        self.kalshi_hybrid: Optional[HybridKalshiClient] = None
        self.polymarket_hybrid: Optional[HybridPolymarketClient] = None

        # Game tracking
        self._games: dict[str, GameContext] = {}
        self._game_tasks: dict[str, asyncio.Task] = {}
        self._running = False
        self._heartbeat_task: Optional[asyncio.Task] = None

        # Heartbeat publisher for health monitoring
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

        # WebSocket streaming tasks
        self._ws_stream_tasks: dict[Platform, asyncio.Task] = {}
        self._market_to_game: dict[str, str] = {}  # market_id -> game_id mapping

        # NEW: Circuit breaker from Rust (terauss integration)
        self.circuit_breaker = arbees_core.CircuitBreaker(
            arbees_core.CircuitBreakerConfig(
                max_position_per_market=int(os.environ.get("MAX_POSITION_PER_MARKET", 50000)),
                max_total_position=int(os.environ.get("MAX_TOTAL_POSITION", 100000)),
                max_daily_loss=float(os.environ.get("MAX_DAILY_LOSS", 500.0)),
                max_consecutive_errors=int(os.environ.get("MAX_CONSECUTIVE_ERRORS", 5)),
                cooldown_secs=int(os.environ.get("CIRCUIT_BREAKER_COOLDOWN", 300)),
                enabled=os.environ.get("CIRCUIT_BREAKER_ENABLED", "true").lower() == "true",
            )
        )
        logger.info(f"Circuit breaker initialized (enabled={self.circuit_breaker.is_trading_allowed()})")

    @property
    def game_count(self) -> int:
        """Number of active games."""
        return len(self._games)

    @property
    def can_accept_games(self) -> bool:
        """Whether shard can accept more games."""
        return self.game_count < self.max_games

    async def start(self) -> None:
        """Start the shard and connect to services."""
        logger.info(
            f"Starting GameShard {self.shard_id} "
            f"(websocket_streaming={self.use_websocket_streaming}, "
            f"polymarket_via_redis={self.polymarket_via_redis})"
        )

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Connect to market clients
        if self.use_websocket_streaming:
            # Use hybrid clients for WebSocket streaming
            self.kalshi_hybrid = HybridKalshiClient()
            await self.kalshi_hybrid.connect()
            # Also keep REST client as fallback
            self.kalshi = self.kalshi_hybrid._rest

            # Only connect to Polymarket directly if NOT using Redis mode
            if not self.polymarket_via_redis:
                self.polymarket_hybrid = HybridPolymarketClient()
                await self.polymarket_hybrid.connect()
                self.polymarket = self.polymarket_hybrid._rest
                logger.info("Using WebSocket streaming for Polymarket prices (direct)")
            else:
                logger.info("Polymarket prices via Redis (VPN monitor)")

            logger.info("Using WebSocket streaming for Kalshi prices (10-50ms latency)")
        else:
            # REST-only clients
            self.kalshi = KalshiClient()
            await self.kalshi.connect()

            # Only connect to Polymarket directly if NOT using Redis mode
            if not self.polymarket_via_redis:
                self.polymarket = PolymarketClient()
                await self.polymarket.connect()
                logger.info("Using REST polling for Polymarket prices (direct)")
            else:
                logger.info("Polymarket prices via Redis (VPN monitor)")

            logger.info("Using REST polling for Kalshi prices")

        self._running = True

        # Subscribe to commands from orchestrator
        command_channel = f"shard:{self.shard_id}:command"
        await self.redis.subscribe(command_channel, self._handle_command)
        asyncio.create_task(self.redis.start_listening())

        # Start shard heartbeat (to orchestrator)
        self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        # Start health monitoring heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="game_shard",
            instance_id=self.shard_id,
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
            "kalshi_ws_ok": self.use_websocket_streaming,
            "polymarket_via_redis": self.polymarket_via_redis,
        })

        logger.info(f"GameShard {self.shard_id} started")

    async def _handle_command(self, data: dict) -> None:
        """Handle command from orchestrator."""
        cmd_type = data.get("type")

        if cmd_type == "add_game":
            game_id = data.get("game_id")
            sport_str = data.get("sport")
            kalshi_id = data.get("kalshi_market_id")
            poly_id = data.get("polymarket_market_id")

            # NEW: Support multi-market type format
            # Format: {"moneyline": {"kalshi": "id1", "polymarket": "id2"}, ...}
            market_ids_by_type_raw = data.get("market_ids_by_type")
            market_ids_by_type = None

            if market_ids_by_type_raw:
                market_ids_by_type = {}
                for type_str, platforms in market_ids_by_type_raw.items():
                    try:
                        market_type = MarketType(type_str)
                        market_ids_by_type[market_type] = {}
                        for plat_str, market_id in platforms.items():
                            platform = Platform(plat_str)
                            market_ids_by_type[market_type][platform] = market_id
                    except ValueError as e:
                        logger.warning(f"Unknown market type or platform: {e}")

            if game_id and sport_str:
                sport = Sport(sport_str)
                logger.info(f"Received add_game command: {game_id} ({sport_str}) kalshi={kalshi_id} poly={poly_id} multi_market={market_ids_by_type is not None}")
                await self.add_game(game_id, sport, kalshi_id, poly_id, market_ids_by_type)

        elif cmd_type == "remove_game":
            game_id = data.get("game_id")
            if game_id:
                logger.info(f"Received remove_game command: {game_id}")
                await self.remove_game(game_id)

    async def stop(self) -> None:
        """Stop the shard gracefully."""
        logger.info(f"Stopping GameShard {self.shard_id}")
        self._running = False

        # Stop heartbeat publisher
        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        # Cancel heartbeat
        if self._heartbeat_task:
            self._heartbeat_task.cancel()

        # Cancel WebSocket stream tasks
        for platform, task in self._ws_stream_tasks.items():
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
        self._ws_stream_tasks.clear()

        # Stop all game monitoring
        for game_id in list(self._games.keys()):
            await self.remove_game(game_id)

        # Disconnect from services
        if self.kalshi_hybrid:
            await self.kalshi_hybrid.disconnect()
        elif self.kalshi:
            await self.kalshi.disconnect()
        if self.polymarket_hybrid:
            await self.polymarket_hybrid.disconnect()
        elif self.polymarket:
            await self.polymarket.disconnect()
        if self.redis:
            await self.redis.disconnect()

        logger.info(f"GameShard {self.shard_id} stopped")

    async def _heartbeat_loop(self) -> None:
        """Send periodic heartbeats to orchestrator."""
        while self._running:
            try:
                # Get circuit breaker status
                cb_status = self.circuit_breaker.status()

                status = {
                    "shard_id": self.shard_id,
                    "game_count": self.game_count,
                    "max_games": self.max_games,
                    "games": list(self._games.keys()),
                    "timestamp": datetime.utcnow().isoformat(),
                    # NEW: Circuit breaker status (terauss integration)
                    "circuit_breaker": {
                        "trading_allowed": self.circuit_breaker.is_trading_allowed(),
                        "daily_pnl": cb_status.get("daily_pnl_dollars", 0.0),
                        "consecutive_errors": cb_status.get("consecutive_errors", 0),
                    },
                }
                if self.redis:
                    await self.redis.publish_shard_heartbeat(self.shard_id, status)

                # Update health monitoring heartbeat metrics
                if self._heartbeat_publisher:
                    total_signals = sum(ctx.signals_generated for ctx in self._games.values())
                    self._heartbeat_publisher.update_metrics({
                        "games_monitored": float(self.game_count),
                        "signals_generated": float(total_signals),
                        "circuit_breaker_ok": 1.0 if self.circuit_breaker.is_trading_allowed() else 0.0,
                    })
            except Exception as e:
                logger.warning(f"Heartbeat failed: {e}")

            await asyncio.sleep(10)

    # ==========================================================================
    # Game Management
    # ==========================================================================

    async def add_game(
        self,
        game_id: str,
        sport: Sport,
        kalshi_market_id: Optional[str] = None,
        polymarket_market_id: Optional[str] = None,
        market_ids_by_type: Optional[dict[MarketType, dict[Platform, str]]] = None,
    ) -> bool:
        """
        Start monitoring a game.

        Args:
            game_id: ESPN game ID
            sport: Sport type
            kalshi_market_id: Optional Kalshi market to monitor (legacy, for backwards compat)
            polymarket_market_id: Optional Polymarket market to monitor (legacy)
            market_ids_by_type: NEW - Multiple market types per game for 3-8x more opportunities
                Example: {
                    MarketType.MONEYLINE: {Platform.KALSHI: "id1", Platform.POLYMARKET: "id2"},
                    MarketType.SPREAD: {Platform.KALSHI: "id3", Platform.POLYMARKET: "id4"},
                    MarketType.TOTAL: {Platform.KALSHI: "id5", Platform.POLYMARKET: "id6"},
                }

        Returns:
            True if game was added
        """
        # Allow id updates if the orchestrator re-sends assignments (e.g. market discovery correction).
        if game_id in self._games:
            ctx = self._games[game_id]

            updated = False

            # Update multi-market mapping if provided
            if market_ids_by_type:
                ctx.market_ids_by_type = market_ids_by_type
                for market_type, platforms in market_ids_by_type.items():
                    for platform, market_id in platforms.items():
                        self._market_to_game[market_id] = game_id
                        logger.info(
                            f"Game {game_id}: UPDATED {market_type.value} {platform.value} market -> {market_id}"
                        )
                # Back-compat: keep legacy moneyline mapping up to date
                if MarketType.MONEYLINE in market_ids_by_type:
                    ctx.market_ids.update(market_ids_by_type[MarketType.MONEYLINE])
                updated = True

            # Legacy: update single market IDs
            if kalshi_market_id and ctx.market_ids.get(Platform.KALSHI) != kalshi_market_id:
                old = ctx.market_ids.get(Platform.KALSHI)
                ctx.market_ids[Platform.KALSHI] = kalshi_market_id
                self._market_to_game[kalshi_market_id] = game_id
                logger.info(f"Game {game_id}: UPDATED Kalshi market {old} -> {kalshi_market_id}")
                updated = True

            if polymarket_market_id and ctx.market_ids.get(Platform.POLYMARKET) != polymarket_market_id:
                old = ctx.market_ids.get(Platform.POLYMARKET)
                ctx.market_ids[Platform.POLYMARKET] = polymarket_market_id
                self._market_to_game[polymarket_market_id] = game_id
                logger.info(f"Game {game_id}: UPDATED Polymarket market {old} -> {polymarket_market_id}")
                updated = True

            if not updated:
                logger.warning(f"Game {game_id} already being monitored")
            return updated

        if not self.can_accept_games:
            logger.warning(f"Shard at capacity ({self.game_count}/{self.max_games})")
            return False

        # Create ESPN client for this sport
        espn_client = ESPNClient(sport)
        await espn_client.connect()

        # Create game context
        ctx = GameContext(
            game_id=game_id,
            sport=sport,
            espn_client=espn_client,
        )

        # NEW: Multi-market type support (3-8x more arbitrage opportunities)
        if market_ids_by_type:
            ctx.market_ids_by_type = market_ids_by_type
            for market_type, platforms in market_ids_by_type.items():
                for platform, market_id in platforms.items():
                    self._market_to_game[market_id] = game_id
                    logger.info(f"Game {game_id}: {market_type.value} {platform.value} market set to {market_id}")

            # Also populate legacy market_ids with moneyline for backwards compat
            if MarketType.MONEYLINE in market_ids_by_type:
                ctx.market_ids = market_ids_by_type[MarketType.MONEYLINE].copy()

        # Legacy: Track single market IDs (backwards compatibility)
        if kalshi_market_id:
            ctx.market_ids[Platform.KALSHI] = kalshi_market_id
            self._market_to_game[kalshi_market_id] = game_id
            logger.info(f"Game {game_id}: Kalshi market set to {kalshi_market_id}")
        if polymarket_market_id:
            ctx.market_ids[Platform.POLYMARKET] = polymarket_market_id
            self._market_to_game[polymarket_market_id] = game_id
            logger.info(f"Game {game_id}: Polymarket market set to {polymarket_market_id}")

        self._games[game_id] = ctx

        # Subscribe to WebSocket streaming if enabled (for Kalshi, and Polymarket if not via Redis)
        if self.use_websocket_streaming:
            await self._subscribe_to_ws_streams(ctx)

        # Subscribe to Redis price channel for Polymarket prices when using VPN monitor
        if self.polymarket_via_redis and self.redis:
            await self._subscribe_to_polymarket_redis(ctx)

        # Start monitoring task
        task = asyncio.create_task(self._monitor_game(ctx))
        self._game_tasks[game_id] = task

        market_count = len(ctx.market_ids_by_type) if ctx.market_ids_by_type else len(ctx.market_ids)
        logger.info(f"Added game {game_id} ({sport.value}) to shard {self.shard_id} with {market_count} market type(s)")
        return True

    async def remove_game(self, game_id: str) -> bool:
        """Stop monitoring a game."""
        if game_id not in self._games:
            return False

        ctx = self._games[game_id]
        ctx.is_active = False

        # Best-effort settlement on removal.
        # Orchestrator may remove a game once it no longer appears "live", even if ESPN lags flipping
        # to FINAL. We should not leave open paper trades stranded.
        if self.db and ctx.last_state:
            try:
                open_positions = await self.db.get_open_positions_for_game(ctx.game_id)
                if open_positions:
                    if self._is_game_complete(ctx.last_state) or ctx.last_state.game_progress >= 0.98:
                        logger.info(
                            f"Game {ctx.game_id} removed with {len(open_positions)} open positions; "
                            f"attempting settlement (status={ctx.last_state.status}, progress={ctx.last_state.game_progress:.3f})"
                        )
                        await self.db.update_game_status(ctx.game_id, "final")
                        await self._settle_game_positions(ctx)
            except Exception as e:
                logger.error(f"Failed settlement on remove_game for {ctx.game_id}: {e}")

        # Cancel monitoring task
        task = self._game_tasks.get(game_id)
        if task:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
            del self._game_tasks[game_id]

        # Unsubscribe from WebSocket streams
        if self.use_websocket_streaming:
            await self._unsubscribe_from_ws_streams(ctx)

        # Clean up market_to_game mappings (legacy)
        for platform, market_id in ctx.market_ids.items():
            self._market_to_game.pop(market_id, None)

        # Clean up multi-market type mappings
        for market_type, platforms in ctx.market_ids_by_type.items():
            for platform, market_id in platforms.items():
                self._market_to_game.pop(market_id, None)

        # Disconnect ESPN client
        await ctx.espn_client.disconnect()

        # Finalize game state in DB if possible
        if self.db and ctx.last_state:
            # We mark it as valid to update the timestamp, but we don't force 'final'
            # unless we know for sure. However, if we are removing it, it's likely done.
            # Safe bet: just update timestamp so it falls out of "live" queries eventually
            pass

        del self._games[game_id]
        logger.info(f"Removed game {game_id} from shard {self.shard_id}")
        return True

    # ==========================================================================
    # Game Monitoring Loop
    # ==========================================================================

    async def _monitor_game(self, ctx: GameContext) -> None:
        """Main monitoring loop for a game."""
        logger.info(f"Starting monitoring for game {ctx.game_id}")

        try:
            while ctx.is_active and self._running:
                # Get poll interval based on game state
                interval = self._get_poll_interval(ctx)

                # Poll game state and market prices in parallel
                # IMPORTANT: never let a transient market poll failure kill the entire
                # game monitor loop (which would prevent end-of-game settlement).
                results = await asyncio.gather(
                    self._poll_game_state(ctx),
                    self._poll_market_prices(ctx),
                    return_exceptions=True,
                )
                for r in results:
                    if isinstance(r, Exception):
                        logger.warning(f"Monitor subtask error for game {ctx.game_id}: {r}")

                # Check if game is final
                if ctx.last_state and self._is_game_complete(ctx.last_state):
                    logger.info(f"Game {ctx.game_id} is final: {ctx.last_state.home_team} {ctx.last_state.home_score} - {ctx.last_state.away_score} {ctx.last_state.away_team}")

                    # Ensure final state is persisted
                    if self.db:
                        await self.db.update_game_status(ctx.game_id, "final")

                    # Settle all open positions for this game
                    await self._settle_game_positions(ctx)

                    break

                await asyncio.sleep(interval)

        except asyncio.CancelledError:
            logger.info(f"Monitoring cancelled for game {ctx.game_id}")
        except Exception as e:
            logger.error(f"Error monitoring game {ctx.game_id}: {e}")
        finally:
            # Clean up
            ctx.is_active = False

            # Safety net: if we already know the game is complete, ensure we settle even if
            # the loop exited due to errors/cancellation.
            if self.db and ctx.last_state and self._is_game_complete(ctx.last_state):
                try:
                    # Ensure final state is persisted
                    await self.db.update_game_status(ctx.game_id, "final")
                    await self._settle_game_positions(ctx)
                except Exception as e:
                    logger.error(f"Failed to settle positions in cleanup for game {ctx.game_id}: {e}")

    def _is_game_complete(self, state: GameState) -> bool:
        """Return True if the game should be treated as complete/final."""
        status = (state.status or "").lower()

        # Canonical statuses
        if status in ("final", "complete", "completed"):
            return True

        # ESPN sometimes emits raw codes like "status_final" / "status_complete"
        if status.endswith("_final") or status.endswith("_complete") or status.endswith("_completed"):
            return True

        # Fallback: substring match for safety
        if "final" in status or "complete" in status:
            return True

        # Heuristic: end of regulation and clock at 0:00.
        # This helps when orchestrator stops sending the game as "live" before ESPN flips to FINAL,
        # or when finalization is delayed.
        if status in ("end_period", "status_end_period") and state.time_remaining_seconds == 0:
            # Only treat as complete when we are at/after the final scheduled period.
            if state.period >= state.sport.periods:
                return True

        return False
    async def _settle_game_positions(self, ctx: GameContext) -> None:
        """
        Settle all open paper trading positions for a completed game.

        Settlement uses the final calculated win probability as exit price:
        - Winning team's probability → ~1.0 (they won)
        - Losing team's probability → ~0.0 (they lost)

        For each position:
        - BUY on winning team: profit (exit at high price)
        - BUY on losing team: loss (exit at low price)
        - SELL on winning team: loss (have to pay out high)
        - SELL on losing team: profit (keep premium, pay nothing)
        """
        if not self.db or not ctx.last_state:
            return

        # Get all open positions for this game
        open_positions = await self.db.get_open_positions_for_game(ctx.game_id)
        if not open_positions:
            logger.info(f"No open positions to settle for game {ctx.game_id}")
            return

        logger.info(f"Settling {len(open_positions)} open positions for game {ctx.game_id}")

        # Determine winner from final score
        home_won = ctx.last_state.home_score > ctx.last_state.away_score
        home_team = ctx.last_state.home_team
        away_team = ctx.last_state.away_team

        # Game-end settlement: contracts settle at exactly 1.0 or 0.0
        # No uncertainty - winner pays $1 per contract, loser pays $0
        winning_exit_price = 1.0
        losing_exit_price = 0.0

        total_pnl = 0.0
        # TIMESTAMPTZ in DB expects datetime, not string
        exit_time = datetime.now(timezone.utc)

        for position in open_positions:
            trade_id = position['trade_id']
            market_title = position['market_title'] or ""
            side = position['side']
            entry_price = float(position['entry_price'])
            size = float(position['size'])

            # Determine if this position is on the winning or losing team
            # Market titles are like "Boston Celtics to win" or "Detroit Pistons to win"
            market_title_lower = market_title.lower()

            # Score matches by specificity (prefer exact nickname match over partial)
            def score_match(team: str, abbrev: str | None, title: str) -> int:
                """Return match score: 0=no match, 1=partial, 2=full team, 3=nickname exact"""
                if not team:
                    return 0
                team_lower = team.lower()
                # Check for team nickname (last word) as exact word match
                nickname = team_lower.split()[-1] if team_lower.split() else ""
                if nickname and f" {nickname}" in title or title.startswith(nickname):
                    return 3
                # Full team name match
                if team_lower in title:
                    return 2
                # Abbreviation match
                if abbrev and abbrev.lower() in title:
                    return 1
                return 0

            home_score = score_match(home_team, ctx.last_state.home_team_abbrev, market_title_lower)
            away_score = score_match(away_team, ctx.last_state.away_team_abbrev, market_title_lower)

            # Use score to determine which team, preferring higher confidence matches
            if home_score > away_score and home_score > 0:
                exit_price = winning_exit_price if home_won else losing_exit_price
                team_won = home_won
            elif away_score > home_score and away_score > 0:
                exit_price = losing_exit_price if home_won else winning_exit_price
                team_won = not home_won
            elif home_score == away_score and home_score > 0:
                # Both teams match equally (ambiguous) - log warning and settle as push
                logger.warning(
                    f"Ambiguous team match for market '{market_title}' "
                    f"(home='{home_team}' score={home_score}, away='{away_team}' score={away_score}), "
                    f"settling as push"
                )
                exit_price = entry_price
                team_won = None
            else:
                # Can't determine team from market title, use entry price (push)
                logger.warning(f"Cannot determine team for market '{market_title}', settling as push")
                exit_price = entry_price
                team_won = None

            # Determine outcome based on position side and whether team won
            if team_won is None:
                outcome = "push"
            elif side == "buy":
                outcome = "win" if team_won else "loss"
            else:  # sell
                outcome = "loss" if team_won else "win"

            # Calculate P&L
            if side == "buy":
                pnl = size * (exit_price - entry_price)
            else:
                pnl = size * (entry_price - exit_price)

            total_pnl += pnl

            # Close the position
            await self.db.close_paper_trade(
                trade_id=trade_id,
                exit_price=exit_price,
                exit_time=exit_time,
                outcome=outcome,
            )
            
            logger.info(
                f"Settled position {trade_id}: {side.upper()} {market_title} "
                f"entry=${entry_price:.4f} exit=${exit_price:.4f} "
                f"P&L=${pnl:.2f} ({outcome.upper()})"
            )

        # ---------------------------------------------------------
        # Piggybank & Bankroll Update
        # ---------------------------------------------------------
        piggybank_amount = 0.0
        if total_pnl > 0:
            # "50% of all positive trades are kept for a safety buffer"
            # We assume this means 50% of the NET profit from the game
            piggybank_amount = total_pnl * 0.50

        # Update bankroll via DatabaseClient
        if self.db:
            await self.db.update_bankroll(
                pnl_change=total_pnl,
                piggybank_change=piggybank_amount,
                account_name="default"
            )

        # ---------------------------------------------------------
        # Trading Cooldown
        # ---------------------------------------------------------
        # "3 mins for positive trades and 5 minutes for negative"
        cooldown_minutes = 3 if total_pnl > 0 else 5
        cooldown_until = datetime.now(timezone.utc) + timedelta(minutes=cooldown_minutes)
        
        ctx.cooldown_until = cooldown_until
        
        if self.db:
            await self.db.set_game_cooldown(ctx.game_id, cooldown_until)
            
        logger.info(
            f"Settled game {ctx.game_id}: PnL=${total_pnl:.2f}, "
            f"Piggybank=${piggybank_amount:.2f}, "
            f"Cooldown until {cooldown_until.isoformat()} ({cooldown_minutes}m)"
        )

    def _get_poll_interval(self, ctx: GameContext) -> float:
        """Get poll interval based on game state."""
        if ctx.last_state is None:
            return self.default_poll_interval

        # Halftime - poll less frequently
        if ctx.last_state.status == "halftime":
            return self.halftime_interval

        # Crunch time - poll more frequently
        if self._is_crunch_time(ctx):
            return self.crunch_time_interval

        return self.default_poll_interval

    def _is_crunch_time(self, ctx: GameContext) -> bool:
        """Check if game is in crunch time (close score, little time left)."""
        state = ctx.last_state
        if state is None:
            return False

        # Close score (within one possession)
        close_score = abs(state.score_diff) <= 8

        # Late in game
        late_game = state.game_progress > 0.85

        # 4th quarter/period
        final_period = state.period >= state.sport.periods

        return close_score and (late_game or final_period)

    async def _poll_game_state(self, ctx: GameContext) -> None:
        """Poll ESPN for game state updates."""
        try:
            new_state, new_plays = await ctx.espn_client.poll_game(
                ctx.game_id,
                ctx.last_state,
            )

            if new_state is None:
                return

            # Upsert game info (to get team names in the games table)
            if self.db and new_state.home_team and new_state.away_team:
                try:
                    logger.info(f"Upserting game {ctx.game_id}: {new_state.away_team} @ {new_state.home_team}")
                    await self.db.upsert_game(
                        game_id=ctx.game_id,
                        sport=ctx.sport.value,
                        home_team=new_state.home_team,
                        away_team=new_state.away_team,
                        scheduled_time=datetime.utcnow(),  # Pass datetime object, not string
                        home_team_abbrev=getattr(new_state, 'home_team_abbrev', None),
                        away_team_abbrev=getattr(new_state, 'away_team_abbrev', None),
                    )
                except Exception as e:
                    logger.error(f"Error upserting game {ctx.game_id}: {e}")
            else:
                if self.db:
                    logger.warning(f"Missing team names for game {ctx.game_id}: home='{new_state.home_team}', away='{new_state.away_team}'")

            old_state = ctx.last_state
            old_prob = ctx.last_home_win_prob

            # Calculate new win probability
            new_prob = self._calculate_win_prob(new_state)

            # Update context
            ctx.last_state = new_state
            ctx.last_home_win_prob = new_prob

            # Persist state to database
            if self.db:
                await self.db.insert_game_state(
                    game_id=ctx.game_id,
                    sport=ctx.sport.value,
                    home_score=new_state.home_score,
                    away_score=new_state.away_score,
                    period=new_state.period,
                    time_remaining=new_state.time_remaining,
                    status=new_state.status,
                    possession=new_state.possession,
                    home_win_prob=new_prob,
                    down=new_state.down,
                    yards_to_go=new_state.yards_to_go,
                    yard_line=new_state.yard_line,
                    is_redzone=new_state.is_redzone,
                )

            # Publish state update
            if self.redis:
                await self.redis.publish_game_state(ctx.game_id, new_state)

            # Process new plays
            for play in new_plays:
                ctx.plays_detected += 1
                await self._process_play(ctx, play, old_prob, new_prob)

            # Generate signals from win probability change
            if old_prob is not None and new_prob is not None:
                await self._generate_signals(ctx, old_state, new_state, old_prob, new_prob, new_plays)

        except Exception as e:
            logger.error(f"Error polling game state for {ctx.game_id}: {e}")

    # ==========================================================================
    # WebSocket Streaming
    # ==========================================================================

    async def _subscribe_to_ws_streams(self, ctx: GameContext) -> None:
        """Subscribe to WebSocket streams for a game's markets (all market types).

        Note: When polymarket_via_redis=True, Polymarket subscriptions are skipped
        here and handled via Redis instead (see _subscribe_to_polymarket_redis).
        """
        # Collect all market IDs to subscribe to
        kalshi_markets_to_sub = []
        poly_markets_to_sub = []

        # NEW: Subscribe to all market types
        for market_type, platforms in ctx.market_ids_by_type.items():
            if Platform.KALSHI in platforms and self.kalshi_hybrid:
                market_id = platforms[Platform.KALSHI]
                kalshi_markets_to_sub.append({
                    "market_id": market_id,
                    "title": "",
                    "game_id": ctx.game_id,
                    "market_type": market_type.value,
                })

            # Only subscribe to Polymarket WebSocket if NOT using Redis mode
            if Platform.POLYMARKET in platforms and self.polymarket_hybrid and not self.polymarket_via_redis:
                market_id = platforms[Platform.POLYMARKET]
                poly_markets_to_sub.append({
                    "condition_id": market_id,
                    "game_id": ctx.game_id,
                    "market_type": market_type.value,
                })

        # Legacy: Subscribe to single market IDs (backwards compatibility)
        if not ctx.market_ids_by_type:
            if ctx.market_ids.get(Platform.KALSHI) and self.kalshi_hybrid:
                market_id = ctx.market_ids[Platform.KALSHI]
                kalshi_markets_to_sub.append({
                    "market_id": market_id,
                    "title": "",
                    "game_id": ctx.game_id,
                })

            # Only subscribe to Polymarket WebSocket if NOT using Redis mode
            if ctx.market_ids.get(Platform.POLYMARKET) and self.polymarket_hybrid and not self.polymarket_via_redis:
                market_id = ctx.market_ids[Platform.POLYMARKET]
                poly_markets_to_sub.append({
                    "condition_id": market_id,
                    "game_id": ctx.game_id,
                })

        # Subscribe to Kalshi markets
        if kalshi_markets_to_sub and self.kalshi_hybrid:
            await self.kalshi_hybrid.subscribe_with_metadata(kalshi_markets_to_sub)
            logger.info(f"Subscribed to {len(kalshi_markets_to_sub)} Kalshi WebSocket market(s) for game {ctx.game_id}")

            # Start stream task if not running
            if Platform.KALSHI not in self._ws_stream_tasks:
                task = asyncio.create_task(self._run_ws_price_stream(Platform.KALSHI))
                self._ws_stream_tasks[Platform.KALSHI] = task

        # Subscribe to Polymarket markets
        if poly_markets_to_sub and self.polymarket_hybrid:
            for sub in poly_markets_to_sub:
                market_id = sub["condition_id"]
                # Resolve token_id if needed (Polymarket requires token_ids for WS)
                market = await self.polymarket_hybrid.get_market(market_id)
                if market:
                    token_id = await self.polymarket_hybrid.resolve_yes_token_id(market)
                    if token_id:
                        title = market.get("question", market.get("title", ""))
                        ctx.market_titles[market_id] = title
                        await self.polymarket_hybrid.subscribe_with_metadata([{
                            "token_id": token_id,
                            "condition_id": market_id,
                            "title": title,
                            "game_id": ctx.game_id,
                            "market_type": sub.get("market_type", "moneyline"),
                        }])

            logger.info(f"Subscribed to {len(poly_markets_to_sub)} Polymarket WebSocket market(s) for game {ctx.game_id}")

            # Start stream task if not running
            if Platform.POLYMARKET not in self._ws_stream_tasks:
                task = asyncio.create_task(self._run_ws_price_stream(Platform.POLYMARKET))
                self._ws_stream_tasks[Platform.POLYMARKET] = task

    async def _unsubscribe_from_ws_streams(self, ctx: GameContext) -> None:
        """Unsubscribe from WebSocket streams when removing a game."""
        # Collect all Kalshi market IDs to unsubscribe
        kalshi_ids_to_unsub = []

        # From multi-market types
        for market_type, platforms in ctx.market_ids_by_type.items():
            if Platform.KALSHI in platforms:
                kalshi_ids_to_unsub.append(platforms[Platform.KALSHI])

        # From legacy single market
        if ctx.market_ids.get(Platform.KALSHI):
            kalshi_ids_to_unsub.append(ctx.market_ids[Platform.KALSHI])

        # Unsubscribe from Kalshi
        if kalshi_ids_to_unsub and self.kalshi_hybrid:
            await self.kalshi_hybrid.unsubscribe(kalshi_ids_to_unsub)
            logger.debug(f"Unsubscribed from {len(kalshi_ids_to_unsub)} Kalshi WebSocket market(s) for game {ctx.game_id}")

        # Note: Polymarket doesn't have explicit unsubscribe

    # ==========================================================================
    # Polymarket via Redis (VPN Monitor)
    # ==========================================================================

    async def _subscribe_to_polymarket_redis(self, ctx: GameContext) -> None:
        """Subscribe to Polymarket prices from Redis (published by VPN monitor).

        When POLYMARKET_VIA_REDIS=true, the shard doesn't connect to Polymarket
        directly. Instead, the polymarket_monitor service (running behind VPN)
        publishes prices to Redis on game:{game_id}:price channels.
        """
        if not self.redis:
            return

        # Subscribe to the game's price channel
        price_channel = Channel.GAME_PRICE.format(game_id=ctx.game_id)

        async def handle_polymarket_price(data: dict) -> None:
            """Handle incoming Polymarket price from Redis."""
            await self._handle_redis_polymarket_price(ctx, data)

        await self.redis.subscribe(price_channel, handle_polymarket_price)
        logger.info(f"Subscribed to Redis price channel for Polymarket: {price_channel}")

    async def _handle_redis_polymarket_price(self, ctx: GameContext, data: dict) -> None:
        """Handle Polymarket price update received from Redis (VPN monitor).

        The polymarket_monitor publishes MarketPrice objects to game:{game_id}:price.
        We filter for platform=polymarket and update the game context accordingly.
        
        IMPORTANT: For moneyline markets, we receive TWO prices (one per team).
        The contract_team field indicates which team this YES contract is for.
        """
        # Check if this is a Polymarket price
        platform_str = data.get("platform")
        if platform_str != Platform.POLYMARKET.value:
            return  # Ignore non-Polymarket prices (e.g., Kalshi publishes here too)

        # Extract contract_team (which team this YES contract is for)
        contract_team = data.get("contract_team")

        # Reconstruct MarketPrice from dict
        try:
            price = MarketPrice(
                market_id=data.get("market_id", ""),
                platform=Platform.POLYMARKET,
                game_id=data.get("game_id"),
                market_title=data.get("market_title", ""),
                contract_team=contract_team,
                yes_bid=float(data.get("yes_bid", 0)),
                yes_ask=float(data.get("yes_ask", 1)),
                yes_bid_size=float(data.get("yes_bid_size", 0) or 0),
                yes_ask_size=float(data.get("yes_ask_size", 0) or 0),
                volume=float(data.get("volume", 0)),
                liquidity=float(data.get("liquidity", 0)),
                timestamp=datetime.fromisoformat(data["timestamp"]) if "timestamp" in data else datetime.utcnow(),
            )
        except (KeyError, ValueError, TypeError) as e:
            logger.warning(f"Failed to parse Polymarket price from Redis: {e}")
            return

        # Store by team if contract_team is specified
        if contract_team:
            ctx.market_prices_by_team[(Platform.POLYMARKET, contract_team)] = price
            logger.debug(f"Stored Polymarket price for team '{contract_team}': bid={price.yes_bid:.3f} ask={price.yes_ask:.3f}")
            
            # Also determine if this is the HOME team's contract for legacy compatibility
            # If so, use this as the "main" Polymarket price
            if ctx.last_state:
                home_team = ctx.last_state.home_team or ""
                if self._teams_match(contract_team, home_team):
                    ctx.market_prices[Platform.POLYMARKET] = price
        else:
            # No contract_team specified - use as generic price (legacy)
            ctx.market_prices[Platform.POLYMARKET] = price

        # Determine market type and update by-type mapping
        market_type = self._determine_market_type(ctx, price.market_id, Platform.POLYMARKET)
        if market_type:
            ctx.market_prices_by_type[(market_type, Platform.POLYMARKET)] = price

        # Store market title for parsing
        if price.market_title:
            ctx.market_titles[price.market_id] = price.market_title

        # Persist to database
        if self.db:
            await self.db.insert_market_price(
                market_id=price.market_id,
                platform=Platform.POLYMARKET.value,
                yes_bid=price.yes_bid,
                yes_ask=price.yes_ask,
                yes_bid_size=price.yes_bid_size,
                yes_ask_size=price.yes_ask_size,
                volume=price.volume,
                liquidity=price.liquidity,
                game_id=ctx.game_id,
                market_title=price.market_title,
                market_type=data.get("market_type", "moneyline"),
                contract_team=contract_team,  # Which team's YES contract
            )

        # Check for signal generation based on market price change
        await self._check_market_signals(ctx, Platform.POLYMARKET, price)

        # Check for cross-market arbitrage opportunities
        await self._check_cross_market_arbitrage(ctx)

        logger.debug(
            f"[Redis] Polymarket price for {ctx.game_id}: "
            f"bid={price.yes_bid:.3f} ask={price.yes_ask:.3f}"
        )

    async def _run_ws_price_stream(self, platform: Platform) -> None:
        """Background task to process WebSocket price updates.

        This task runs continuously and routes price updates to the appropriate
        game context for signal generation.
        """
        logger.info(f"Starting WebSocket price stream for {platform.value}")

        while self._running:
            try:
                if platform == Platform.KALSHI and self.kalshi_hybrid:
                    # Wait for markets to be subscribed
                    market_ids = list(self.kalshi_hybrid.subscribed_markets)
                    if not market_ids:
                        logger.debug("No Kalshi markets subscribed yet, waiting...")
                        await asyncio.sleep(2)
                        continue

                    logger.info(f"Streaming prices for {len(market_ids)} Kalshi markets")
                    async for price in self.kalshi_hybrid.stream_prices(market_ids):
                        if not self._running:
                            break
                        await self._handle_ws_price_update(platform, price)

                elif platform == Platform.POLYMARKET and self.polymarket_hybrid:
                    # Wait for token_ids to be subscribed
                    token_ids = list(self.polymarket_hybrid.subscribed_markets)
                    if not token_ids:
                        logger.debug("No Polymarket markets subscribed yet, waiting...")
                        await asyncio.sleep(2)
                        continue

                    logger.info(f"Streaming prices for {len(token_ids)} Polymarket markets")
                    async for price in self.polymarket_hybrid.stream_prices(token_ids):
                        if not self._running:
                            break
                        await self._handle_ws_price_update(platform, price)

            except asyncio.CancelledError:
                logger.info(f"WebSocket stream cancelled for {platform.value}")
                break
            except Exception as e:
                logger.error(f"WebSocket stream error for {platform.value}: {e}")
                # Wait before retrying
                await asyncio.sleep(5)

    async def _handle_ws_price_update(self, platform: Platform, price: MarketPrice) -> None:
        """Handle incoming WebSocket price update.

        Routes the price update to the appropriate game context and
        triggers signal generation if conditions are met.
        """
        # Find the game associated with this market
        game_id = self._market_to_game.get(price.market_id)
        if not game_id:
            # Try to find by game_id in the price object
            game_id = price.game_id

        if not game_id or game_id not in self._games:
            return

        ctx = self._games[game_id]

        # Update context with new price (legacy)
        ctx.market_prices[platform] = price

        # NEW: Store by team if contract_team is available
        # This enables proper team matching in signal generation
        if price.contract_team:
            ctx.market_prices_by_team[(platform, price.contract_team)] = price
            logger.debug(
                f"[WS] Stored {platform.value} price for team '{price.contract_team}': "
                f"mid={price.mid_price:.3f}"
            )

        # NEW: Store by market type if we can determine it
        market_type = self._determine_market_type(ctx, price.market_id, platform)
        if market_type:
            ctx.market_prices_by_type[(market_type, platform)] = price

        # Store market title for parsing
        if price.market_title:
            ctx.market_titles[price.market_id] = price.market_title

        # Persist to database (include contract_team for team-aware queries)
        if self.db:
            await self.db.insert_market_price(
                market_id=price.market_id,
                platform=platform.value,
                yes_bid=price.yes_bid,
                yes_ask=price.yes_ask,
                yes_bid_size=price.yes_bid_size,
                yes_ask_size=price.yes_ask_size,
                volume=price.volume,
                liquidity=price.liquidity,
                game_id=game_id,
                market_title=price.market_title,
                contract_team=price.contract_team,  # Track which team's contract
            )

        # Publish to Redis
        if self.redis:
            await self.redis.publish_market_price(game_id, price)

        # Check for signal generation based on market price change
        await self._check_market_signals(ctx, platform, price)

        # NEW: Check for cross-market arbitrage opportunities
        await self._check_cross_market_arbitrage(ctx)

    def _determine_market_type(
        self,
        ctx: GameContext,
        market_id: str,
        platform: Platform,
    ) -> Optional[MarketType]:
        """Determine the market type for a given market ID."""
        # Check multi-market type mappings
        for market_type, platforms in ctx.market_ids_by_type.items():
            if platforms.get(platform) == market_id:
                return market_type

        # Legacy: assume moneyline for single market
        if ctx.market_ids.get(platform) == market_id:
            return MarketType.MONEYLINE

        return None

    def _teams_match(self, team1: str, team2: str) -> bool:
        """Check if two team names refer to the same team.
        
        Handles variations like:
        - "Boston Celtics" vs "Celtics"
        - "BOS" vs "Boston Celtics"
        - Case insensitive
        """
        if not team1 or not team2:
            return False
            
        t1 = team1.lower().strip()
        t2 = team2.lower().strip()
        
        # Exact match
        if t1 == t2:
            return True
            
        # One contains the other
        if t1 in t2 or t2 in t1:
            return True
            
        # Check last word (team nickname)
        t1_words = t1.split()
        t2_words = t2.split()
        if t1_words and t2_words:
            if t1_words[-1] == t2_words[-1]:
                return True
                
        return False

    async def _check_market_signals(
        self,
        ctx: GameContext,
        platform: Platform,
        price: MarketPrice,
    ) -> None:
        """Check if market price change warrants signal generation.

        This enables faster signal generation based on market price changes
        detected via WebSocket, complementing game-state-based signals.

        IMPORTANT: Must properly match team probabilities to avoid false edges.
        The price.contract_team tells us which team's YES contract this is.
        ctx.last_home_win_prob is always for the HOME team.
        """
        if ctx.last_home_win_prob is None or not ctx.last_state:
            return

        # Check trading cooldown
        if ctx.cooldown_until and datetime.utcnow() < ctx.cooldown_until:
             return

        # Determine which team this price is for
        contract_team = price.contract_team
        if not contract_team:
            # Can't determine team - skip to avoid mismatched comparison
            logger.debug(f"Skipping market signal check: unknown contract_team for {price.market_id}")
            return

        # Determine if this is home or away team's contract
        home_team = ctx.last_state.home_team
        is_home_contract = self._teams_match(contract_team, home_team)

        # Get the correct model probability for this team
        # last_home_win_prob is HOME team's probability
        if is_home_contract:
            model_prob = ctx.last_home_win_prob
            target_team = home_team
        else:
            model_prob = 1.0 - ctx.last_home_win_prob  # Away team prob
            target_team = ctx.last_state.away_team

        # Use executable price for edge to avoid overstating edge due to spread.
        buy_edge = (model_prob - price.yes_ask) * 100.0   # if we BUY, we pay ask
        sell_edge = (price.yes_bid - model_prob) * 100.0  # if we SELL/NO, we receive bid

        # Choose direction based on which executable edge is positive
        if buy_edge >= 0:
            direction = SignalDirection.BUY
            market_prob = price.yes_ask
            edge = buy_edge
        else:
            direction = SignalDirection.SELL
            market_prob = price.yes_bid
            edge = sell_edge

        # Log for debugging
        logger.debug(
            f"[{ctx.game_id}] Market signal check: "
            f"contract_team='{contract_team}' ({'HOME' if is_home_contract else 'AWAY'}), "
            f"model_prob={model_prob:.3f}, market_prob={market_prob:.3f}, edge={edge:.1f}%"
        )

        # Only signal on significant edge (> 5%) - increased for uncalibrated model
        if abs(edge) < 5.0:
            return

        # Additional friction check
        spread = (price.yes_ask - price.yes_bid) * 100.0
        required_edge = 2.0 + (spread / 2.0) + 1.0  # fees + half spread + margin

        if abs(edge) < required_edge:
            return

        # Hysteresis check
        if ctx.active_signal and ctx.active_signal.direction != direction:
            if abs(edge) < (required_edge * 2.0):
                return

        # Generate signal - use target_team (the team we're betting on)
        signal = TradingSignal(
            signal_id=str(uuid.uuid4()),
            signal_type=SignalType.MARKET_MISPRICING,
            game_id=ctx.game_id,
            sport=ctx.sport,
            team=target_team,  # Team this price/probability is for
            direction=direction,
            model_prob=model_prob,  # Probability for the TARGET team
            market_prob=market_prob,
            edge_pct=abs(edge),
            confidence=min(1.0, abs(edge) / 10.0),
            reason=f"Market mispricing detected via WebSocket (model: {model_prob*100:.1f}%, market: {market_prob*100:.1f}%, team: {target_team})",
        )

        ctx.signals_generated += 1
        ctx.active_signal = signal

        # Persist signal
        if self.db:
            await self.db.insert_trading_signal(
                signal_id=signal.signal_id,
                signal_type=signal.signal_type.value,
                direction=signal.direction.value,
                edge_pct=signal.edge_pct,
                game_id=signal.game_id,
                sport=signal.sport.value if signal.sport else None,
                team=signal.team,
                model_prob=signal.model_prob,
                market_prob=signal.market_prob,
                confidence=signal.confidence,
                reason=signal.reason,
            )

        # Log signal with shard_id for race condition visibility
        trace_log(
            service="game_shard",
            event="signal_generated",
            signal_id=signal.signal_id,
            game_id=signal.game_id,
            shard_id=self.shard_id,
            sport=signal.sport.value if signal.sport else None,
            team=signal.team,
            direction=signal.direction.value,
            signal_type=signal.signal_type.value,
            model_prob=signal.model_prob,
            market_prob=signal.market_prob,
            edge_pct=signal.edge_pct,
            source="websocket_market_mispricing",
        )

        # Publish signal
        if self.redis:
            await self.redis.publish_signal(signal)

        logger.info(
            f"[WS][shard={self.shard_id}] Generated signal: {signal.direction.value} {signal.team} "
            f"(edge: {signal.edge_pct:.1f}%, platform: {platform.value})"
        )

    async def _check_cross_market_arbitrage(self, ctx: GameContext) -> None:
        """
        Check for cross-market arbitrage opportunities across all market types.

        Uses SIMD-accelerated arbitrage detection from Rust core (terauss integration)
        for sub-microsecond detection across all market types.

        This is where the 3-8x opportunity multiplier comes from:
        - Compare SAME market type across platforms (Kalshi vs Polymarket)
        - Only compare compatible markets (verified via parser)
        - Generate arbitrage signals when pricing discrepancy exists
        """
        # Check circuit breaker first
        if not self.circuit_breaker.is_trading_allowed():
            return
            
        # Check trading cooldown
        if ctx.cooldown_until and datetime.utcnow() < ctx.cooldown_until:
            return

        for market_type, platforms in ctx.market_ids_by_type.items():
            # Need both platforms for arbitrage
            if Platform.KALSHI not in platforms or Platform.POLYMARKET not in platforms:
                continue

            kalshi_id = platforms[Platform.KALSHI]
            poly_id = platforms[Platform.POLYMARKET]

            # Get prices for both platforms
            kalshi_price = ctx.market_prices_by_type.get((market_type, Platform.KALSHI))
            poly_price = ctx.market_prices_by_type.get((market_type, Platform.POLYMARKET))

            if not kalshi_price or not poly_price:
                continue

            # Parse market titles to verify compatibility
            kalshi_title = ctx.market_titles.get(kalshi_id, "")
            poly_title = ctx.market_titles.get(poly_id, "")

            if kalshi_title and poly_title:
                kalshi_parsed = parse_market(kalshi_title, platform="kalshi")
                poly_parsed = parse_market(poly_title, platform="polymarket")

                if kalshi_parsed and poly_parsed:
                    if not kalshi_parsed.is_compatible_with(poly_parsed):
                        logger.warning(
                            f"Market incompatibility detected for {market_type.value}: "
                            f"Kalshi='{kalshi_title}' vs Poly='{poly_title}'"
                        )
                        continue

            # NEW: SIMD-accelerated arbitrage detection (terauss integration)
            # Convert prices to cents (0-100 scale)
            kalshi_yes = int(kalshi_price.yes_ask * 100)
            kalshi_no = int((1.0 - kalshi_price.yes_bid) * 100)  # NO ask = 1 - YES bid
            poly_yes = int(poly_price.yes_ask * 100)
            poly_no = int((1.0 - poly_price.yes_bid) * 100)

            # SIMD check for all arb types in parallel
            # Returns bitmask: bit 0 = PolyYes+KalshiNo, bit 1 = KalshiYes+PolyNo
            arb_mask = arbees_core.simd_check_arbs(
                kalshi_yes, kalshi_no, poly_yes, poly_no, 100
            )

            if arb_mask == 0:
                continue  # No arbitrage detected

            # Decode the arb types detected
            arb_types = arbees_core.simd_decode_mask(arb_mask)
            logger.debug(f"SIMD detected arb types: {arb_types} for {market_type.value}")

            # Process each detected arb type
            # Note: arb_type values are bit flags: 1=PolyYes+KalshiNo, 2=KalshiYes+PolyNo
            if arb_mask & 1:  # PolyYes + KalshiNo
                profit_cents = arbees_core.simd_calculate_profit(
                    kalshi_yes, kalshi_no, poly_yes, poly_no, 1  # arb_type=1 (bit flag)
                )
                if profit_cents > 0:
                    await self._emit_arbitrage_signal(
                        ctx, market_type,
                        buy_platform=Platform.POLYMARKET,
                        sell_platform=Platform.KALSHI,
                        buy_price=poly_price.yes_ask,
                        sell_price=1.0 - kalshi_price.yes_bid,  # NO price
                        edge_pct=float(profit_cents),  # Already in cents, use as percentage
                        arb_type="PolyYes+KalshiNo",
                    )

            if arb_mask & 2:  # KalshiYes + PolyNo
                profit_cents = arbees_core.simd_calculate_profit(
                    kalshi_yes, kalshi_no, poly_yes, poly_no, 2  # arb_type=2 (bit flag)
                )
                if profit_cents > 0:
                    await self._emit_arbitrage_signal(
                        ctx, market_type,
                        buy_platform=Platform.KALSHI,
                        sell_platform=Platform.POLYMARKET,
                        buy_price=kalshi_price.yes_ask,
                        sell_price=1.0 - poly_price.yes_bid,  # NO price
                        edge_pct=float(profit_cents),
                        arb_type="KalshiYes+PolyNo",
                    )

    async def _emit_arbitrage_signal(
        self,
        ctx: GameContext,
        market_type: MarketType,
        buy_platform: Platform,
        sell_platform: Platform,
        buy_price: float,
        sell_price: float,
        edge_pct: float,
        arb_type: Optional[str] = None,
    ) -> None:
        """Emit a cross-market arbitrage signal.

        Args:
            ctx: Game context
            market_type: Type of market (moneyline, spread, etc.)
            buy_platform: Platform to buy on
            sell_platform: Platform to sell on
            buy_price: Price to buy at (0-1 scale)
            sell_price: Price to sell at (0-1 scale)
            edge_pct: Expected edge percentage
            arb_type: Optional SIMD arb type string (e.g., "PolyYes+KalshiNo")
        """
        # Check circuit breaker before emitting
        if not self.circuit_breaker.is_trading_allowed():
            logger.warning(f"Circuit breaker halted - skipping arb signal for {ctx.game_id}")
            return

        arb_type_str = f" ({arb_type})" if arb_type else ""
        signal = TradingSignal(
            signal_id=str(uuid.uuid4()),
            signal_type=SignalType.CROSS_MARKET_ARB,
            game_id=ctx.game_id,
            sport=ctx.sport,
            team=ctx.last_state.home_team if ctx.last_state else None,
            direction=SignalDirection.BUY,  # Buy on buy_platform
            model_prob=sell_price,  # Use sell price as "model" expectation
            market_prob=buy_price,  # Buy price is what we pay
            edge_pct=edge_pct,
            confidence=min(1.0, edge_pct / 5.0),  # Higher edge = higher confidence
            reason=(
                f"Cross-market arb{arb_type_str} on {market_type.value}: "
                f"BUY {buy_platform.value} @ {buy_price*100:.1f}¢, "
                f"SELL {sell_platform.value} @ {sell_price*100:.1f}¢"
            ),
        )

        ctx.signals_generated += 1

        # Persist signal
        if self.db:
            await self.db.insert_trading_signal(
                signal_id=signal.signal_id,
                signal_type=signal.signal_type.value,
                direction=signal.direction.value,
                edge_pct=signal.edge_pct,
                game_id=signal.game_id,
                sport=signal.sport.value if signal.sport else None,
                team=signal.team,
                model_prob=signal.model_prob,
                market_prob=signal.market_prob,
                confidence=signal.confidence,
                reason=signal.reason,
            )

        # Log signal with shard_id for race condition visibility
        trace_log(
            service="game_shard",
            event="signal_generated",
            signal_id=signal.signal_id,
            game_id=signal.game_id,
            shard_id=self.shard_id,
            sport=signal.sport.value if signal.sport else None,
            team=signal.team,
            direction=signal.direction.value,
            signal_type=signal.signal_type.value,
            model_prob=signal.model_prob,
            market_prob=signal.market_prob,
            edge_pct=signal.edge_pct,
            source="cross_market_arb",
            arb_type=arb_type,
            buy_platform=buy_platform.value,
            sell_platform=sell_platform.value,
        )

        # Publish signal
        if self.redis:
            await self.redis.publish_signal(signal)

        logger.info(
            f"[ARB][shard={self.shard_id}] {market_type.value}: BUY {buy_platform.value}@{buy_price*100:.1f}¢ "
            f"SELL {sell_platform.value}@{sell_price*100:.1f}¢ (edge: {edge_pct:.1f}%)"
        )

    async def _poll_market_prices(self, ctx: GameContext) -> None:
        """Poll market prices for the game.

        Note: When WebSocket streaming is enabled, this serves as a fallback
        for markets not subscribed via WebSocket.
        """
        # Skip if no market IDs configured (check both legacy and multi-market)
        if not ctx.market_ids and not ctx.market_ids_by_type:
            return

        # Check if we have recent market data (within 30 seconds)
        # If WS is enabled but not delivering data, we need REST fallback
        now = datetime.utcnow()
        stale_threshold = 30.0  # seconds

        def is_market_data_fresh(platform: Platform, market_type: Optional[MarketType] = None) -> bool:
            if market_type:
                price = ctx.market_prices_by_type.get((market_type, platform))
            else:
                price = ctx.market_prices.get(platform)
            if not price:
                return False
            age = (now - price.timestamp).total_seconds()
            return age < stale_threshold

        try:
            # NEW: Poll all market types
            for market_type, platforms in ctx.market_ids_by_type.items():
                # Poll Kalshi for this market type
                if Platform.KALSHI in platforms and self.kalshi:
                    market_id = platforms[Platform.KALSHI]
                    if not self.use_websocket_streaming or not is_market_data_fresh(Platform.KALSHI, market_type):
                        price = await self.kalshi.get_market_price(market_id)
                        if price:
                            ctx.market_prices_by_type[(market_type, Platform.KALSHI)] = price
                            if price.market_title:
                                ctx.market_titles[market_id] = price.market_title

                            if self.db:
                                await self.db.insert_market_price(
                                    market_id=market_id,
                                    platform="kalshi",
                                    yes_bid=price.yes_bid,
                                    yes_ask=price.yes_ask,
                                    yes_bid_size=price.yes_bid_size,
                                    yes_ask_size=price.yes_ask_size,
                                    volume=price.volume,
                                    liquidity=price.liquidity,
                                    game_id=ctx.game_id,
                                    market_title=price.market_title,
                                )

                            if self.redis:
                                await self.redis.publish_market_price(ctx.game_id, price)

                # Poll Polymarket for this market type
                if Platform.POLYMARKET in platforms and self.polymarket:
                    market_id = platforms[Platform.POLYMARKET]
                    if not self.use_websocket_streaming or not is_market_data_fresh(Platform.POLYMARKET, market_type):
                        price = await self.polymarket.get_market_price(market_id)
                        if price:
                            ctx.market_prices_by_type[(market_type, Platform.POLYMARKET)] = price
                            if price.market_title:
                                ctx.market_titles[market_id] = price.market_title

                            if self.db:
                                await self.db.insert_market_price(
                                    market_id=market_id,
                                    platform="polymarket",
                                    yes_bid=price.yes_bid,
                                    yes_ask=price.yes_ask,
                                    yes_bid_size=price.yes_bid_size,
                                    yes_ask_size=price.yes_ask_size,
                                    volume=price.volume,
                                    liquidity=price.liquidity,
                                    game_id=ctx.game_id,
                                    market_title=price.market_title,
                                    contract_team=price.contract_team,
                                )

                            if self.redis:
                                await self.redis.publish_market_price(ctx.game_id, price)

            # Legacy: Poll single market IDs (backwards compatibility)
            if not ctx.market_ids_by_type:
                # Only skip polling if we have fresh WS data
                if self.use_websocket_streaming:
                    kalshi_fresh = is_market_data_fresh(Platform.KALSHI)
                    poly_fresh = is_market_data_fresh(Platform.POLYMARKET)

                    kalshi_needed = Platform.KALSHI in ctx.market_ids and not kalshi_fresh
                    poly_needed = Platform.POLYMARKET in ctx.market_ids and not poly_fresh

                    if not kalshi_needed and not poly_needed:
                        return  # All data is fresh, no need to poll

                    if kalshi_needed or poly_needed:
                        logger.info(f"REST fallback for {ctx.game_id}: kalshi_needed={kalshi_needed}, poly_needed={poly_needed}")

                # Poll Kalshi
                should_poll_kalshi = ctx.market_ids.get(Platform.KALSHI) and self.kalshi
                if self.use_websocket_streaming:
                    should_poll_kalshi = should_poll_kalshi and not is_market_data_fresh(Platform.KALSHI)

                if should_poll_kalshi:
                    market_id = ctx.market_ids[Platform.KALSHI]
                    logger.info(f"Polling Kalshi REST for {ctx.game_id} market {market_id}")
                    price = await self.kalshi.get_market_price(market_id)
                    if price:
                        logger.info(f"Got Kalshi price: bid={price.yes_bid:.3f}, ask={price.yes_ask:.3f}, mid={price.mid_price:.3f}")
                        ctx.market_prices[Platform.KALSHI] = price

                        if self.db:
                            await self.db.insert_market_price(
                                market_id=market_id,
                                platform="kalshi",
                                yes_bid=price.yes_bid,
                                yes_ask=price.yes_ask,
                                yes_bid_size=price.yes_bid_size,
                                yes_ask_size=price.yes_ask_size,
                                volume=price.volume,
                                liquidity=price.liquidity,
                                game_id=ctx.game_id,
                                market_title=price.market_title,
                            )

                        if self.redis:
                            await self.redis.publish_market_price(ctx.game_id, price)

                # Poll Polymarket
                should_poll_poly = ctx.market_ids.get(Platform.POLYMARKET) and self.polymarket
                if self.use_websocket_streaming:
                    should_poll_poly = should_poll_poly and not is_market_data_fresh(Platform.POLYMARKET)

                if should_poll_poly:
                    market_id = ctx.market_ids[Platform.POLYMARKET]
                    price = await self.polymarket.get_market_price(market_id)
                    if price:
                        ctx.market_prices[Platform.POLYMARKET] = price

                        if self.db:
                            await self.db.insert_market_price(
                                market_id=market_id,
                                platform="polymarket",
                                yes_bid=price.yes_bid,
                                yes_ask=price.yes_ask,
                                yes_bid_size=price.yes_bid_size,
                                yes_ask_size=price.yes_ask_size,
                                volume=price.volume,
                                liquidity=price.liquidity,
                                game_id=ctx.game_id,
                                market_title=price.market_title,
                                contract_team=price.contract_team,
                            )

                        if self.redis:
                            await self.redis.publish_market_price(ctx.game_id, price)

            # Check for cross-market arbitrage opportunities
            await self._check_cross_market_arbitrage(ctx)

        except Exception as e:
            logger.error(f"Error polling market prices for {ctx.game_id}: {e}")

    async def _process_play(
        self,
        ctx: GameContext,
        play: Play,
        old_prob: Optional[float],
        new_prob: Optional[float],
    ) -> None:
        """Process a new play."""
        # Persist play to database
        if self.db:
            await self.db.insert_play(
                play_id=play.play_id,
                game_id=ctx.game_id,
                sport=ctx.sport.value,
                play_type=play.play_type.value,
                description=play.description,
                sequence_number=play.sequence_number,
                home_score=play.home_score,
                away_score=play.away_score,
                period=play.period,
                time_remaining=play.time_remaining,
                home_win_prob_before=old_prob,
                home_win_prob_after=new_prob,
                team=play.team,
                player=play.player,
                yards_gained=play.yards_gained,
                yard_line=play.yard_line,
                down=play.down,
                yards_to_go=play.yards_to_go,
                is_scoring=play.is_scoring,
                is_turnover=play.is_turnover,
            )

        # Publish play to Redis
        if self.redis:
            await self.redis.publish_play(ctx.game_id, play)

    async def _generate_signals(
        self,
        ctx: GameContext,
        old_state: Optional[GameState],
        new_state: GameState,
        old_prob: float,
        new_prob: float,
        new_plays: list[Play],
    ) -> None:
        """Generate trading signals from game updates."""
        prob_change = new_prob - old_prob

        # Only signal on significant changes (> 3% for probability shifts)
        if abs(prob_change) < 0.02:
            return

        # 1. Clamp probabilities to avoid extreme confidence
        # Capping at 95% prevents div-by-zero edges while allowing model to express strong favorites
        capped_new_prob = max(0.05, min(0.95, new_prob))
        
        # 2. Estimate Fees/Friction (2% round trip estimate)
        estimated_fees = 2.0 

        # Determine which team we're betting on based on probability change
        # prob_change > 0 means HOME team's probability increased → bet on HOME
        # prob_change < 0 means HOME team's probability decreased → bet on AWAY
        target_team = new_state.home_team if prob_change > 0 else new_state.away_team
        is_home_team_bet = prob_change > 0

        # Get market price for the TARGET TEAM's contract
        # For Polymarket: look up by team name
        # For Kalshi: typically home team contract, so we may need to invert
        market_price = None
        
        # Try Polymarket team-specific price first
        if target_team:
            for (platform, team_name), price in ctx.market_prices_by_team.items():
                if platform == Platform.POLYMARKET and self._teams_match(team_name, target_team):
                    market_price = price
                    logger.debug(f"Using Polymarket price for team '{team_name}': bid={price.yes_bid:.3f}")
                    break
        
        # Fallback to legacy prices
        if not market_price:
            market_price = ctx.market_prices.get(Platform.KALSHI) or ctx.market_prices.get(Platform.POLYMARKET)
            # If we have a Polymarket price, check if it needs inversion
            # Legacy prices (from REST fallback) may not have contract_team set
            # In that case, assume it's the HOME team's price (Polymarket default)
            if market_price and market_price.platform == Platform.POLYMARKET:
                contract_team = market_price.contract_team

                # If contract_team is None, assume HOME team (common pattern for legacy prices)
                if not contract_team and ctx.last_state:
                    contract_team = ctx.last_state.home_team
                    logger.debug(f"[{ctx.game_id}] Legacy price has no contract_team, assuming HOME team: {contract_team}")

                if contract_team and target_team and not self._teams_match(contract_team, target_team):
                    # This price is for the OTHER team - INVERT IT
                    # If Team A has 70% YES, Team B has ~30% YES
                    inverted_mid = 1.0 - market_price.mid_price
                    logger.info(
                        f"[{ctx.game_id}] Inverting Polymarket price: "
                        f"contract='{contract_team}' (mid={market_price.mid_price:.3f}), "
                        f"target='{target_team}' -> inverted_mid={inverted_mid:.3f}"
                    )
                    # Create a modified price with inverted values
                    market_price = MarketPrice(
                        market_id=market_price.market_id,
                        platform=market_price.platform,
                        market_title=f"{target_team} (inverted from {contract_team})",
                        contract_team=target_team,
                        yes_bid=1.0 - market_price.yes_ask,  # Invert bid/ask
                        yes_ask=1.0 - market_price.yes_bid,
                        volume=market_price.volume,
                        liquidity=market_price.liquidity,
                        timestamp=market_price.timestamp,
                    )

        if market_price is not None:
            # Check data freshness synchronization
            # We want game state and market price to be from roughly the same time window
            now = datetime.utcnow()
            
            # 1. Market Price Age
            market_age = (now - market_price.timestamp).total_seconds()
            if market_age > self.market_data_ttl:
                 # Market data is too old
                 logger.warning(f"Skipping signal for {ctx.game_id}: Market price stale ({market_age:.1f}s > {self.market_data_ttl}s)")
                 return

            # 2. Game State Age
            game_age = (now - new_state.updated_at).total_seconds()
            
             # 3. Synchronization Delta
            sync_delta = abs((new_state.updated_at - market_price.timestamp).total_seconds())
            
            if sync_delta > self.sync_delta_tolerance:
                 # Data is de-synced (e.g. fast game update vs slow market poll)
                 # We still signal but with reduced confidence or logging
                 logger.info(f"Signal sync warning for {ctx.game_id}: Game/Market delta {sync_delta:.1f}s > {self.sync_delta_tolerance}s")
            
            # With market data: calculate edge vs market using EXECUTABLE prices (bid/ask),
            # not mid, otherwise we systematically overstate edge (and lose to spread).
            model_prob_for_target = capped_new_prob if is_home_team_bet else (1.0 - capped_new_prob)

            buy_edge = (model_prob_for_target - market_price.yes_ask) * 100.0   # pay ask
            sell_edge = (market_price.yes_bid - model_prob_for_target) * 100.0  # receive bid

            # Pick the best executable action
            if buy_edge >= sell_edge:
                direction = SignalDirection.BUY
                market_prob = market_price.yes_ask
                edge = buy_edge
            else:
                direction = SignalDirection.SELL
                market_prob = market_price.yes_bid
                edge = sell_edge

            # Log the team-aware calculation
            logger.info(
                f"[{ctx.game_id}] Signal calculation: "
                f"target_team='{target_team}' ({'HOME' if is_home_team_bet else 'AWAY'}), "
                f"model_prob={model_prob_for_target:.3f}, "
                f"market_prob={market_prob:.3f} (contract_team='{market_price.contract_team}'), "
                f"raw_edge={edge:.1f}%"
            )

            # Fee-adjusted edge
            spread = (market_price.yes_ask - market_price.yes_bid) * 100.0
            required_edge = estimated_fees + (spread / 2.0) + 1.0 # 1% extra margin
            
            # Only signal if edge exceeds friction
            if edge < required_edge:
                return
        else:
            # Without market data: skip signal generation
            # We require real market prices to validate edge and execute trades
            logger.debug(f"Skipping signal for {ctx.game_id}: no market price available")
            return

        # `direction` already chosen based on executable edge.

        # 3. Hysteresis (Flip-Flop Protection)
        # If we have an active signal in the OPPOSITE direction, we need much stronger evidence to flip
        if ctx.active_signal and ctx.active_signal.direction != direction:
             # If flipping, require double the normal edge to justify exit cost + entry cost
             if abs(edge) < (required_edge * 2.0):
                 logger.debug(f"Ignoring flip signal {direction} for game {ctx.game_id}: edge {edge:.1f}% < required {required_edge*2:.1f}%")
                 return

        # Create signal
        # IMPORTANT: model_prob should be the probability for the TARGET TEAM (signal.team),
        # not always home team probability. This ensures edge calculation is consistent.
        signal = TradingSignal(
            signal_id=str(uuid.uuid4()),
            signal_type=SignalType.WIN_PROB_SHIFT,
            game_id=ctx.game_id,
            sport=ctx.sport,
            team=target_team,  # Use target_team (home if prob_change > 0, away otherwise)
            direction=direction,
            model_prob=model_prob_for_target,  # Probability for TARGET team, not always home
            market_prob=market_prob,
            edge_pct=float(edge),
            confidence=min(1.0, abs(prob_change) * 10),
            reason=f"Win prob changed {prob_change*100:.1f}% ({old_prob*100:.1f}% → {new_prob*100:.1f}%)",
            play_id=new_plays[-1].play_id if new_plays else None,
        )

        ctx.signals_generated += 1
        ctx.active_signal = signal  # Update state

        # Persist signal
        if self.db:
            await self.db.insert_trading_signal(
                signal_id=signal.signal_id,
                signal_type=signal.signal_type.value,
                direction=signal.direction.value,
                edge_pct=signal.edge_pct,
                game_id=signal.game_id,
                sport=signal.sport.value if signal.sport else None,
                team=signal.team,
                model_prob=signal.model_prob,
                market_prob=signal.market_prob,
                confidence=signal.confidence,
                reason=signal.reason,
                play_id=signal.play_id,
            )

        # Log signal with shard_id for race condition visibility
        trace_log(
            service="game_shard",
            event="signal_generated",
            signal_id=signal.signal_id,
            game_id=signal.game_id,
            shard_id=self.shard_id,
            sport=signal.sport.value if signal.sport else None,
            team=signal.team,
            direction=signal.direction.value,
            signal_type=signal.signal_type.value,
            model_prob=signal.model_prob,
            market_prob=signal.market_prob,
            edge_pct=signal.edge_pct,
            source="win_prob_shift",
            prob_change=prob_change,
            contract_team=market_price.contract_team if market_price else None,
        )

        # Publish signal
        if self.redis:
            await self.redis.publish_signal(signal)

        logger.info(
            f"[{ctx.game_id}][shard={self.shard_id}] SIGNAL: {signal.direction.value} {signal.team} "
            f"(model={model_prob_for_target:.1%}, market={market_prob:.1%}, edge={signal.edge_pct:.1f}%, "
            f"contract_team='{market_price.contract_team}')"
        )

    def _calculate_win_prob(self, state: GameState) -> float:
        """
        Calculate win probability from game state using Rust core.
        """
        try:
            # Map Python Sport to Rust Sport
            # Note: Assuming binary compatibility or string mapping
            rust_sport = getattr(arbees_core.Sport, state.sport.value.upper(), None)
            if not rust_sport:
                # Fallback mapping if direct attribute fails
                sport_map = {
                    "nfl": arbees_core.Sport.NFL,
                    "nba": arbees_core.Sport.NBA,
                    "nhl": arbees_core.Sport.NHL,
                    "mlb": arbees_core.Sport.MLB,
                    "ncaaf": arbees_core.Sport.NCAAF,
                    "ncaab": arbees_core.Sport.NCAAB,
                    "mls": arbees_core.Sport.MLS,
                    "soccer": arbees_core.Sport.Soccer,
                    "tennis": arbees_core.Sport.Tennis,
                    "mma": arbees_core.Sport.MMA,
                }
                rust_sport = sport_map.get(state.sport.value)

            if not rust_sport:
                logger.warning(f"Unsupported sport for win prob: {state.sport}")
                return 0.5

            # Create Rust GameState
            # GameState::new(game_id, sport, home_team, away_team, home_score, away_score, period, time_remaining_seconds)
            rust_state = arbees_core.GameState(
                state.game_id,
                rust_sport,
                state.home_team,
                state.away_team,
                state.home_score,
                state.away_score,
                state.period,
                state.time_remaining_seconds
            )

            # Set optional fields if present
            if state.possession:
                rust_state.possession = state.possession
            
            if state.sport in (Sport.NFL, Sport.NCAAF):
                if state.down is not None:
                    rust_state.down = state.down
                if state.yards_to_go is not None:
                    rust_state.yards_to_go = state.yards_to_go
                if state.yard_line is not None:
                    rust_state.yard_line = state.yard_line
                rust_state.is_redzone = state.is_redzone

            # Calculate probability (for home team)
            raw_prob = arbees_core.calculate_win_probability(rust_state, True)
            
            # Clamp probability to [0.05, 0.95] to avoid extreme confidence / div-by-zero
            return max(0.05, min(0.95, raw_prob))

        except Exception as e:
            logger.error(f"Error calculating win prob with Rust core: {e}")
            # Fallback to simple heuristic
            score_diff = state.home_score - state.away_score
            prob = 0.5 + (score_diff * 0.05)
            return max(0.05, min(0.95, prob))

    # ==========================================================================
    # Status and Metrics
    # ==========================================================================

    def get_status(self) -> dict:
        """Get shard status including circuit breaker state."""
        cb_status = self.circuit_breaker.status()

        return {
            "shard_id": self.shard_id,
            "running": self._running,
            "game_count": self.game_count,
            "max_games": self.max_games,
            "games": [
                {
                    "game_id": ctx.game_id,
                    "sport": ctx.sport.value,
                    "plays_detected": ctx.plays_detected,
                    "signals_generated": ctx.signals_generated,
                    "started_at": ctx.started_at.isoformat(),
                    "status": ctx.last_state.status if ctx.last_state else "unknown",
                    "score": f"{ctx.last_state.home_score}-{ctx.last_state.away_score}" if ctx.last_state else "0-0",
                }
                for ctx in self._games.values()
            ],
            # NEW: Circuit breaker status (terauss integration)
            "circuit_breaker": {
                "trading_allowed": self.circuit_breaker.is_trading_allowed(),
                "halted": cb_status.get("halted", False),
                "daily_pnl": cb_status.get("daily_pnl_dollars", 0.0),
                "consecutive_errors": cb_status.get("consecutive_errors", 0),
                "trip_reason": cb_status.get("trip_reason"),
            },
        }


# Entry point for running as service
async def main():
    """Run GameShard as standalone service."""
    logging.basicConfig(level=logging.INFO)

    shard = GameShard()
    await shard.start()

    try:
        # Keep running until interrupted
        while True:
            await asyncio.sleep(60)
            logger.info(f"Shard status: {shard.get_status()}")
    except asyncio.CancelledError:
        pass
    finally:
        await shard.stop()


if __name__ == "__main__":
    asyncio.run(main())
