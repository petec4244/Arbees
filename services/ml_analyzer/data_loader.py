"""
Database queries for loading historical data for ML analysis.

Provides efficient queries to load trades, signals, and game data
from both live and archive tables.
"""

from datetime import date, datetime, timedelta
from typing import Optional
import logging

from arbees_shared.db.connection import get_pool

logger = logging.getLogger(__name__)


class DataLoader:
    """
    Loads historical data from TimescaleDB for ML analysis.

    Supports loading from both archive tables (archived_trades, archived_signals)
    and live tables (paper_trades, trading_signals) for recent data.
    """

    async def load_trades_for_date(self, for_date: date) -> list[dict]:
        """
        Load all closed trades for a specific date.

        Checks both archive tables and live tables.

        Args:
            for_date: The date to load trades for

        Returns:
            List of trade dictionaries
        """
        pool = await get_pool()

        # Try archived trades first
        archived_trades = await pool.fetch("""
            SELECT
                at.trade_id, at.game_id, at.signal_id, at.signal_type,
                at.platform, at.market_type, at.side, at.team,
                at.entry_price, at.exit_price, at.size,
                at.opened_at, at.closed_at,
                at.status, at.outcome, at.pnl, at.pnl_pct,
                at.edge_at_entry, at.model_prob_at_entry, at.market_prob_at_entry,
                at.game_period_at_entry, at.score_diff_at_entry, at.time_remaining_at_entry,
                ag.sport
            FROM archived_trades at
            JOIN archived_games ag ON at.archive_game_id = ag.archive_id
            WHERE DATE(at.closed_at) = $1
            ORDER BY at.closed_at ASC
        """, for_date)

        # Also check live trades (for recent data not yet archived)
        live_trades = await pool.fetch("""
            SELECT
                pt.trade_id, pt.game_id, pt.signal_id, pt.signal_type,
                pt.platform, 'moneyline' as market_type, pt.side, NULL as team,
                pt.entry_price, pt.exit_price, pt.size,
                pt.entry_time as opened_at, pt.exit_time as closed_at,
                pt.status, pt.outcome, pt.pnl, pt.pnl_pct,
                pt.edge_at_entry, pt.model_prob, pt.model_prob as market_prob_at_entry,
                NULL as game_period_at_entry, NULL as score_diff_at_entry, NULL as time_remaining_at_entry,
                pt.sport
            FROM paper_trades pt
            WHERE pt.status = 'closed'
              AND DATE(pt.exit_time) = $1
              AND NOT EXISTS (
                  SELECT 1 FROM archived_trades at WHERE at.trade_id = pt.trade_id
              )
            ORDER BY pt.exit_time ASC
        """, for_date)

        # Combine and deduplicate
        all_trades = [dict(row) for row in archived_trades]
        seen_ids = {t["trade_id"] for t in all_trades}

        for row in live_trades:
            if row["trade_id"] not in seen_ids:
                all_trades.append(dict(row))

        logger.info(f"Loaded {len(all_trades)} trades for {for_date} "
                   f"({len(archived_trades)} archived, {len(live_trades)} live)")
        return all_trades

    async def load_trades_for_range(
        self,
        from_date: date,
        to_date: date,
    ) -> list[dict]:
        """
        Load all closed trades within a date range.

        Args:
            from_date: Start date (inclusive)
            to_date: End date (inclusive)

        Returns:
            List of trade dictionaries
        """
        pool = await get_pool()

        # Load from archived trades
        archived_trades = await pool.fetch("""
            SELECT
                at.trade_id, at.game_id, at.signal_id, at.signal_type,
                at.platform, at.market_type, at.side, at.team,
                at.entry_price, at.exit_price, at.size,
                at.opened_at, at.closed_at,
                at.status, at.outcome, at.pnl, at.pnl_pct,
                at.edge_at_entry, at.model_prob_at_entry, at.market_prob_at_entry,
                at.game_period_at_entry, at.score_diff_at_entry, at.time_remaining_at_entry,
                ag.sport
            FROM archived_trades at
            JOIN archived_games ag ON at.archive_game_id = ag.archive_id
            WHERE DATE(at.closed_at) >= $1 AND DATE(at.closed_at) <= $2
            ORDER BY at.closed_at ASC
        """, from_date, to_date)

        # Also check live trades
        live_trades = await pool.fetch("""
            SELECT
                pt.trade_id, pt.game_id, pt.signal_id, pt.signal_type,
                pt.platform, 'moneyline' as market_type, pt.side, NULL as team,
                pt.entry_price, pt.exit_price, pt.size,
                pt.entry_time as opened_at, pt.exit_time as closed_at,
                pt.status, pt.outcome, pt.pnl, pt.pnl_pct,
                pt.edge_at_entry, pt.model_prob, pt.model_prob as market_prob_at_entry,
                NULL as game_period_at_entry, NULL as score_diff_at_entry, NULL as time_remaining_at_entry,
                pt.sport
            FROM paper_trades pt
            WHERE pt.status = 'closed'
              AND DATE(pt.exit_time) >= $1 AND DATE(pt.exit_time) <= $2
              AND NOT EXISTS (
                  SELECT 1 FROM archived_trades at WHERE at.trade_id = pt.trade_id
              )
            ORDER BY pt.exit_time ASC
        """, from_date, to_date)

        # Combine
        all_trades = [dict(row) for row in archived_trades]
        seen_ids = {t["trade_id"] for t in all_trades}

        for row in live_trades:
            if row["trade_id"] not in seen_ids:
                all_trades.append(dict(row))

        logger.info(f"Loaded {len(all_trades)} trades for {from_date} to {to_date}")
        return all_trades

    async def load_signals_for_date(self, for_date: date) -> list[dict]:
        """
        Load all signals generated on a specific date.

        Args:
            for_date: The date to load signals for

        Returns:
            List of signal dictionaries
        """
        pool = await get_pool()

        # Try archived signals first
        archived_signals = await pool.fetch("""
            SELECT
                asig.signal_id, asig.game_id, asig.signal_type, asig.direction,
                asig.team, asig.market_type,
                asig.model_prob, asig.market_prob, asig.edge_pct, asig.confidence,
                asig.reason, asig.generated_at, asig.expires_at, asig.was_executed,
                ag.sport
            FROM archived_signals asig
            JOIN archived_games ag ON asig.archive_game_id = ag.archive_id
            WHERE DATE(asig.generated_at) = $1
            ORDER BY asig.generated_at ASC
        """, for_date)

        # Also check live signals
        live_signals = await pool.fetch("""
            SELECT
                ts.signal_id, ts.game_id, ts.signal_type, ts.direction,
                ts.team, NULL as market_type,
                ts.model_prob, ts.market_prob, ts.edge_pct, ts.confidence,
                ts.reason, ts.time as generated_at, ts.expires_at, ts.executed as was_executed,
                ts.sport
            FROM trading_signals ts
            WHERE DATE(ts.time) = $1
              AND NOT EXISTS (
                  SELECT 1 FROM archived_signals asig WHERE asig.signal_id = ts.signal_id
              )
            ORDER BY ts.time ASC
        """, for_date)

        # Combine
        all_signals = [dict(row) for row in archived_signals]
        seen_ids = {s["signal_id"] for s in all_signals}

        for row in live_signals:
            if row["signal_id"] not in seen_ids:
                all_signals.append(dict(row))

        logger.info(f"Loaded {len(all_signals)} signals for {for_date}")
        return all_signals

    async def load_historical_trades(
        self,
        days: int = 30,
        min_trades: int = 0,
    ) -> list[dict]:
        """
        Load historical trades for ML training.

        Args:
            days: Number of days to look back
            min_trades: Minimum trades required (returns empty if not met)

        Returns:
            List of trade dictionaries
        """
        to_date = date.today()
        from_date = to_date - timedelta(days=days)

        trades = await self.load_trades_for_range(from_date, to_date)

        if len(trades) < min_trades:
            logger.warning(f"Only {len(trades)} trades found, need {min_trades} for ML")
            return []

        return trades

    async def load_games_for_date(self, for_date: date) -> list[dict]:
        """
        Load all archived games for a specific date.

        Args:
            for_date: The date to load games for

        Returns:
            List of game dictionaries
        """
        pool = await get_pool()

        games = await pool.fetch("""
            SELECT
                archive_id, game_id, sport, home_team, away_team,
                final_home_score, final_away_score,
                scheduled_time, ended_at, archived_at,
                total_trades, winning_trades, losing_trades, push_trades,
                total_pnl, total_signals_generated, total_signals_executed, avg_edge_pct
            FROM archived_games
            WHERE DATE(ended_at) = $1
            ORDER BY ended_at ASC
        """, for_date)

        return [dict(row) for row in games]

    async def get_performance_by_sport(self, days: int = 30) -> dict[str, dict]:
        """
        Get aggregated performance by sport.

        Args:
            days: Number of days to look back

        Returns:
            Dictionary mapping sport -> performance metrics
        """
        pool = await get_pool()

        rows = await pool.fetch("""
            SELECT
                sport,
                COUNT(*) as games,
                SUM(total_trades) as trades,
                SUM(winning_trades) as wins,
                SUM(losing_trades) as losses,
                SUM(total_pnl) as pnl,
                AVG(CASE WHEN total_trades > 0
                    THEN winning_trades::float / total_trades
                    ELSE 0 END) as avg_win_rate
            FROM archived_games
            WHERE ended_at > NOW() - INTERVAL '%s days'
            GROUP BY sport
        """ % days)

        return {
            row["sport"]: {
                "games": row["games"],
                "trades": int(row["trades"] or 0),
                "wins": int(row["wins"] or 0),
                "losses": int(row["losses"] or 0),
                "pnl": float(row["pnl"] or 0),
                "win_rate": float(row["avg_win_rate"] or 0),
            }
            for row in rows
        }

    async def get_performance_by_signal_type(self, days: int = 30) -> dict[str, dict]:
        """
        Get aggregated performance by signal type.

        Args:
            days: Number of days to look back

        Returns:
            Dictionary mapping signal_type -> performance metrics
        """
        pool = await get_pool()

        rows = await pool.fetch("""
            SELECT
                signal_type,
                COUNT(*) as trades,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
                SUM(pnl) as pnl,
                AVG(edge_at_entry) as avg_edge
            FROM archived_trades
            WHERE closed_at > NOW() - INTERVAL '%s days'
            GROUP BY signal_type
        """ % days)

        result = {}
        for row in rows:
            signal_type = row["signal_type"] or "unknown"
            trades = int(row["trades"])
            wins = int(row["wins"])

            result[signal_type] = {
                "trades": trades,
                "wins": wins,
                "losses": int(row["losses"]),
                "pnl": float(row["pnl"] or 0),
                "win_rate": wins / trades if trades > 0 else 0,
                "avg_edge": float(row["avg_edge"] or 0),
            }

        return result

    async def get_performance_by_edge_bucket(self, days: int = 30) -> dict[str, dict]:
        """
        Get performance broken down by edge range.

        Args:
            days: Number of days to look back

        Returns:
            Dictionary mapping edge_range -> performance metrics
        """
        pool = await get_pool()

        rows = await pool.fetch("""
            SELECT
                CASE
                    WHEN edge_at_entry < 1 THEN '0-1%'
                    WHEN edge_at_entry < 2 THEN '1-2%'
                    WHEN edge_at_entry < 3 THEN '2-3%'
                    WHEN edge_at_entry < 5 THEN '3-5%'
                    ELSE '5%+'
                END as edge_range,
                COUNT(*) as trades,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                SUM(pnl) as pnl
            FROM archived_trades
            WHERE closed_at > NOW() - INTERVAL '%s days'
              AND edge_at_entry IS NOT NULL
            GROUP BY edge_range
            ORDER BY MIN(edge_at_entry)
        """ % days)

        result = {}
        for row in rows:
            trades = int(row["trades"])
            wins = int(row["wins"])

            result[row["edge_range"]] = {
                "trades": trades,
                "wins": wins,
                "pnl": float(row["pnl"] or 0),
                "win_rate": wins / trades if trades > 0 else 0,
            }

        return result

    async def get_current_parameters(self) -> dict[str, float]:
        """
        Get current trading parameters from config or defaults.

        Returns:
            Dictionary of parameter name -> value
        """
        # TODO: Load from configuration table if implemented
        return {
            "min_edge_pct": 2.0,
            "max_position_pct": 10.0,
            "kelly_fraction": 0.25,
            "take_profit_pct": 3.0,
            "stop_loss_pct": 5.0,
        }
