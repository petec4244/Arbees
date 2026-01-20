"""Async PostgreSQL/TimescaleDB connection management with asyncpg."""

import os
from contextlib import asynccontextmanager
from datetime import datetime
from typing import AsyncGenerator, Optional, Union

import asyncpg
from asyncpg import Pool, Connection

# Global connection pool
_pool: Optional[Pool] = None


def get_database_url() -> str:
    """Get database URL from environment.

    Raises:
        RuntimeError: If DATABASE_URL environment variable is not set.
    """
    url = os.environ.get("DATABASE_URL")
    if not url:
        raise RuntimeError(
            "DATABASE_URL environment variable is required. "
            "Set it in your .env file or environment."
        )
    return url


async def get_pool() -> Pool:
    """Get or create the connection pool."""
    global _pool
    if _pool is None:
        _pool = await asyncpg.create_pool(
            get_database_url(),
            min_size=5,
            max_size=20,
            command_timeout=60,
            statement_cache_size=100,
        )
    return _pool


async def close_pool() -> None:
    """Close the connection pool."""
    global _pool
    if _pool is not None:
        await _pool.close()
        _pool = None


@asynccontextmanager
async def get_connection() -> AsyncGenerator[Connection, None]:
    """Get a connection from the pool."""
    pool = await get_pool()
    async with pool.acquire() as conn:
        yield conn


@asynccontextmanager
async def transaction() -> AsyncGenerator[Connection, None]:
    """Get a connection with automatic transaction management."""
    pool = await get_pool()
    async with pool.acquire() as conn:
        async with conn.transaction():
            yield conn


