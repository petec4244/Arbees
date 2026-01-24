"""
FuturesMonitor service for pre-game market tracking.

Workflow:
1. Discovery Loop (every 30 minutes)
   - Poll ESPN for games 24-48h ahead
   - Insert into futures_games table

2. Market Discovery (continuous)
   - Find Kalshi/Polymarket markets as they list
   - Store market IDs for handoff

3. Price Polling (every 60 seconds)
   - Record prices to futures_price_history
   - Track line movement from opening

4. Signal Generation
   - Generate signal when edge >= 5%
   - Track line movement alerts (>= 3% move)

5. Handoff to Orchestrator (15 minutes before start)
   - Publish to Redis 'futures:game_starting' channel
   - Include discovered market IDs
"""

import asyncio
import logging
import os
import json
import uuid
from datetime import datetime, timedelta, timezone
from typing import Optional

import redis.asyncio as redis

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.game import GameInfo, Sport
from arbees_shared.models.market import Platform
from arbees_shared.models.market_types import MarketType
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from data_providers.espn.client import ESPNClient
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from .config import FuturesConfig

logger = logging.getLogger(__name__)


class FuturesMonitor:
    """
    Monitors upcoming games and tracks pre-game market movements.

    Features:
    - Auto-discovery of upcoming games from ESPN
    - Market discovery on Kalshi and Polymarket
    - Price history tracking (for ML analysis)
    - Line movement detection and alerting
    - Seamless handoff to Orchestrator when games start
    """

    # Supported sports
    SUPPORTED_SPORTS = [
        Sport.NFL,
        Sport.NBA,
        Sport.NHL,
        Sport.MLB,
        Sport.NCAAF,
        Sport.NCAAB,
        Sport.MLS,
    ]

    def __init__(
        self,
        db: Optional[DatabaseClient] = None,
        redis_bus: Optional[RedisBus] = None,
        config: Optional[FuturesConfig] = None,
    ):
        """Initialize the FuturesMonitor.

        Args:
            db: Database client. If None, will be created on start.
            redis_bus: Redis client for pub/sub. If None, will be created on start.
            config: Monitor configuration. Uses defaults if None.
        """
        self.db = db
        self.redis = redis_bus
        self.config = config or FuturesConfig.from_env()
        self._running = False

        # ESPN clients by sport
        self._espn_clients: dict[Sport, ESPNClient] = {}

        # Market clients
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None

        # In-memory tracking
        self._monitored_games: dict[str, dict] = {}  # game_id -> game_data
        self._market_cache: dict[str, dict] = {}  # game_id -> market_ids

        # Background tasks
        self._discovery_task: Optional[asyncio.Task] = None
        self._price_poll_task: Optional[asyncio.Task] = None
        self._handoff_task: Optional[asyncio.Task] = None
        self._discovery_listener_task: Optional[asyncio.Task] = None

        # Redis for discovery requests (Rust service)
        self._discovery_client: Optional[redis.Redis] = None
        self._discovery_pubsub: Optional[redis.client.PubSub] = None
        self._market_discovery_mode = os.environ.get("MARKET_DISCOVERY_MODE", "rust").lower()

        # Heartbeat publisher for health monitoring
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

    async def start(self) -> None:
        """Start the FuturesMonitor service."""
        logger.info(
            f"Starting FuturesMonitor (lookahead={self.config.lookahead_hours}h, "
            f"handoff={self.config.handoff_minutes}min, min_edge={self.config.min_edge_pct}%)"
        )

        # Connect to database
        if self.db is None:
            pool = await get_pool()
            self.db = DatabaseClient(pool)

        # Connect to Redis
        if self.redis is None:
            self.redis = RedisBus()
            await self.redis.connect()

        # Connect to market clients
        self.kalshi = KalshiClient()
        await self.kalshi.connect()

        self.polymarket = PolymarketClient()
        await self.polymarket.connect()

        # Create ESPN clients for each sport
        for sport in self.SUPPORTED_SPORTS:
            client = ESPNClient(sport)
            await client.connect()
            self._espn_clients[sport] = client

        # Connect to Redis for Rust discovery
        if self._market_discovery_mode == "rust":
            self._discovery_client = redis.from_url(
                os.environ.get("REDIS_URL", "redis://redis:6379"),
                decode_responses=False,
            )
            await self._discovery_client.ping()
            logger.info("Connected to Redis for Rust market discovery")

            # Subscribe to discovery results
            self._discovery_pubsub = self._discovery_client.pubsub()
            await self._discovery_pubsub.subscribe("discovery:results")
            logger.info("Subscribed to discovery:results channel")

        self._running = True

        # Start background tasks
        self._discovery_task = asyncio.create_task(self._game_discovery_loop())
        self._price_poll_task = asyncio.create_task(self._price_poll_loop())
        self._handoff_task = asyncio.create_task(self._handoff_check_loop())

        # Start discovery results listener if using Rust mode
        if self._market_discovery_mode == "rust" and self._discovery_pubsub:
            self._discovery_listener_task = asyncio.create_task(self._discovery_results_listener())

        # Run initial discovery immediately
        await self._discover_upcoming_games()

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="futures_monitor",
            instance_id=os.environ.get("HOSTNAME", "futures-monitor-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
            "kalshi_ok": self.kalshi is not None,
            "polymarket_ok": self.polymarket is not None,
        })

        logger.info("FuturesMonitor started")

    async def stop(self) -> None:
        """Stop the FuturesMonitor service."""
        logger.info("Stopping FuturesMonitor")
        self._running = False

        # Stop heartbeat publisher
        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        # Cancel background tasks
        for task in [self._discovery_task, self._price_poll_task, self._handoff_task, self._discovery_listener_task]:
            if task:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

        # Close discovery pubsub
        if self._discovery_pubsub:
            await self._discovery_pubsub.unsubscribe()
            await self._discovery_pubsub.close()

        # Disconnect ESPN clients
        for client in self._espn_clients.values():
            await client.disconnect()

        # Disconnect market clients
        if self.kalshi:
            await self.kalshi.disconnect()
        if self.polymarket:
            await self.polymarket.disconnect()

        # Disconnect Redis
        if self.redis:
            await self.redis.disconnect()
        if self._discovery_client:
            await self._discovery_client.close()

        logger.info("FuturesMonitor stopped")

    # ==========================================================================
    # Game Discovery
    # ==========================================================================

    async def _game_discovery_loop(self) -> None:
        """Periodically discover upcoming games."""
        while self._running:
            try:
                await self._discover_upcoming_games()
            except Exception as e:
                logger.error(f"Error in game discovery: {e}", exc_info=True)

            await asyncio.sleep(self.config.game_discovery_interval_seconds)

    async def _discover_upcoming_games(self) -> None:
        """Discover upcoming games from ESPN and add to monitoring."""
        now = datetime.utcnow()
        window_start = now + timedelta(hours=self.config.min_hours_before_start)
        window_end = now + timedelta(hours=self.config.lookahead_hours)

        logger.info(f"Discovering games from {window_start} to {window_end}")

        all_games: list[GameInfo] = []

        for sport, client in self._espn_clients.items():
            try:
                # Get scheduled games for the next few days
                games = await client.get_scheduled_games(days_ahead=3)

                # Filter to our monitoring window
                for game in games:
                    # Normalize timezone
                    scheduled = game.scheduled_time
                    if scheduled.tzinfo is not None:
                        scheduled = scheduled.astimezone(timezone.utc).replace(tzinfo=None)

                    if window_start <= scheduled <= window_end:
                        all_games.append(game)

            except Exception as e:
                logger.warning(f"Error fetching {sport.value} games: {e}")

        logger.info(f"Found {len(all_games)} upcoming games in monitoring window")

        # Add new games to monitoring
        for game in all_games:
            if game.game_id not in self._monitored_games:
                await self._add_game_to_monitoring(game)

    async def _add_game_to_monitoring(self, game: GameInfo) -> None:
        """Add a game to futures monitoring."""
        pool = await get_pool()

        # Normalize scheduled_time
        scheduled = game.scheduled_time
        if scheduled.tzinfo is not None:
            scheduled = scheduled.astimezone(timezone.utc).replace(tzinfo=None)

        # Insert into futures_games table
        try:
            await pool.execute(
                """
                INSERT INTO futures_games (
                    game_id, sport, home_team, away_team,
                    home_team_abbrev, away_team_abbrev,
                    scheduled_time, lifecycle_status
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'futures_monitoring')
                ON CONFLICT (game_id) DO UPDATE SET
                    scheduled_time = EXCLUDED.scheduled_time,
                    updated_at = NOW()
                """,
                game.game_id,
                game.sport.value,
                game.home_team,
                game.away_team,
                game.home_team_abbrev,
                game.away_team_abbrev,
                scheduled,
            )

            # Also ensure game exists in main games table
            await pool.execute(
                """
                INSERT INTO games (
                    game_id, sport, home_team, away_team,
                    home_team_abbrev, away_team_abbrev,
                    scheduled_time, status, lifecycle_status
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'scheduled', 'futures_monitoring')
                ON CONFLICT (game_id) DO UPDATE SET
                    scheduled_time = EXCLUDED.scheduled_time,
                    lifecycle_status = 'futures_monitoring',
                    updated_at = NOW()
                """,
                game.game_id,
                game.sport.value,
                game.home_team,
                game.away_team,
                game.home_team_abbrev,
                game.away_team_abbrev,
                scheduled,
            )

            self._monitored_games[game.game_id] = {
                "game_id": game.game_id,
                "sport": game.sport,
                "home_team": game.home_team,
                "away_team": game.away_team,
                "scheduled_time": scheduled,
                "markets_discovered": False,
            }

            logger.info(
                f"Added to futures monitoring: {game.away_team} @ {game.home_team} "
                f"({game.sport.value}) at {scheduled}"
            )

            # Request market discovery
            await self._request_market_discovery(game)

        except Exception as e:
            logger.error(f"Error adding game {game.game_id} to monitoring: {e}")

    async def _request_market_discovery(self, game: GameInfo) -> None:
        """Request market discovery for a game."""
        if self._market_discovery_mode == "rust" and self._discovery_client:
            request = {
                "game_id": game.game_id,
                "sport": game.sport.value,
                "home_team": game.home_team,
                "away_team": game.away_team,
                "home_abbr": game.home_team_abbrev,
                "away_abbr": game.away_team_abbrev,
            }
            await self._discovery_client.publish(
                Channel.DISCOVERY_REQUESTS.value,
                json.dumps(request).encode("utf-8"),
            )
            logger.debug(f"Sent discovery request for {game.game_id}")
        else:
            # Fallback to local discovery
            await self._discover_markets_locally(game)

    async def _discovery_results_listener(self) -> None:
        """Listen for market discovery results from Rust service."""
        logger.info("Starting discovery results listener")

        while self._running and self._discovery_pubsub:
            try:
                message = await self._discovery_pubsub.get_message(
                    ignore_subscribe_messages=True,
                    timeout=1.0,
                )
                if message is None:
                    continue

                if message["type"] != "message":
                    continue

                # Parse discovery result
                data = json.loads(message["data"])
                game_id = data.get("game_id")
                poly_id = data.get("polymarket_moneyline")
                kalshi_id = data.get("kalshi_moneyline")

                if not game_id:
                    continue

                # Update database if we got any market IDs
                if poly_id or kalshi_id:
                    await self._update_game_markets(game_id, kalshi_id, poly_id)
                    logger.info(
                        f"Received discovery result: game={game_id} "
                        f"kalshi={kalshi_id} poly={poly_id}"
                    )

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Error in discovery results listener: {e}", exc_info=True)
                await asyncio.sleep(1)

        logger.info("Discovery results listener stopped")

    async def _discover_markets_locally(self, game: GameInfo) -> None:
        """Discover markets using local clients (fallback)."""
        kalshi_id = None
        poly_id = None

        try:
            if self.kalshi:
                markets = await self.kalshi.get_markets(sport=game.sport.value, limit=200)
                for market in markets:
                    title = market.get("title", "").lower()
                    if (game.home_team.lower() in title or game.away_team.lower() in title):
                        if "win" in title:
                            kalshi_id = market.get("ticker")
                            break
        except Exception as e:
            logger.debug(f"Error finding Kalshi market: {e}")

        try:
            if self.polymarket:
                query = f"{game.away_team} {game.home_team}"
                markets = await self.polymarket.search_markets(query, limit=10)
                for market in markets:
                    title = market.get("question", market.get("title", "")).lower()
                    if (game.home_team.lower() in title or game.away_team.lower() in title):
                        poly_id = market.get("condition_id") or market.get("id")
                        break
        except Exception as e:
            logger.debug(f"Error finding Polymarket market: {e}")

        if kalshi_id or poly_id:
            await self._update_game_markets(game.game_id, kalshi_id, poly_id)

    async def _update_game_markets(
        self,
        game_id: str,
        kalshi_id: Optional[str],
        poly_id: Optional[str],
    ) -> None:
        """Update market IDs for a game."""
        pool = await get_pool()

        await pool.execute(
            """
            UPDATE futures_games
            SET kalshi_market_id = COALESCE($2, kalshi_market_id),
                polymarket_market_id = COALESCE($3, polymarket_market_id),
                markets_discovered_at = CASE
                    WHEN $2 IS NOT NULL OR $3 IS NOT NULL THEN NOW()
                    ELSE markets_discovered_at
                END,
                updated_at = NOW()
            WHERE game_id = $1
            """,
            game_id,
            kalshi_id,
            poly_id,
        )

        self._market_cache[game_id] = {
            "kalshi": kalshi_id,
            "polymarket": poly_id,
        }

        if game_id in self._monitored_games:
            self._monitored_games[game_id]["markets_discovered"] = True

        logger.info(
            f"Updated markets for {game_id}: kalshi={kalshi_id}, polymarket={poly_id}"
        )

    # ==========================================================================
    # Price Polling
    # ==========================================================================

    async def _price_poll_loop(self) -> None:
        """Periodically poll prices for monitored games."""
        while self._running:
            try:
                await self._poll_all_prices()
                
                # Update heartbeat metrics
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.update_metrics({
                        "games_monitored": float(len(self._monitored_games)),
                        "markets_cached": float(len(self._market_cache)),
                    })
            except Exception as e:
                logger.error(f"Error in price polling: {e}", exc_info=True)

            await asyncio.sleep(self.config.price_poll_interval_seconds)

    async def _poll_all_prices(self) -> None:
        """Poll prices for all monitored games."""
        pool = await get_pool()
        now = datetime.utcnow()

        # Get games with discovered markets
        rows = await pool.fetch(
            """
            SELECT game_id, sport, home_team, away_team, scheduled_time,
                   kalshi_market_id, polymarket_market_id,
                   opening_home_prob, current_home_prob
            FROM futures_games
            WHERE lifecycle_status = 'futures_monitoring'
              AND (kalshi_market_id IS NOT NULL OR polymarket_market_id IS NOT NULL)
            """
        )

        for row in rows:
            game_id = row["game_id"]
            scheduled = row["scheduled_time"]
            # Normalize timezone - strip tzinfo if present
            if scheduled.tzinfo is not None:
                scheduled = scheduled.replace(tzinfo=None)
            hours_until = (scheduled - now).total_seconds() / 3600

            try:
                # Poll Kalshi
                if row["kalshi_market_id"] and self.kalshi:
                    await self._poll_kalshi_price(row, hours_until)

                # Poll Polymarket
                if row["polymarket_market_id"] and self.polymarket:
                    await self._poll_polymarket_price(row, hours_until)

                # Check for signals
                await self._check_for_signals(row, hours_until)

            except Exception as e:
                logger.debug(f"Error polling prices for {game_id}: {e}")

    async def _poll_kalshi_price(self, row: dict, hours_until: float) -> None:
        """Poll Kalshi price for a game."""
        pool = await get_pool()
        market_id = row["kalshi_market_id"]

        try:
            market = await self.kalshi.get_market(market_id)
            if not market:
                return

            yes_bid = float(market.get("yes_bid", 0) or 0) / 100
            yes_ask = float(market.get("yes_ask", 0) or 0) / 100
            yes_mid = (yes_bid + yes_ask) / 2 if yes_bid and yes_ask else 0
            spread_cents = (yes_ask - yes_bid) * 100 if yes_bid and yes_ask else 0
            volume = float(market.get("volume", 0) or 0)

            # Insert price history
            await pool.execute(
                """
                INSERT INTO futures_price_history (
                    time, game_id, platform, market_type, team,
                    yes_bid, yes_ask, yes_mid, spread_cents,
                    volume, hours_until_start
                ) VALUES (NOW(), $1, 'kalshi', 'moneyline', NULL,
                          $2, $3, $4, $5, $6, $7)
                """,
                row["game_id"],
                yes_bid,
                yes_ask,
                yes_mid,
                spread_cents,
                volume,
                hours_until,
            )

            # Update opening/current probs
            await self._update_probabilities(
                row["game_id"], yes_mid, row["opening_home_prob"]
            )

        except Exception as e:
            logger.debug(f"Error polling Kalshi {market_id}: {e}")

    async def _poll_polymarket_price(self, row: dict, hours_until: float) -> None:
        """Poll Polymarket price for a game."""
        pool = await get_pool()
        market_id = row["polymarket_market_id"]

        try:
            # Use get_market_price which properly extracts prices from orderbook
            price = await self.polymarket.get_market_price(market_id)
            if not price:
                return

            # MarketPrice object has yes_bid, yes_ask, volume
            yes_bid = price.yes_bid if price.yes_bid is not None else 0
            yes_ask = price.yes_ask if price.yes_ask is not None else 0
            # Handle cases where one side might be 0 or missing
            if yes_bid is not None and yes_ask is not None:
                yes_mid = (yes_bid + yes_ask) / 2
                spread_cents = (yes_ask - yes_bid) * 100
            elif yes_ask is not None and yes_ask > 0:
                yes_mid = yes_ask / 2  # Estimate from ask only
                spread_cents = 0
            elif yes_bid is not None and yes_bid > 0:
                yes_mid = (yes_bid + 1) / 2  # Estimate from bid only
                spread_cents = 0
            else:
                yes_mid = 0
                spread_cents = 0
            volume = price.volume if price.volume is not None else 0

            # Insert price history
            await pool.execute(
                """
                INSERT INTO futures_price_history (
                    time, game_id, platform, market_type, team,
                    yes_bid, yes_ask, yes_mid, spread_cents,
                    volume, hours_until_start
                ) VALUES (NOW(), $1, 'polymarket', 'moneyline', NULL,
                          $2, $3, $4, $5, $6, $7)
                """,
                row["game_id"],
                yes_bid,
                yes_ask,
                yes_mid,
                spread_cents,
                volume,
                hours_until,
            )

            # Update probabilities if Kalshi didn't
            if not row["kalshi_market_id"]:
                if yes_mid > 0:
                    await self._update_probabilities(
                        row["game_id"], yes_mid, row["opening_home_prob"]
                    )
                    logger.debug(f"Updated probability for {row['game_id']}: yes_mid={yes_mid:.3f}")

        except Exception as e:
            logger.warning(f"Error polling Polymarket {market_id}: {e}", exc_info=True)

    async def _update_probabilities(
        self,
        game_id: str,
        current_prob: float,
        opening_prob: Optional[float],
    ) -> None:
        """Update opening and current probabilities for a game."""
        pool = await get_pool()

        if opening_prob is None:
            # First price - set as opening
            away_prob = 1.0 - current_prob
            await pool.execute(
                """
                UPDATE futures_games
                SET opening_home_prob = $2,
                    opening_away_prob = $3,
                    current_home_prob = $2,
                    current_away_prob = $3,
                    updated_at = NOW()
                WHERE game_id = $1 AND opening_home_prob IS NULL
                """,
                game_id,
                current_prob,
                away_prob,
            )
        else:
            # Update current and calculate movement
            # Convert opening_prob from Decimal (from DB) to float
            opening_prob_float = float(opening_prob)
            movement = abs(current_prob - opening_prob_float) * 100
            direction = "home" if current_prob > opening_prob_float else "away"
            away_prob = 1.0 - current_prob

            await pool.execute(
                """
                UPDATE futures_games
                SET current_home_prob = $2,
                    current_away_prob = $3,
                    line_movement_pct = $4,
                    max_movement_pct = GREATEST(max_movement_pct, $4),
                    movement_direction = $5,
                    updated_at = NOW()
                WHERE game_id = $1
                """,
                game_id,
                current_prob,
                away_prob,
                movement,
                direction,
            )

    # ==========================================================================
    # Signal Generation
    # ==========================================================================

    @staticmethod
    def _kalshi_fee_pct(price: float) -> float:
        """Kalshi fee per contract as a fraction of $1 (price in 0-1)."""
        price_cents = int(round(price * 100))
        if price_cents <= 0 or price_cents >= 100:
            return 0.0
        numerator = 7 * price_cents * (100 - price_cents) + 9999
        fee_cents = numerator // 10000
        return fee_cents / 100.0

    def _net_edge_after_fees_pct(self, entry_price: float, exit_price: float, edge_pct: float) -> float:
        """Estimate net edge after Kalshi entry/exit fees (percent points)."""
        fee_total = self._kalshi_fee_pct(entry_price) + self._kalshi_fee_pct(exit_price)
        return max(0.0, edge_pct - (fee_total * 100.0))

    async def _check_for_signals(self, row: dict, hours_until: float) -> None:
        """Check for signal conditions on a game."""
        pool = await get_pool()
        game_id = row["game_id"]

        # Get latest prices from both platforms
        latest = await pool.fetch(
            """
            SELECT platform, yes_mid
            FROM futures_price_history
            WHERE game_id = $1
              AND time > NOW() - INTERVAL '5 minutes'
            ORDER BY time DESC
            """,
            game_id,
        )

        if len(latest) < 2:
            return

        prices = {r["platform"]: r["yes_mid"] for r in latest if r["yes_mid"]}
        kalshi_prob = prices.get("kalshi")
        poly_prob = prices.get("polymarket")

        # Cross-platform edge detection
        if kalshi_prob and poly_prob:
            edge = abs(kalshi_prob - poly_prob) * 100

            # Fee-aware edge adjustment for Kalshi entry/exit
            # Assume entry at Kalshi price and exit near Polymarket price
            entry_price = kalshi_prob
            exit_price = poly_prob
            fee_adjusted_edge = self._net_edge_after_fees_pct(entry_price, exit_price, edge)

            if fee_adjusted_edge >= self.config.min_edge_pct:
                await self._generate_signal(
                    row,
                    signal_type="futures_early_edge",
                    edge_pct=fee_adjusted_edge,
                    model_prob=max(kalshi_prob, poly_prob),
                    market_prob=min(kalshi_prob, poly_prob),
                    hours_until=hours_until,
                    reason=(
                        f"Cross-platform edge (fee adj): Kalshi={kalshi_prob:.1%} "
                        f"vs Poly={poly_prob:.1%}"
                    ),
                )

        # Line movement detection
        opening = row["opening_home_prob"]
        current = row["current_home_prob"]

        if opening and current:
            movement = abs(current - opening) * 100

            # Fee-aware movement adjustment (Kalshi entry at opening, exit at current)
            fee_adjusted_movement = self._net_edge_after_fees_pct(opening, current, movement)

            if fee_adjusted_movement >= self.config.line_movement_alert_pct:
                await self._generate_signal(
                    row,
                    signal_type="futures_line_movement",
                    edge_pct=fee_adjusted_movement,
                    model_prob=current,
                    market_prob=opening,
                    hours_until=hours_until,
                    opening_prob=opening,
                    current_prob=current,
                    movement_pct=fee_adjusted_movement,
                    reason=(
                        f"Line movement (fee adj): {opening:.1%} → {current:.1%} "
                        f"({fee_adjusted_movement:.1f}%)"
                    ),
                )

    async def _generate_signal(
        self,
        row: dict,
        signal_type: str,
        edge_pct: float,
        model_prob: float,
        market_prob: float,
        hours_until: float,
        reason: str,
        opening_prob: Optional[float] = None,
        current_prob: Optional[float] = None,
        movement_pct: Optional[float] = None,
    ) -> None:
        """Generate a futures signal."""
        pool = await get_pool()
        signal_id = f"futures-{uuid.uuid4().hex[:12]}"

        # Determine direction
        direction = "yes" if model_prob > market_prob else "no"

        await pool.execute(
            """
            INSERT INTO futures_signals (
                time, signal_id, game_id, sport,
                signal_type, direction, team, market_type,
                model_prob, market_prob, edge_pct, confidence,
                opening_prob, current_prob, movement_pct,
                hours_until_start, reason, expires_at
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, NULL, 'moneyline',
                $6, $7, $8, $9, $10, $11, $12, $13, $14,
                NOW() + INTERVAL '1 hour'
            )
            """,
            signal_id,
            row["game_id"],
            row["sport"],
            signal_type,
            direction,
            model_prob,
            market_prob,
            edge_pct,
            min(1.0, edge_pct / 10),  # Simple confidence scaling
            opening_prob,
            current_prob,
            movement_pct,
            hours_until,
            reason,
        )

        # Publish to Redis for real-time updates
        if self.redis:
            await self.redis.publish(
                "futures:signals:new",
                {
                    "signal_id": signal_id,
                    "game_id": row["game_id"],
                    "sport": row["sport"],
                    "matchup": f"{row['away_team']} @ {row['home_team']}",
                    "signal_type": signal_type,
                    "edge_pct": edge_pct,
                    "hours_until_start": hours_until,
                    "reason": reason,
                },
            )

        logger.info(
            f"FUTURES_SIGNAL | {signal_type} | {row['game_id']} | "
            f"edge={edge_pct:.1f}% | {reason}"
        )

    # ==========================================================================
    # Handoff to Orchestrator
    # ==========================================================================

    async def _handoff_check_loop(self) -> None:
        """Check for games ready to hand off to Orchestrator."""
        while self._running:
            try:
                await self._process_handoffs()
            except Exception as e:
                logger.error(f"Error in handoff check: {e}", exc_info=True)

            await asyncio.sleep(60)  # Check every minute

    async def _process_handoffs(self) -> None:
        """Hand off games that are about to start."""
        pool = await get_pool()
        handoff_cutoff = datetime.utcnow() + timedelta(
            minutes=self.config.handoff_minutes
        )

        # Get games ready for handoff
        rows = await pool.fetch(
            """
            SELECT game_id, sport, home_team, away_team, scheduled_time,
                   kalshi_market_id, polymarket_market_id, market_ids_by_type
            FROM futures_games
            WHERE lifecycle_status = 'futures_monitoring'
              AND scheduled_time <= $1
            """,
            handoff_cutoff,
        )

        for row in rows:
            await self._handoff_game(row)

    async def _handoff_game(self, row: dict) -> None:
        """Hand off a single game to Orchestrator."""
        pool = await get_pool()
        game_id = row["game_id"]

        logger.info(
            f"Handing off game {game_id} ({row['away_team']} @ {row['home_team']}) to Orchestrator"
        )

        # Update lifecycle status
        await pool.execute(
            """
            UPDATE futures_games
            SET lifecycle_status = 'pre_game',
                handed_off_at = NOW(),
                updated_at = NOW()
            WHERE game_id = $1
            """,
            game_id,
        )

        await pool.execute(
            """
            UPDATE games
            SET lifecycle_status = 'pre_game',
                updated_at = NOW()
            WHERE game_id = $1
            """,
            game_id,
        )

        # Publish handoff event to Redis
        if self.redis:
            handoff_data = {
                "game_id": game_id,
                "sport": row["sport"],
                "home_team": row["home_team"],
                "away_team": row["away_team"],
                "scheduled_time": row["scheduled_time"].isoformat() if row["scheduled_time"] else None,
                "kalshi_market_id": row["kalshi_market_id"],
                "polymarket_market_id": row["polymarket_market_id"],
                "market_ids_by_type": row["market_ids_by_type"],
            }
            await self.redis.publish("futures:game_starting", handoff_data)

            # Also notify Polymarket monitor if we have a market
            if row["polymarket_market_id"]:
                await self.redis.publish(
                    Channel.MARKET_ASSIGNMENTS.value,
                    {
                        "type": "polymarket_assign",
                        "game_id": game_id,
                        "sport": row["sport"],
                        "markets": [
                            {
                                "market_type": MarketType.MONEYLINE.value,
                                "condition_id": str(row["polymarket_market_id"]),
                            }
                        ],
                    },
                )

        # Remove from in-memory tracking
        self._monitored_games.pop(game_id, None)
        self._market_cache.pop(game_id, None)

        logger.info(f"Handoff complete for {game_id}")

    # ==========================================================================
    # Status and API
    # ==========================================================================

    def get_status(self) -> dict:
        """Get current monitor status."""
        return {
            "running": self._running,
            "monitored_games": len(self._monitored_games),
            "games_with_markets": sum(
                1 for g in self._monitored_games.values()
                if g.get("markets_discovered")
            ),
            "config": {
                "lookahead_hours": self.config.lookahead_hours,
                "handoff_minutes": self.config.handoff_minutes,
                "min_edge_pct": self.config.min_edge_pct,
                "line_movement_alert_pct": self.config.line_movement_alert_pct,
            },
        }


# Entry point for running as standalone service
async def main():
    """Run FuturesMonitor as standalone service."""
    log_level = os.environ.get("LOG_LEVEL", "INFO")
    logging.basicConfig(
        level=getattr(logging, log_level),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
    )

    monitor = FuturesMonitor()
    await monitor.start()

    try:
        while True:
            await asyncio.sleep(60)
            status = monitor.get_status()
            logger.info(
                f"FuturesMonitor status: {status['monitored_games']} games "
                f"({status['games_with_markets']} with markets)"
            )
    except asyncio.CancelledError:
        pass
    finally:
        await monitor.stop()


if __name__ == "__main__":
    asyncio.run(main())
