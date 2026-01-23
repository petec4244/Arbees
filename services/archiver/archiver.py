"""
GameArchiver service for archiving completed games.

Workflow:
1. Listen for 'games:ended' events from Redis (or poll database)
2. Wait grace period (for score corrections)
3. Copy data to archive tables (atomic transaction)
4. Mark original game as archived
5. Compute summary statistics
"""

import asyncio
import logging
from datetime import datetime, timedelta
from typing import Optional

from arbees_shared.db.connection import DatabaseClient, get_pool, transaction
from arbees_shared.messaging.redis_bus import RedisBus
from .config import ArchiverConfig

logger = logging.getLogger(__name__)


class GameArchiver:
    """
    Archives completed games to historical tables.

    The archiver runs continuously, listening for game ended events
    and processing them after a configurable grace period.

    Features:
    - Atomic transactions ensure data integrity
    - Grace period allows for score corrections
    - Computes summary statistics (win rate, P&L, etc.)
    - Can optionally clean up live tables after archiving
    """

    def __init__(
        self,
        db: Optional[DatabaseClient] = None,
        redis: Optional[RedisBus] = None,
        config: Optional[ArchiverConfig] = None,
    ):
        """Initialize the archiver.

        Args:
            db: Database client. If None, will be created on start.
            redis: Redis client for pub/sub. If None, will be created on start.
            config: Archiver configuration. Uses defaults if None.
        """
        self.db = db
        self.redis = redis
        self.config = config or ArchiverConfig.from_env()
        self._running = False
        self._pending_archives: dict[str, datetime] = {}  # game_id -> end_time
        self._poll_task: Optional[asyncio.Task] = None
        self._archive_task: Optional[asyncio.Task] = None

    async def start(self) -> None:
        """Start the archiver service."""
        logger.info(
            f"Starting GameArchiver (grace_period={self.config.grace_period_minutes}min, "
            f"poll_interval={self.config.poll_interval_seconds}s)"
        )

        # Connect to database
        if self.db is None:
            pool = await get_pool()
            self.db = DatabaseClient(pool)

        # Connect to Redis
        if self.redis is None:
            self.redis = RedisBus()
            await self.redis.connect()

        self._running = True

        # Subscribe to game ended events
        await self.redis.subscribe("games:ended", self._on_game_ended)
        asyncio.create_task(self.redis.start_listening())

        # Start background tasks
        self._poll_task = asyncio.create_task(self._poll_for_completed_games())
        self._archive_task = asyncio.create_task(self._archive_loop())

        logger.info("GameArchiver started")

    async def stop(self) -> None:
        """Stop the archiver service."""
        logger.info("Stopping GameArchiver")
        self._running = False

        # Cancel background tasks
        if self._poll_task:
            self._poll_task.cancel()
            try:
                await self._poll_task
            except asyncio.CancelledError:
                pass

        if self._archive_task:
            self._archive_task.cancel()
            try:
                await self._archive_task
            except asyncio.CancelledError:
                pass

        # Disconnect from Redis
        if self.redis:
            await self.redis.unsubscribe("games:ended")
            await self.redis.disconnect()

        logger.info("GameArchiver stopped")

    async def _on_game_ended(self, message: dict) -> None:
        """Handle game ended event from Redis."""
        game_id = message.get("game_id")
        if not game_id:
            logger.warning("Received game ended event without game_id")
            return

        # Check if already pending
        if game_id in self._pending_archives:
            logger.debug(f"Game {game_id} already pending archive")
            return

        logger.info(f"Game ended: {game_id}, queuing for archive (grace period: {self.config.grace_period_minutes}min)")
        self._pending_archives[game_id] = datetime.utcnow()

    async def _poll_for_completed_games(self) -> None:
        """Poll database for completed games not yet archived.

        This catches any games that ended while the archiver was down,
        or that didn't receive a Redis event.
        """
        while self._running:
            try:
                pool = await get_pool()
                rows = await pool.fetch(
                    """
                    SELECT game_id, updated_at
                    FROM games
                    WHERE status IN ('final', 'complete')
                      AND (archived IS NULL OR archived = FALSE)
                      AND updated_at < NOW() - INTERVAL '30 minutes'
                    ORDER BY updated_at ASC
                    LIMIT 50
                    """
                )

                for row in rows:
                    game_id = row["game_id"]
                    if game_id not in self._pending_archives:
                        logger.info(f"Found unarchived game from poll: {game_id}")
                        self._pending_archives[game_id] = row["updated_at"]

            except Exception as e:
                logger.error(f"Error polling for completed games: {e}")

            await asyncio.sleep(self.config.poll_interval_seconds)

    async def _archive_loop(self) -> None:
        """Process pending archives after grace period."""
        while self._running:
            try:
                await self._process_ready_games()
            except Exception as e:
                logger.error(f"Archive loop error: {e}", exc_info=True)

            # Check more frequently than poll interval for responsiveness
            await asyncio.sleep(60)

    async def _process_ready_games(self) -> None:
        """Archive games that have passed the grace period."""
        grace_cutoff = datetime.utcnow() - timedelta(
            minutes=self.config.grace_period_minutes
        )

        ready_games = [
            game_id
            for game_id, end_time in self._pending_archives.items()
            if end_time < grace_cutoff
        ]

        if not ready_games:
            return

        logger.info(f"Processing {len(ready_games)} games ready for archive")

        for game_id in ready_games[: self.config.archive_batch_size]:
            try:
                await self._archive_game(game_id)
                del self._pending_archives[game_id]
                logger.info(f"Successfully archived game: {game_id}")
            except Exception as e:
                logger.error(f"Failed to archive {game_id}: {e}", exc_info=True)
                # Keep in pending for retry, but update timestamp to delay retry
                self._pending_archives[game_id] = datetime.utcnow()

    async def _archive_game(self, game_id: str) -> None:
        """
        Archive a single game with all its data.

        Uses a transaction to ensure atomicity - either all data is
        archived or none of it is.
        """
        pool = await get_pool()

        async with pool.acquire() as conn:
            async with conn.transaction():
                # 1. Get game details
                game = await conn.fetchrow(
                    "SELECT * FROM games WHERE game_id = $1",
                    game_id
                )
                if not game:
                    logger.warning(f"Game {game_id} not found in database")
                    return

                if game.get("archived"):
                    logger.info(f"Game {game_id} already archived")
                    return

                # 2. Get all trades for this game
                trades = await conn.fetch(
                    """
                    SELECT * FROM paper_trades
                    WHERE game_id = $1
                    ORDER BY time ASC
                    """,
                    game_id
                )

                # 3. Get all signals for this game
                signals = await conn.fetch(
                    """
                    SELECT * FROM trading_signals
                    WHERE game_id = $1
                    ORDER BY time ASC
                    """,
                    game_id
                )

                # 4. Get the latest game state for ended_at timestamp
                latest_state = await conn.fetchrow(
                    """
                    SELECT time, status FROM game_states
                    WHERE game_id = $1
                    ORDER BY time DESC
                    LIMIT 1
                    """,
                    game_id
                )

                # 5. Compute summary statistics
                stats = self._compute_stats(trades, signals)

                # Determine ended_at time
                ended_at = latest_state["time"] if latest_state else game.get("updated_at") or datetime.utcnow()

                # 6. Insert into archived_games
                archive_id = await conn.fetchval(
                    """
                    INSERT INTO archived_games (
                        game_id, sport, home_team, away_team,
                        final_home_score, final_away_score,
                        scheduled_time, ended_at,
                        total_trades, winning_trades, losing_trades, push_trades,
                        total_pnl, total_signals_generated, total_signals_executed,
                        avg_edge_pct
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8,
                        $9, $10, $11, $12, $13, $14, $15, $16
                    )
                    RETURNING archive_id
                    """,
                    game_id,
                    game["sport"],
                    game["home_team"],
                    game["away_team"],
                    game.get("final_home_score") or 0,
                    game.get("final_away_score") or 0,
                    game.get("scheduled_time"),
                    ended_at,
                    stats["total_trades"],
                    stats["winning_trades"],
                    stats["losing_trades"],
                    stats["push_trades"],
                    stats["total_pnl"],
                    stats["total_signals_generated"],
                    stats["total_signals_executed"],
                    stats["avg_edge_pct"],
                )

                logger.info(
                    f"Created archived_game {archive_id} for {game_id}: "
                    f"{stats['total_trades']} trades, ${stats['total_pnl']:.2f} P&L"
                )

                # 7. Insert archived trades
                for trade in trades:
                    await conn.execute(
                        """
                        INSERT INTO archived_trades (
                            trade_id, archive_game_id, game_id, signal_id, signal_type,
                            platform, market_id, market_type, market_title, side, team,
                            entry_price, exit_price, size,
                            opened_at, closed_at,
                            status, outcome, pnl, pnl_pct,
                            edge_at_entry, model_prob_at_entry, market_prob_at_entry
                        ) VALUES (
                            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                            $12, $13, $14, $15, $16, $17, $18, $19, $20,
                            $21, $22, $23
                        )
                        """,
                        trade["trade_id"],
                        archive_id,
                        game_id,
                        trade.get("signal_id"),
                        trade.get("signal_type"),
                        trade["platform"],
                        trade["market_id"],
                        trade.get("market_type", "moneyline"),
                        trade.get("market_title"),
                        trade["side"],
                        None,  # team - could be extracted from market_title if needed
                        trade["entry_price"],
                        trade.get("exit_price"),
                        trade["size"],
                        trade.get("entry_time") or trade["time"],
                        trade.get("exit_time"),
                        trade["status"],
                        trade.get("outcome"),
                        trade.get("pnl"),
                        trade.get("pnl_pct"),
                        trade.get("edge_at_entry"),
                        trade.get("model_prob"),
                        None,  # market_prob_at_entry - not stored in paper_trades
                    )

                # 8. Insert archived signals
                executed_signal_ids = {t["signal_id"] for t in trades if t.get("signal_id")}
                for signal in signals:
                    was_executed = signal["signal_id"] in executed_signal_ids
                    await conn.execute(
                        """
                        INSERT INTO archived_signals (
                            signal_id, archive_game_id, game_id,
                            signal_type, direction, team, market_type,
                            model_prob, market_prob, edge_pct, confidence, reason,
                            generated_at, expires_at,
                            was_executed
                        ) VALUES (
                            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                            $13, $14, $15
                        )
                        """,
                        signal["signal_id"],
                        archive_id,
                        game_id,
                        signal["signal_type"],
                        signal["direction"],
                        signal.get("team"),
                        None,  # market_type - could be determined from context
                        signal.get("model_prob"),
                        signal.get("market_prob"),
                        signal["edge_pct"],
                        signal.get("confidence"),
                        signal.get("reason"),
                        signal["time"],
                        signal.get("expires_at"),
                        was_executed,
                    )

                # 9. Mark original game as archived
                await conn.execute(
                    """
                    UPDATE games
                    SET archived = TRUE, archived_at = NOW()
                    WHERE game_id = $1
                    """,
                    game_id
                )

                logger.info(
                    f"Archived game {game_id}: {len(trades)} trades, {len(signals)} signals"
                )

    def _compute_stats(self, trades: list, signals: list) -> dict:
        """Compute summary statistics for archived game."""
        winning = sum(1 for t in trades if t.get("outcome") == "win")
        losing = sum(1 for t in trades if t.get("outcome") == "loss")
        push = sum(1 for t in trades if t.get("outcome") == "push")
        total_pnl = sum(float(t.get("pnl") or 0) for t in trades)

        executed_signal_ids = {t.get("signal_id") for t in trades if t.get("signal_id")}
        executed_count = len(executed_signal_ids)

        edges = [float(t.get("edge_at_entry") or 0) for t in trades if t.get("edge_at_entry")]
        avg_edge = sum(edges) / len(edges) if edges else None

        return {
            "total_trades": len(trades),
            "winning_trades": winning,
            "losing_trades": losing,
            "push_trades": push,
            "total_pnl": total_pnl,
            "total_signals_generated": len(signals),
            "total_signals_executed": executed_count,
            "avg_edge_pct": avg_edge,
        }

    async def archive_game_manual(self, game_id: str) -> bool:
        """Manually trigger archiving for a specific game.

        Bypasses the grace period. Useful for admin operations.

        Args:
            game_id: The game ID to archive.

        Returns:
            True if archived successfully, False otherwise.
        """
        try:
            await self._archive_game(game_id)
            # Remove from pending if present
            self._pending_archives.pop(game_id, None)
            return True
        except Exception as e:
            logger.error(f"Manual archive failed for {game_id}: {e}")
            return False

    def get_status(self) -> dict:
        """Get current archiver status."""
        return {
            "running": self._running,
            "pending_archives": len(self._pending_archives),
            "pending_game_ids": list(self._pending_archives.keys()),
            "config": {
                "grace_period_minutes": self.config.grace_period_minutes,
                "archive_batch_size": self.config.archive_batch_size,
                "poll_interval_seconds": self.config.poll_interval_seconds,
            },
        }


# Entry point for running as standalone service
async def main():
    """Run GameArchiver as standalone service."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
    )

    archiver = GameArchiver()
    await archiver.start()

    try:
        # Keep running until interrupted
        while True:
            await asyncio.sleep(60)
            status = archiver.get_status()
            logger.info(f"Archiver status: {status['pending_archives']} pending")
    except asyncio.CancelledError:
        pass
    finally:
        await archiver.stop()


if __name__ == "__main__":
    asyncio.run(main())