class DatabaseClient:
    """High-level database client for Arbees operations."""

    def __init__(self, pool: Optional[Pool] = None):
        self._pool = pool

    async def _get_pool(self) -> Pool:
        if self._pool is None:
            self._pool = await get_pool()
        return self._pool

    # ==========================================================================
    # Game Operations
    # ==========================================================================

    async def upsert_game(
        self,
        game_id: str,
        sport: str,
        home_team: str,
        away_team: str,
        scheduled_time: Union[datetime, str],
        home_team_abbrev: Optional[str] = None,
        away_team_abbrev: Optional[str] = None,
        venue: Optional[str] = None,
        broadcast: Optional[str] = None,
        status: str = "scheduled",
    ) -> None:
        """Insert or update a game.

        Note: The ON CONFLICT clause intentionally does NOT update status
        to avoid overwriting live game status with 'scheduled'.
        """
        # Convert string to datetime if necessary
        if isinstance(scheduled_time, str):
            scheduled_time = datetime.fromisoformat(scheduled_time.replace("Z", "+00:00"))
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO games (
                game_id, sport, home_team, away_team, scheduled_time,
                home_team_abbrev, away_team_abbrev, venue, broadcast, status
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (game_id) DO UPDATE SET
                venue = COALESCE(EXCLUDED.venue, games.venue),
                broadcast = COALESCE(EXCLUDED.broadcast, games.broadcast),
                scheduled_time = EXCLUDED.scheduled_time,
                updated_at = NOW()
            """,
            game_id, sport, home_team, away_team, scheduled_time,
            home_team_abbrev, away_team_abbrev, venue, broadcast, status
        )

    async def update_game_status(self, game_id: str, status: str) -> None:
        """Update a game's status."""
        pool = await self._get_pool()
        await pool.execute(
            """
            UPDATE games
            SET status = $2, updated_at = NOW()
            WHERE game_id = $1
            """,
            game_id, status
        )

    async def get_live_games(self, sport: Optional[str] = None) -> list[dict]:
        """Get all live games, optionally filtered by sport."""
        pool = await self._get_pool()
        if sport:
            rows = await pool.fetch(
                """
                SELECT * FROM games
                WHERE status IN ('in_progress', 'halftime', 'end_period')
                  AND sport = $1
                ORDER BY scheduled_time
                """,
                sport
            )
        else:
            rows = await pool.fetch(
                """
                SELECT * FROM games
                WHERE status IN ('in_progress', 'halftime', 'end_period')
                ORDER BY scheduled_time
                """
            )
        return [dict(row) for row in rows]

    # ==========================================================================
    # Game State Operations
    # ==========================================================================

    async def insert_game_state(
        self,
        game_id: str,
        sport: str,
        home_score: int,
        away_score: int,
        period: int,
        time_remaining: str,
        status: str,
        possession: Optional[str] = None,
        home_win_prob: Optional[float] = None,
        **kwargs
    ) -> None:
        """Insert a game state snapshot."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO game_states (
                time, game_id, sport, home_score, away_score, period,
                time_remaining, status, possession, home_win_prob, away_win_prob,
                down, yards_to_go, yard_line, is_redzone, strength,
                balls, strikes, outs, runners_on_base
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19
            )
            """,
            game_id, sport, home_score, away_score, period,
            time_remaining, status, possession,
            home_win_prob, 1.0 - home_win_prob if home_win_prob else None,
            kwargs.get('down'), kwargs.get('yards_to_go'),
            kwargs.get('yard_line'), kwargs.get('is_redzone', False),
            kwargs.get('strength'),
            kwargs.get('balls'), kwargs.get('strikes'), kwargs.get('outs'),
            kwargs.get('runners_on_base')
        )

    async def get_latest_game_state(self, game_id: str) -> Optional[dict]:
        """Get the most recent game state for a game."""
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT * FROM get_latest_game_state($1)",
            game_id
        )
        return dict(row) if row else None

    # ==========================================================================
    # Play Operations
    # ==========================================================================

    async def insert_play(
        self,
        play_id: str,
        game_id: str,
        sport: str,
        play_type: str,
        description: str,
        sequence_number: int,
        home_score: int,
        away_score: int,
        period: int,
        time_remaining: str,
        home_win_prob_before: Optional[float] = None,
        home_win_prob_after: Optional[float] = None,
        **kwargs
    ) -> None:
        """Insert a play record."""
        pool = await self._get_pool()
        prob_change = None
        if home_win_prob_before is not None and home_win_prob_after is not None:
            prob_change = home_win_prob_after - home_win_prob_before

        await pool.execute(
            """
            INSERT INTO plays (
                time, play_id, game_id, sport, play_type, description,
                team, player, sequence_number, home_score, away_score,
                period, time_remaining, yards_gained, yard_line, down,
                yards_to_go, is_scoring, is_turnover, shot_distance,
                shot_type, zone, strength,
                home_win_prob_before, home_win_prob_after, prob_change
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                $21, $22, $23, $24, $25
            )
            """,
            play_id, game_id, sport, play_type, description,
            kwargs.get('team'), kwargs.get('player'), sequence_number,
            home_score, away_score, period, time_remaining,
            kwargs.get('yards_gained'), kwargs.get('yard_line'),
            kwargs.get('down'), kwargs.get('yards_to_go'),
            kwargs.get('is_scoring', False), kwargs.get('is_turnover', False),
            kwargs.get('shot_distance'), kwargs.get('shot_type'),
            kwargs.get('zone'), kwargs.get('strength'),
            home_win_prob_before, home_win_prob_after, prob_change
        )

    async def get_recent_plays(
        self,
        game_id: str,
        limit: int = 10
    ) -> list[dict]:
        """Get recent plays for a game."""
        pool = await self._get_pool()
        rows = await pool.fetch(
            "SELECT * FROM get_recent_plays($1, $2)",
            game_id, limit
        )
        return [dict(row) for row in rows]

    # ==========================================================================
    # Market Price Operations
    # ==========================================================================

    async def insert_market_price(
        self,
        market_id: str,
        platform: str,
        yes_bid: float,
        yes_ask: float,
        volume: float = 0,
        liquidity: float = 0,
        game_id: Optional[str] = None,
        market_title: Optional[str] = None,
        **kwargs
    ) -> None:
        """Insert a market price snapshot."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO market_prices (
                time, market_id, platform, game_id, market_title,
                yes_bid, yes_ask, volume, open_interest, liquidity,
                status, last_trade_price
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11
            )
            """,
            market_id, platform, game_id, market_title,
            yes_bid, yes_ask, volume,
            kwargs.get('open_interest', 0), liquidity,
            kwargs.get('status', 'open'), kwargs.get('last_trade_price')
        )

    async def get_latest_market_price(
        self,
        market_id: str,
        platform: str
    ) -> Optional[dict]:
        """Get the most recent price for a market."""
        pool = await self._get_pool()
        row = await pool.fetchrow(
            """
            SELECT * FROM market_prices
            WHERE market_id = $1 AND platform = $2
            ORDER BY time DESC
            LIMIT 1
            """,
            market_id, platform
        )
        return dict(row) if row else None

    # ==========================================================================
    # Trading Signal Operations
    # ==========================================================================

    async def insert_trading_signal(
        self,
        signal_id: str,
        signal_type: str,
        direction: str,
        edge_pct: float,
        game_id: Optional[str] = None,
        sport: Optional[str] = None,
        team: Optional[str] = None,
        model_prob: Optional[float] = None,
        market_prob: Optional[float] = None,
        confidence: Optional[float] = None,
        reason: Optional[str] = None,
        **kwargs
    ) -> None:
        """Insert a trading signal."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO trading_signals (
                time, signal_id, signal_type, game_id, sport, team,
                direction, model_prob, market_prob, edge_pct, confidence,
                platform_buy, platform_sell, buy_price, sell_price,
                liquidity_available, reason, play_id, expires_at
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18
            )
            """,
            signal_id, signal_type, game_id, sport, team,
            direction, model_prob, market_prob, edge_pct, confidence,
            kwargs.get('platform_buy'), kwargs.get('platform_sell'),
            kwargs.get('buy_price'), kwargs.get('sell_price'),
            kwargs.get('liquidity_available', 0), reason,
            kwargs.get('play_id'), kwargs.get('expires_at')
        )

    async def get_active_signals(
        self,
        min_edge: float = 1.0,
        limit: int = 50
    ) -> list[dict]:
        """Get active (non-executed, non-expired) signals."""
        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            SELECT * FROM trading_signals
            WHERE NOT executed
              AND (expires_at IS NULL OR expires_at > NOW())
              AND edge_pct >= $1
            ORDER BY time DESC
            LIMIT $2
            """,
            min_edge, limit
        )
        return [dict(row) for row in rows]

    # ==========================================================================
    # Paper Trade Operations
    # ==========================================================================

    async def insert_paper_trade(
        self,
        trade_id: str,
        platform: str,
        market_id: str,
        side: str,
        entry_price: float,
        size: float,
        entry_time: str,
        signal_id: Optional[str] = None,
        game_id: Optional[str] = None,
        sport: Optional[str] = None,
        signal_type: Optional[str] = None,
        model_prob: Optional[float] = None,
        edge_at_entry: Optional[float] = None,
        kelly_fraction: Optional[float] = None,
        **kwargs
    ) -> None:
        """Insert a paper trade."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO paper_trades (
                time, trade_id, signal_id, game_id, sport, platform,
                market_id, market_title, side, signal_type, entry_price,
                size, model_prob, edge_at_entry, kelly_fraction, entry_time,
                status, entry_fees
            ) VALUES (
                NOW(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, 'open', $16
            )
            """,
            trade_id, signal_id, game_id, sport, platform,
            market_id, kwargs.get('market_title'), side, signal_type,
            entry_price, size, model_prob, edge_at_entry, kelly_fraction,
            entry_time, kwargs.get('entry_fees', 0)
        )

    async def close_paper_trade(
        self,
        trade_id: str,
        exit_price: float,
        exit_time: str,
        outcome: str,
        exit_fees: float = 0
    ) -> None:
        """Close a paper trade."""
        pool = await self._get_pool()

        # Calculate PnL
        row = await pool.fetchrow(
            "SELECT * FROM paper_trades WHERE trade_id = $1 ORDER BY time DESC LIMIT 1",
            trade_id
        )
        if row:
            entry_price = float(row['entry_price'])
            size = float(row['size'])
            side = row['side']
            entry_fees = float(row['entry_fees'] or 0)

            if side == 'buy':
                gross_pnl = size * (exit_price - entry_price)
            else:
                gross_pnl = size * (entry_price - exit_price)

            pnl = gross_pnl - entry_fees - exit_fees
            risk = size * entry_price if side == 'buy' else size * (1 - entry_price)
            pnl_pct = (pnl / risk * 100) if risk > 0 else 0

            await pool.execute(
                """
                UPDATE paper_trades
                SET exit_price = $2, exit_time = $3, status = 'closed',
                    outcome = $4, exit_fees = $5, pnl = $6, pnl_pct = $7
                WHERE trade_id = $1
                """,
                trade_id, exit_price, exit_time, outcome, exit_fees, pnl, pnl_pct
            )

    async def get_open_positions_for_game(self, game_id: str) -> list[dict]:
        """Get all open paper trades for a specific game."""
        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            SELECT trade_id, market_id, market_title, side, entry_price, size,
                   entry_time, entry_fees
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
            ORDER BY time DESC
            """,
            game_id
        )
        return [dict(row) for row in rows]

    async def update_bankroll(
        self,
        pnl_change: float,
        account_name: str = "default"
    ) -> None:
        """Update bankroll balance after a trade settlement."""
        pool = await self._get_pool()
        await pool.execute(
            """
            UPDATE bankroll
            SET
                current_balance = current_balance + $1,
                peak_balance = GREATEST(peak_balance, current_balance + $1),
                trough_balance = LEAST(trough_balance, current_balance + $1),
                updated_at = NOW()
            WHERE account_name = $2
            """,
            pnl_change, account_name
        )

    async def get_performance_stats(
        self,
        days: int = 30,
        signal_type: Optional[str] = None
    ) -> dict:
        """Get aggregate performance statistics."""
        pool = await self._get_pool()

        # Use parameterized queries to prevent SQL injection
        if signal_type:
            query = """
                SELECT
                    COUNT(*) as total_trades,
                    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as winning_trades,
                    SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losing_trades,
                    SUM(pnl) as total_pnl,
                    AVG(pnl) as avg_pnl,
                    AVG(edge_at_entry) as avg_edge
                FROM paper_trades
                WHERE status = 'closed'
                  AND time > NOW() - make_interval(days => $1)
                  AND signal_type = $2
            """
            row = await pool.fetchrow(query, days, signal_type)
        else:
            query = """
                SELECT
                    COUNT(*) as total_trades,
                    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as winning_trades,
                    SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losing_trades,
                    SUM(pnl) as total_pnl,
                    AVG(pnl) as avg_pnl,
                    AVG(edge_at_entry) as avg_edge
                FROM paper_trades
                WHERE status = 'closed'
                  AND time > NOW() - make_interval(days => $1)
            """
            row = await pool.fetchrow(query, days)

        return dict(row) if row else {}
