"""
Orchestrator service for managing GameShards.

Responsibilities:
- Discover live games across all sports
- Assign games to shards with load balancing
- Monitor shard health
- Handle shard failures with redistribution
"""

import asyncio
import logging
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Optional

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.game import GameInfo, Sport
from data_providers.espn.client import ESPNClient
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient

logger = logging.getLogger(__name__)


@dataclass
class ShardInfo:
    """Information about a GameShard."""
    shard_id: str
    game_count: int = 0
    max_games: int = 20
    games: list[str] = field(default_factory=list)
    last_heartbeat: datetime = field(default_factory=datetime.utcnow)
    is_healthy: bool = True

    @property
    def available_capacity(self) -> int:
        """Number of games shard can accept."""
        return max(0, self.max_games - self.game_count)


@dataclass
class GameAssignment:
    """Assignment of a game to a shard."""
    game_id: str
    sport: Sport
    shard_id: str
    kalshi_market_id: Optional[str] = None
    polymarket_market_id: Optional[str] = None
    assigned_at: datetime = field(default_factory=datetime.utcnow)


class Orchestrator:
    """
    Orchestrates game monitoring across multiple GameShards.

    Features:
    - Auto-discovery of live games
    - Load-balanced game assignment
    - Shard health monitoring
    - Automatic redistribution on failure
    """

    # Supported sports for monitoring
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
        discovery_interval: float = 30.0,
        health_check_interval: float = 15.0,
        shard_timeout: float = 60.0,
    ):
        """
        Initialize Orchestrator.

        Args:
            discovery_interval: Seconds between game discovery scans
            health_check_interval: Seconds between health checks
            shard_timeout: Seconds before considering shard unhealthy
        """
        self.discovery_interval = discovery_interval
        self.health_check_interval = health_check_interval
        self.shard_timeout = timedelta(seconds=shard_timeout)

        # Connections
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None

        # ESPN clients by sport
        self._espn_clients: dict[Sport, ESPNClient] = {}

        # State
        self._shards: dict[str, ShardInfo] = {}
        self._assignments: dict[str, GameAssignment] = {}  # game_id -> assignment
        self._running = False

        # Tasks
        self._discovery_task: Optional[asyncio.Task] = None
        self._health_task: Optional[asyncio.Task] = None
        self._heartbeat_task: Optional[asyncio.Task] = None

    async def start(self) -> None:
        """Start the orchestrator."""
        logger.info("Starting Orchestrator")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Subscribe to shard heartbeats
        await self.redis.psubscribe(
            "shard:*:heartbeat",
            self._handle_shard_heartbeat,
        )
        await self.redis.start_listening()

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

        self._running = True

        # Start background tasks
        self._discovery_task = asyncio.create_task(self._discovery_loop())
        self._health_task = asyncio.create_task(self._health_check_loop())

        logger.info("Orchestrator started")

    async def stop(self) -> None:
        """Stop the orchestrator gracefully."""
        logger.info("Stopping Orchestrator")
        self._running = False

        # Cancel tasks
        for task in [self._discovery_task, self._health_task, self._heartbeat_task]:
            if task:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

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

        logger.info("Orchestrator stopped")

    # ==========================================================================
    # Shard Management
    # ==========================================================================

    async def _handle_shard_heartbeat(self, channel: str, data: dict) -> None:
        """Handle heartbeat from a shard."""
        shard_id = data.get("shard_id")
        if not shard_id:
            return

        # Update shard info
        if shard_id not in self._shards:
            self._shards[shard_id] = ShardInfo(shard_id=shard_id)
            logger.info(f"Discovered new shard: {shard_id}")

        shard = self._shards[shard_id]
        shard.game_count = data.get("game_count", 0)
        shard.max_games = data.get("max_games", 20)
        shard.games = data.get("games", [])
        shard.last_heartbeat = datetime.utcnow()
        shard.is_healthy = True

    def _get_best_shard(self) -> Optional[ShardInfo]:
        """Get the shard with most available capacity."""
        healthy_shards = [
            s for s in self._shards.values()
            if s.is_healthy and s.available_capacity > 0
        ]

        if not healthy_shards:
            return None

        # Sort by available capacity (most first)
        return max(healthy_shards, key=lambda s: s.available_capacity)

    async def _assign_game_to_shard(
        self,
        game: GameInfo,
        shard: ShardInfo,
        kalshi_market_id: Optional[str] = None,
        polymarket_market_id: Optional[str] = None,
    ) -> bool:
        """Assign a game to a shard via Redis command."""
        assignment = GameAssignment(
            game_id=game.game_id,
            sport=game.sport,
            shard_id=shard.shard_id,
            kalshi_market_id=kalshi_market_id,
            polymarket_market_id=polymarket_market_id,
        )

        # Send command to shard via Redis
        command = {
            "type": "add_game",
            "game_id": game.game_id,
            "sport": game.sport.value,
            "kalshi_market_id": kalshi_market_id,
            "polymarket_market_id": polymarket_market_id,
        }

        if self.redis:
            channel = f"shard:{shard.shard_id}:command"
            await self.redis.publish(channel, command)

        self._assignments[game.game_id] = assignment
        shard.game_count += 1
        shard.games.append(game.game_id)

        logger.info(f"Assigned game {game.game_id} to shard {shard.shard_id}")
        return True

    async def _unassign_game(self, game_id: str) -> None:
        """Remove a game assignment."""
        if game_id not in self._assignments:
            return

        assignment = self._assignments[game_id]
        shard = self._shards.get(assignment.shard_id)

        # Send remove command to shard
        if self.redis:
            command = {
                "type": "remove_game",
                "game_id": game_id,
            }
            channel = f"shard:{assignment.shard_id}:command"
            await self.redis.publish(channel, command)

        # Update tracking
        if shard:
            shard.game_count = max(0, shard.game_count - 1)
            if game_id in shard.games:
                shard.games.remove(game_id)

        del self._assignments[game_id]
        logger.info(f"Unassigned game {game_id}")

    # ==========================================================================
    # Game Discovery
    # ==========================================================================

    async def _discovery_loop(self) -> None:
        """Periodically discover and assign live games."""
        while self._running:
            try:
                await self._discover_and_assign_games()
            except Exception as e:
                logger.error(f"Error in discovery loop: {e}")

            await asyncio.sleep(self.discovery_interval)

    async def _discover_and_assign_games(self) -> None:
        """Discover live games and assign to shards."""
        # Collect live games from all sports
        live_games: list[GameInfo] = []

        for sport, client in self._espn_clients.items():
            try:
                games = await client.get_live_games()
                live_games.extend(games)
            except Exception as e:
                logger.warning(f"Error fetching {sport.value} games: {e}")

        logger.info(f"Discovered {len(live_games)} live games across all sports")

        # Find games that need assignment
        assigned_ids = set(self._assignments.keys())
        new_games = [g for g in live_games if g.game_id not in assigned_ids]

        # Assign new games
        for game in new_games:
            shard = self._get_best_shard()
            if not shard:
                logger.warning(f"No available shards for game {game.game_id}")
                continue

            # Look up market IDs
            kalshi_id = await self._find_kalshi_market(game)
            poly_id = await self._find_polymarket_market(game)

            await self._assign_game_to_shard(game, shard, kalshi_id, poly_id)

        # Clean up finished games
        live_ids = {g.game_id for g in live_games}
        finished_ids = [gid for gid in assigned_ids if gid not in live_ids]

        for game_id in finished_ids:
            await self._unassign_game(game_id)

    async def _find_kalshi_market(self, game: GameInfo) -> Optional[str]:
        """Find Kalshi market for a game."""
        if not self.kalshi:
            return None

        try:
            # Search for market matching the game
            query = f"{game.away_team_abbrev} {game.home_team_abbrev}"
            markets = await self.kalshi.search_markets(query, limit=10)

            for market in markets:
                # Match by team names or game keywords
                title = market.get("title", "").lower()
                if (
                    game.home_team_abbrev.lower() in title or
                    game.away_team_abbrev.lower() in title
                ):
                    return market.get("ticker")

        except Exception as e:
            logger.debug(f"Error finding Kalshi market: {e}")

        return None

    async def _find_polymarket_market(self, game: GameInfo) -> Optional[str]:
        """Find Polymarket market for a game."""
        if not self.polymarket:
            return None

        try:
            # Search for market matching the game
            query = f"{game.away_team} {game.home_team}"
            markets = await self.polymarket.search_markets(query, limit=10)

            for market in markets:
                title = market.get("question", market.get("title", "")).lower()
                if (
                    game.home_team.lower() in title or
                    game.away_team.lower() in title
                ):
                    return market.get("condition_id") or market.get("id")

        except Exception as e:
            logger.debug(f"Error finding Polymarket market: {e}")

        return None

    # ==========================================================================
    # Health Monitoring
    # ==========================================================================

    async def _health_check_loop(self) -> None:
        """Periodically check shard health."""
        while self._running:
            try:
                await self._check_shard_health()
            except Exception as e:
                logger.error(f"Error in health check loop: {e}")

            await asyncio.sleep(self.health_check_interval)

    async def _check_shard_health(self) -> None:
        """Check health of all shards."""
        now = datetime.utcnow()

        for shard_id, shard in list(self._shards.items()):
            time_since_heartbeat = now - shard.last_heartbeat

            if time_since_heartbeat > self.shard_timeout:
                if shard.is_healthy:
                    logger.warning(f"Shard {shard_id} is unhealthy (no heartbeat)")
                    shard.is_healthy = False

                    # Redistribute games from unhealthy shard
                    await self._redistribute_shard_games(shard_id)
            else:
                shard.is_healthy = True

    async def _redistribute_shard_games(self, failed_shard_id: str) -> None:
        """Redistribute games from a failed shard."""
        # Find games assigned to failed shard
        games_to_reassign = [
            a for a in self._assignments.values()
            if a.shard_id == failed_shard_id
        ]

        if not games_to_reassign:
            return

        logger.info(f"Redistributing {len(games_to_reassign)} games from failed shard {failed_shard_id}")

        for assignment in games_to_reassign:
            # Remove old assignment
            del self._assignments[assignment.game_id]

            # Find new shard
            new_shard = self._get_best_shard()
            if not new_shard:
                logger.error(f"No available shard for game {assignment.game_id}")
                continue

            # Create fake GameInfo for assignment
            game_info = GameInfo(
                game_id=assignment.game_id,
                sport=assignment.sport,
                home_team="",
                away_team="",
                home_team_abbrev="",
                away_team_abbrev="",
                scheduled_time=datetime.utcnow(),
            )

            await self._assign_game_to_shard(
                game_info,
                new_shard,
                assignment.kalshi_market_id,
                assignment.polymarket_market_id,
            )

    # ==========================================================================
    # Status and API
    # ==========================================================================

    def get_status(self) -> dict:
        """Get orchestrator status."""
        return {
            "running": self._running,
            "shards": [
                {
                    "shard_id": s.shard_id,
                    "game_count": s.game_count,
                    "max_games": s.max_games,
                    "available_capacity": s.available_capacity,
                    "is_healthy": s.is_healthy,
                    "last_heartbeat": s.last_heartbeat.isoformat(),
                    "games": s.games,
                }
                for s in self._shards.values()
            ],
            "total_games": len(self._assignments),
            "assignments": [
                {
                    "game_id": a.game_id,
                    "sport": a.sport.value,
                    "shard_id": a.shard_id,
                    "has_kalshi": a.kalshi_market_id is not None,
                    "has_polymarket": a.polymarket_market_id is not None,
                }
                for a in self._assignments.values()
            ],
        }


# Entry point for running as service
async def main():
    """Run Orchestrator as standalone service."""
    logging.basicConfig(level=logging.INFO)

    orchestrator = Orchestrator()
    await orchestrator.start()

    try:
        while True:
            await asyncio.sleep(60)
            status = orchestrator.get_status()
            logger.info(f"Orchestrator status: {status['total_games']} games, {len(status['shards'])} shards")
    except asyncio.CancelledError:
        pass
    finally:
        await orchestrator.stop()


if __name__ == "__main__":
    asyncio.run(main())
