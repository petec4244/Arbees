"""
PositionTracker service - Phase 1 split from PositionManager.

Responsibilities:
- Subscribe to ExecutionResult messages (positions opened)
- Track open positions in memory and database
- Monitor positions for exit conditions (take-profit, stop-loss)
- Handle game endings (forced settlement)
- Emit PositionUpdate messages for UI/monitoring
"""

import asyncio
import logging
import os
from datetime import datetime, timezone, timedelta
from typing import Optional

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.market import Platform
from arbees_shared.models.trade import PaperTrade, TradeSide, TradeStatus, TradeOutcome
from arbees_shared.models.signal import SignalType
from arbees_shared.models.execution import (
    ExecutionResult,
    ExecutionStatus,
    ExecutionSide,
    PositionUpdate,
    PositionState,
)
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from arbees_shared.utils.trace_logger import trace_log, TraceContext
from arbees_shared.utils.team_validator import TeamValidator, TeamMatchResult
from markets.paper.engine import PaperTradingEngine

logger = logging.getLogger(__name__)


# Sport-specific stop-loss thresholds
SPORT_STOP_LOSS_DEFAULTS: dict[str, float] = {
    "NBA": 3.0,
    "NCAAB": 3.0,
    "NFL": 5.0,
    "NCAAF": 5.0,
    "NHL": 7.0,
    "MLB": 6.0,
    "MLS": 7.0,
    "SOCCER": 7.0,
    "TENNIS": 4.0,
    "MMA": 8.0,
}


class PositionTracker:
    """
    Tracks open positions and monitors for exit conditions.
    """

    def __init__(
        self,
        take_profit_pct: float = 3.0,
        default_stop_loss_pct: float = 5.0,
        exit_check_interval: float = 1.0,
        sport_stop_loss: Optional[dict[str, float]] = None,
        initial_bankroll: float = 1000.0,
        min_hold_seconds: float = 10.0,
        # Hardening options
        price_staleness_ttl: float = 30.0,  # Max age of market price in seconds
        require_valid_book: bool = True,  # Reject exits on empty/pathological books
        debounce_exit_checks: int = 0,  # Require N consecutive checks before exit (0=disabled)
        exit_team_match_min_confidence: float = 0.7,  # Min confidence for exit price team match
    ):
        self.take_profit_pct = take_profit_pct
        self.default_stop_loss_pct = default_stop_loss_pct
        self.exit_check_interval = exit_check_interval
        self.sport_stop_loss = sport_stop_loss or SPORT_STOP_LOSS_DEFAULTS.copy()
        self.initial_bankroll = initial_bankroll
        self.min_hold_seconds = min_hold_seconds

        # Hardening
        self.price_staleness_ttl = price_staleness_ttl
        self.require_valid_book = require_valid_book
        self.debounce_exit_checks = debounce_exit_checks
        self.exit_team_match_min_confidence = exit_team_match_min_confidence

        # Team name validator for exit price selection
        self.team_validator = TeamValidator()

        # Track consecutive exit triggers per trade for debounce
        self._exit_trigger_counts: dict[str, int] = {}

        # Connections
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.paper_engine: Optional[PaperTradingEngine] = None

        # State
        self._running = False
        self._positions_opened = 0
        self._positions_closed = 0

        # Game cooldowns for signal processor communication
        self._game_cooldowns: dict[str, tuple[datetime, bool]] = {}

        # Heartbeat publisher
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

    async def start(self) -> None:
        """Start the position tracker."""
        logger.info("Starting PositionTracker")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Initialize paper engine for position management
        self.paper_engine = PaperTradingEngine(
            initial_bankroll=self.initial_bankroll,
            db_client=self.db,
            redis_bus=self.redis,
        )
        await self._load_bankroll()
        await self._load_open_positions()

        self._running = True

        # Subscribe to execution results
        await self.redis.subscribe(Channel.EXECUTION_RESULTS.value, self._handle_execution_result)

        # Subscribe to game endings
        await self.redis.subscribe("games:ended", self._handle_game_ended)

        # Start listening
        asyncio.create_task(self.redis.start_listening())

        # Start position monitoring loop
        asyncio.create_task(self._position_monitor_loop())

        # Start orphaned position sweep loop (backup for missed game:ended events)
        asyncio.create_task(self._orphan_sweep_loop())

        # Start heartbeat
        asyncio.create_task(self._heartbeat_loop())

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="position_tracker",
            instance_id=os.environ.get("HOSTNAME", "position-tracker-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
        })

        logger.info(
            f"PositionTracker started (take_profit={self.take_profit_pct}%, "
            f"default_stop_loss={self.default_stop_loss_pct}%)"
        )

    async def stop(self) -> None:
        """Stop the position tracker."""
        logger.info("Stopping PositionTracker")
        self._running = False

        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        if self.paper_engine:
            await self._save_bankroll()

        if self.redis:
            await self.redis.disconnect()

        logger.info("PositionTracker stopped")

    async def _load_bankroll(self) -> None:
        """Load bankroll state from database."""
        if not self.paper_engine:
            return

        pool = await get_pool()
        row = await pool.fetchrow("""
            SELECT * FROM bankroll WHERE account_name = 'default'
        """)

        if row:
            from arbees_shared.models.trade import Bankroll
            piggybank = float(row.get("piggybank_balance") or 0.0)
            self.paper_engine._bankroll = Bankroll(
                initial_balance=float(row["initial_balance"]),
                current_balance=float(row["current_balance"]),
                piggybank_balance=piggybank,
                peak_balance=float(row["peak_balance"]),
                trough_balance=float(row["trough_balance"]),
            )
            logger.info(f"Loaded bankroll: ${self.paper_engine._bankroll.current_balance:.2f}")

    async def _save_bankroll(self) -> None:
        """Save bankroll state to database."""
        if not self.paper_engine:
            return

        pool = await get_pool()
        bankroll = self.paper_engine._bankroll
        await pool.execute("""
            UPDATE bankroll
            SET current_balance = $1, piggybank_balance = $2, peak_balance = $3, trough_balance = $4, updated_at = NOW()
            WHERE account_name = 'default'
        """, bankroll.current_balance, bankroll.piggybank_balance, bankroll.peak_balance, bankroll.trough_balance)

    async def _load_open_positions(self) -> None:
        """Load open positions from database into paper engine."""
        if not self.paper_engine:
            return

        pool = await get_pool()
        rows = await pool.fetch("""
            SELECT * FROM paper_trades WHERE status = 'open'
        """)

        for row in rows:
            from arbees_shared.models.game import Sport
            game_id = row.get("game_id")
            if not game_id:
                # Can't manage exits/settlement without a game_id
                continue

            # `PaperTrade` model is strict; populate required fields with safe defaults if missing
            signal_type_val = row.get("signal_type") or SignalType.WIN_PROB_SHIFT.value
            model_prob_val = float(row.get("model_prob") or 0.5)
            edge_val = float(row.get("edge_at_entry") or 0.0)
            kelly_val = float(row.get("kelly_fraction") or 0.0)

            trade = PaperTrade(
                trade_id=row["trade_id"],
                signal_id=row.get("signal_id") or "",
                game_id=game_id,
                sport=Sport(row["sport"]) if row.get("sport") else Sport.NBA,
                platform=Platform(row["platform"]),
                market_id=row["market_id"],
                market_title=row.get("market_title") or "",
                side=TradeSide(row["side"]),
                signal_type=SignalType(signal_type_val),
                entry_price=float(row["entry_price"]),
                exit_price=float(row["exit_price"]) if row.get("exit_price") is not None else None,
                size=float(row["size"]),
                model_prob=model_prob_val,
                edge_at_entry=edge_val,
                kelly_fraction=kelly_val,
                entry_time=row.get("entry_time") or row["time"],
                exit_time=row.get("exit_time"),
                status=TradeStatus.OPEN,
                outcome=TradeOutcome(row.get("outcome") or TradeOutcome.PENDING.value),
                entry_fees=float(row.get("entry_fees") or 0.0),
                exit_fees=float(row.get("exit_fees") or 0.0),
            )

            # Store in the engine's canonical trade list
            self.paper_engine._trades.append(trade)

        logger.info(f"Loaded {len(rows)} open positions from database")

    async def _add_trade_from_db(self, trade_id: str) -> None:
        """Fetch a paper_trades row and add it to the in-memory engine state."""
        if not self.paper_engine:
            return

        # Avoid duplicates if we already have it
        for t in self.paper_engine._trades:
            if t.trade_id == trade_id and t.status == TradeStatus.OPEN:
                return

        pool = await get_pool()
        row = await pool.fetchrow(
            """
            SELECT * FROM paper_trades
            WHERE trade_id = $1
            ORDER BY time DESC
            LIMIT 1
            """,
            trade_id,
        )
        if not row:
            return

        from arbees_shared.models.game import Sport

        game_id = row.get("game_id")
        if not game_id:
            return

        signal_type_val = row.get("signal_type") or SignalType.WIN_PROB_SHIFT.value
        model_prob_val = float(row.get("model_prob") or 0.5)
        edge_val = float(row.get("edge_at_entry") or 0.0)
        kelly_val = float(row.get("kelly_fraction") or 0.0)

        trade = PaperTrade(
            trade_id=row["trade_id"],
            signal_id=row.get("signal_id") or "",
            game_id=game_id,
            sport=Sport(row["sport"]) if row.get("sport") else Sport.NBA,
            platform=Platform(row["platform"]),
            market_id=row["market_id"],
            market_title=row.get("market_title") or "",
            side=TradeSide(row["side"]),
            signal_type=SignalType(signal_type_val),
            entry_price=float(row["entry_price"]),
            exit_price=float(row["exit_price"]) if row.get("exit_price") is not None else None,
            size=float(row["size"]),
            model_prob=model_prob_val,
            edge_at_entry=edge_val,
            kelly_fraction=kelly_val,
            entry_time=row.get("entry_time") or row["time"],
            exit_time=row.get("exit_time"),
            status=TradeStatus(row.get("status") or TradeStatus.OPEN.value),
            outcome=TradeOutcome(row.get("outcome") or TradeOutcome.PENDING.value),
            entry_fees=float(row.get("entry_fees") or 0.0),
            exit_fees=float(row.get("exit_fees") or 0.0),
        )
        if trade.status == TradeStatus.OPEN:
            self.paper_engine._trades.append(trade)

    async def _handle_execution_result(self, data: dict) -> None:
        """Handle incoming execution result."""
        try:
            result = ExecutionResult(**data)
            logger.info(
                f"Received ExecutionResult: {result.request_id} "
                f"status={result.status.value}"
            )

            if result.status == ExecutionStatus.FILLED:
                self._positions_opened += 1

                # Keep local engine state in sync (so exit monitoring works)
                trade_id = result.order_id or result.request_id
                await self._add_trade_from_db(trade_id)

                # Log position opened with trace
                trace_log(
                    service="position_tracker",
                    event="position_opened",
                    trace_id=result.idempotency_key,
                    signal_id=result.signal_id,
                    trade_id=trade_id,
                    game_id=result.game_id,
                    sport=result.sport.value if result.sport else None,
                    platform=result.platform.value if result.platform else None,
                    market_id=result.market_id,
                    contract_team=result.contract_team,
                    side=result.side.value if result.side else None,
                    entry_price=result.avg_price,
                    size=result.filled_qty,
                    edge_pct=result.edge_pct,
                )

                # Emit position update
                await self._emit_position_update(
                    result,
                    PositionState.OPEN,
                )

                logger.info(
                    f"Position opened: {result.game_id} {result.contract_team} "
                    f"@ {result.avg_price:.3f} x ${result.filled_qty:.2f}"
                )

        except Exception as e:
            logger.error(f"Error handling execution result: {e}", exc_info=True)

    async def _handle_game_ended(self, data: dict) -> None:
        """Handle game ending - settle any open positions."""
        game_id = data.get("game_id")
        if not game_id or not self.paper_engine:
            return

        try:
            home_score = data.get("home_score", 0)
            away_score = data.get("away_score", 0)
            home_team = data.get("home_team", "")
            away_team = data.get("away_team", "")
            home_won = home_score > away_score

            logger.info(
                f"Game {game_id} ended: {home_team} {home_score} - {away_score} {away_team}"
            )

            # Find open trades for this game
            open_trades = self.paper_engine.get_open_trades()
            game_trades = [t for t in open_trades if t.game_id == game_id]

            for trade in game_trades:
                # Determine if trade was on winning team
                title = trade.market_title.lower()
                trade_on_home = self._teams_match(home_team, title) if home_team else False
                trade_on_away = self._teams_match(away_team, title) if away_team else False

                if trade_on_home:
                    team_won = home_won
                elif trade_on_away:
                    team_won = not home_won
                else:
                    team_won = home_won  # Fallback

                # Settlement price
                exit_price = 1.0 if team_won else 0.0

                logger.info(
                    f"Settling trade {trade.trade_id}: {trade.side.value} "
                    f"(team_won={team_won}, exit_price={exit_price:.2f})"
                )

                closed_trade = await self.paper_engine.close_trade(
                    trade, exit_price, is_game_settlement=True
                )
                self._positions_closed += 1

                # Record cooldown
                was_win = closed_trade.outcome == TradeOutcome.WIN
                self._game_cooldowns[game_id] = (datetime.now(timezone.utc), was_win)

        except Exception as e:
            logger.error(f"Error handling game ended: {e}", exc_info=True)

    def _teams_match(self, team1: str, team2: str) -> bool:
        """Check if two team names refer to the same team."""
        if not team1 or not team2:
            return False

        t1 = team1.lower().strip()
        t2 = team2.lower().strip()

        if t1 == t2:
            return True
        if t1 in t2 or t2 in t1:
            return True

        t1_words = t1.split()
        t2_words = t2.split()
        if t1_words and t2_words:
            if t1_words[-1] == t2_words[-1]:
                return True

        return False

    async def _position_monitor_loop(self) -> None:
        """Monitor open positions for exit conditions."""
        while self._running:
            try:
                await self._check_exit_conditions()
            except Exception as e:
                logger.error(f"Position monitor error: {e}")
            await asyncio.sleep(self.exit_check_interval)

    async def _orphan_sweep_loop(self) -> None:
        """Periodically sweep for orphaned positions (games ended but positions still open).
        
        This is a backup mechanism for when games:ended Redis messages are missed.
        Runs every 5 minutes.
        """
        # Wait a bit before starting the first sweep
        await asyncio.sleep(60)
        
        while self._running:
            try:
                await self._sweep_orphaned_positions()
            except Exception as e:
                logger.error(f"Orphan sweep error: {e}", exc_info=True)
            # Run every 5 minutes
            await asyncio.sleep(300)

    async def _sweep_orphaned_positions(self) -> None:
        """Check for open positions where the game has ended in the database."""
        if not self.paper_engine or not self.db:
            return

        open_trades = self.paper_engine.get_open_trades()
        if not open_trades:
            return

        # Get unique game IDs from open positions
        game_ids = list(set(t.game_id for t in open_trades if t.game_id))
        if not game_ids:
            return

        # Query database to check which games have ended
        try:
            pool = await get_pool()
            rows = await pool.fetch(
                """
                SELECT game_id, home_team, away_team, final_home_score, final_away_score, status
                FROM games
                WHERE game_id = ANY($1)
                  AND status IN ('final', 'complete', 'completed')
                """,
                game_ids,
            )

            ended_games = {row["game_id"]: dict(row) for row in rows}

            if ended_games:
                logger.info(f"Orphan sweep: found {len(ended_games)} ended games with open positions")

            for trade in open_trades:
                if trade.game_id in ended_games:
                    game_info = ended_games[trade.game_id]
                    
                    home_score = game_info.get("final_home_score") or 0
                    away_score = game_info.get("final_away_score") or 0
                    home_team = game_info.get("home_team") or ""
                    away_team = game_info.get("away_team") or ""
                    home_won = home_score > away_score

                    logger.warning(
                        f"Orphan sweep: settling orphaned position {trade.trade_id} "
                        f"for ended game {trade.game_id} ({home_team} {home_score} - {away_score} {away_team})"
                    )

                    # Determine if trade was on winning team
                    title = (trade.market_title or "").lower()
                    trade_on_home = self._teams_match(home_team, title) if home_team else False
                    trade_on_away = self._teams_match(away_team, title) if away_team else False

                    if trade_on_home:
                        team_won = home_won
                    elif trade_on_away:
                        team_won = not home_won
                    else:
                        # Fallback: assume home team
                        team_won = home_won

                    exit_price = 1.0 if team_won else 0.0

                    logger.info(
                        f"Orphan sweep: closing trade {trade.trade_id} - "
                        f"side={trade.side.value}, team_won={team_won}, exit_price={exit_price:.2f}"
                    )

                    await self.paper_engine.close_trade(trade, exit_price, is_game_settlement=True)
                    self._positions_closed += 1

                    trace_log(
                        service="position_tracker",
                        event="orphan_position_settled",
                        trade_id=trade.trade_id,
                        game_id=trade.game_id,
                        exit_price=exit_price,
                        team_won=team_won,
                    )

        except Exception as e:
            logger.error(f"Error in orphan sweep DB query: {e}", exc_info=True)

    async def _check_exit_conditions(self) -> None:
        """Check all open positions against current market prices."""
        if not self.paper_engine:
            return

        open_trades = self.paper_engine.get_open_trades()
        if not open_trades:
            return

        now = datetime.now(timezone.utc)

        for trade in open_trades:
            # Don't immediately stop out on spread / stale quotes right after entry
            entry_time = trade.entry_time
            if entry_time.tzinfo is None:
                entry_time = entry_time.replace(tzinfo=timezone.utc)
            hold_duration = (now - entry_time).total_seconds()
            if hold_duration < self.min_hold_seconds:
                continue

            prices = await self._get_current_prices_with_metadata(trade)
            if prices is None:
                # No price data - skip exit check
                trace_log(
                    service="position_tracker",
                    event="exit_check_skipped",
                    trade_id=trade.trade_id,
                    game_id=trade.game_id,
                    reason="no_price_data",
                )
                continue

            mark_price, exec_price, price_age_ms, bid, ask = prices

            # HARDENING: Staleness gate - don't exit on stale quotes
            if price_age_ms is not None and price_age_ms > self.price_staleness_ttl * 1000:
                trace_log(
                    service="position_tracker",
                    event="exit_check_skipped",
                    trade_id=trade.trade_id,
                    game_id=trade.game_id,
                    reason="stale_price",
                    price_age_ms=price_age_ms,
                    staleness_ttl_ms=self.price_staleness_ttl * 1000,
                )
                continue

            # HARDENING: Empty/pathological book gate
            if self.require_valid_book:
                if bid <= 0.0 and ask >= 1.0:
                    trace_log(
                        service="position_tracker",
                        event="exit_check_skipped",
                        trade_id=trade.trade_id,
                        game_id=trade.game_id,
                        reason="pathological_book",
                        yes_bid=bid,
                        yes_ask=ask,
                    )
                    continue
                # Also skip if spread is pathological (> 50%)
                spread = ask - bid
                if spread > 0.5:
                    trace_log(
                        service="position_tracker",
                        event="exit_check_skipped",
                        trade_id=trade.trade_id,
                        game_id=trade.game_id,
                        reason="extreme_spread",
                        spread=spread,
                        yes_bid=bid,
                        yes_ask=ask,
                    )
                    continue

            sport = trade.sport.value if trade.sport else ""
            should_exit, reason = self._evaluate_exit(trade, mark_price, sport)

            # Log each exit check (sampled to avoid spam - only log when significant)
            price_move = self._calculate_price_move(trade, mark_price)
            stop_loss_pct = self._get_stop_loss_for_sport(sport)

            # Log exit evaluation details
            trace_log(
                service="position_tracker",
                event="exit_check",
                trade_id=trade.trade_id,
                game_id=trade.game_id,
                side=trade.side.value,
                entry_price=trade.entry_price,
                mark_price=mark_price,
                exec_price=exec_price,
                price_move=price_move,
                hold_duration_s=hold_duration,
                take_profit_threshold=self.take_profit_pct / 100,
                stop_loss_threshold=stop_loss_pct / 100,
                decision="exit" if should_exit else "hold",
                exit_reason=reason if should_exit else None,
                price_age_ms=price_age_ms,
            )

            if should_exit:
                # HARDENING: Debounce - require N consecutive exit triggers
                if self.debounce_exit_checks > 0:
                    count = self._exit_trigger_counts.get(trade.trade_id, 0) + 1
                    self._exit_trigger_counts[trade.trade_id] = count

                    if count < self.debounce_exit_checks:
                        logger.debug(
                            f"Exit debounce {trade.trade_id}: {count}/{self.debounce_exit_checks}"
                        )
                        continue
                    # Reset counter on actual exit
                    self._exit_trigger_counts.pop(trade.trade_id, None)
                else:
                    # Clear any stale counter
                    self._exit_trigger_counts.pop(trade.trade_id, None)

                await self._execute_exit(trade, exec_price, reason, mark_price=mark_price)
            else:
                # Reset debounce counter if condition no longer met
                self._exit_trigger_counts.pop(trade.trade_id, None)

    def _get_stop_loss_for_sport(self, sport: str) -> float:
        """Get stop-loss threshold for a sport."""
        sport_upper = sport.upper() if sport else ""
        return self.sport_stop_loss.get(sport_upper, self.default_stop_loss_pct)

    def _calculate_price_move(self, trade: PaperTrade, current_price: float) -> float:
        """Calculate price movement for logging (positive = in our favor)."""
        if trade.side == TradeSide.BUY:
            # BUY: price going up is good
            return current_price - trade.entry_price
        else:
            # SELL: price going down is good
            return trade.entry_price - current_price

    def _evaluate_exit(
        self,
        trade: PaperTrade,
        current_price: float,
        sport: str
    ) -> tuple[bool, str]:
        """Evaluate if position should be exited."""
        entry_price = trade.entry_price
        stop_loss_pct = self._get_stop_loss_for_sport(sport)

        if trade.side == TradeSide.BUY:
            price_move = current_price - entry_price
            if price_move >= self.take_profit_pct / 100:
                return True, f"take_profit: +{price_move*100:.1f}%"
            if price_move <= -stop_loss_pct / 100:
                return True, f"stop_loss: {price_move*100:.1f}%"
        else:
            price_move = entry_price - current_price
            if price_move >= self.take_profit_pct / 100:
                return True, f"take_profit: +{price_move*100:.1f}%"
            if price_move <= -stop_loss_pct / 100:
                return True, f"stop_loss: {price_move*100:.1f}%"

        return False, ""

    def _extract_contract_team_candidates(self, market_title: str) -> list[str]:
        """Best-effort extraction of contract team for Polymarket rows."""
        title = (market_title or "").strip()
        lower = title.lower()
        if " to win" in lower:
            base = title[: lower.rfind(" to win")].strip()
        else:
            base = title

        parts = [p for p in base.replace("@", " ").split() if p]
        candidates = []
        if base:
            candidates.append(base)
        if len(parts) >= 2:
            candidates.append(" ".join(parts[-2:]))
        if len(parts) >= 1:
            candidates.append(parts[-1])

        # Deduplicate preserving order
        seen = set()
        out = []
        for c in candidates:
            key = c.lower()
            if key and key not in seen:
                seen.add(key)
                out.append(c)
        return out

    async def _get_current_prices(self, trade: PaperTrade) -> Optional[tuple[float, float]]:
        """Get (mark_price, executable_price) for an open trade."""
        result = await self._get_current_prices_with_metadata(trade)
        if result is None:
            return None
        mark, exec_px, _, _, _ = result
        return mark, exec_px

    def _extract_entry_team(self, trade: PaperTrade) -> Optional[str]:
        """
        Extract the entry team from the trade's market_title.

        Paper trades have market_title like "{contract_team} to win".
        """
        title = (trade.market_title or "").strip()
        lower = title.lower()

        # Try to parse "{team} to win" pattern
        if " to win" in lower:
            team = title[: lower.rfind(" to win")].strip()
            if team:
                return team

        # Fallback: use the whole title if it's not empty
        if title:
            return title

        return None

    async def _get_current_prices_with_metadata(
        self, trade: PaperTrade
    ) -> Optional[tuple[float, float, Optional[float], float, float]]:
        """
        Get pricing data with metadata for an open trade.

        Uses confidence-scored team matching to prevent wrong-team price selection
        at exit.

        Returns:
            Tuple of (mark_price, exec_price, price_age_ms, yes_bid, yes_ask)
            or None if no price available or team mismatch
        """
        pool = await get_pool()

        # Extract entry team from trade
        entry_team = self._extract_entry_team(trade)

        # For Polymarket, we need to validate team match to prevent wrong-team exits
        if trade.platform == Platform.POLYMARKET and entry_team:
            # Fetch recent prices with contract_team for this market
            rows = await pool.fetch(
                """
                SELECT yes_bid, yes_ask, time, contract_team
                FROM market_prices
                WHERE platform = $1
                  AND market_id = $2
                  AND contract_team IS NOT NULL
                  AND time > NOW() - INTERVAL '2 minutes'
                ORDER BY time DESC
                LIMIT 5
                """,
                trade.platform.value,
                trade.market_id,
            )

            # Score each candidate
            best_match: Optional[dict] = None
            best_confidence = 0.0
            best_result: Optional[TeamMatchResult] = None
            candidates_scored: list[dict] = []

            for row in rows:
                contract_team = row["contract_team"]
                match_result = self.team_validator.validate_match(entry_team, contract_team)

                candidates_scored.append({
                    "contract_team": contract_team,
                    "confidence": match_result.confidence,
                    "method": match_result.method,
                    "is_match": match_result.is_match,
                })

                if match_result.is_match and match_result.confidence > best_confidence:
                    best_confidence = match_result.confidence
                    best_result = match_result
                    best_match = dict(row)

            # Require minimum confidence
            if not best_match or best_confidence < self.exit_team_match_min_confidence:
                trace_log(
                    service="position_tracker",
                    event="exit_check_skipped",
                    trade_id=trade.trade_id,
                    game_id=trade.game_id,
                    reason="team_mismatch",
                    entry_team=entry_team,
                    best_confidence=best_confidence,
                    min_confidence_threshold=self.exit_team_match_min_confidence,
                    candidates_found=len(rows),
                    candidates_scored=candidates_scored,
                )
                logger.debug(
                    f"Exit skipped for {trade.trade_id}: team mismatch "
                    f"(entry_team='{entry_team}', best_confidence={best_confidence:.0%})"
                )
                return None

            # We have a confident match
            bid = float(best_match["yes_bid"])
            ask = float(best_match["yes_ask"])

            # Calculate price age
            price_age_ms = None
            if best_match.get("time"):
                price_time = best_match["time"]
                if price_time.tzinfo is None:
                    price_time = price_time.replace(tzinfo=timezone.utc)
                price_age_ms = (datetime.now(timezone.utc) - price_time).total_seconds() * 1000

            # Mark-to-market uses mid; execution uses bid/ask
            mark = (bid + ask) / 2.0
            exec_px = bid if trade.side == TradeSide.BUY else ask

            # Log successful validation
            trace_log(
                service="position_tracker",
                event="exit_price_validated",
                trade_id=trade.trade_id,
                game_id=trade.game_id,
                entry_team=entry_team,
                exit_contract_team=best_match["contract_team"],
                confidence=best_confidence,
                match_method=best_result.method if best_result else None,
                yes_bid=bid,
                yes_ask=ask,
                exec_price=exec_px,
                price_age_ms=price_age_ms,
            )

            return mark, exec_px, price_age_ms, bid, ask

        # Non-Polymarket or no entry_team: use simple fallback
        row = await pool.fetchrow(
            """
            SELECT yes_bid, yes_ask, time, contract_team
            FROM market_prices
            WHERE platform = $1 AND market_id = $2
            ORDER BY time DESC
            LIMIT 1
            """,
            trade.platform.value,
            trade.market_id,
        )

        if not row:
            return None

        bid = float(row["yes_bid"])
        ask = float(row["yes_ask"])

        # Calculate price age
        price_age_ms = None
        if row.get("time"):
            price_time = row["time"]
            if price_time.tzinfo is None:
                price_time = price_time.replace(tzinfo=timezone.utc)
            price_age_ms = (datetime.now(timezone.utc) - price_time).total_seconds() * 1000

        # Mark-to-market should use mid; execution uses bid/ask
        mark = (bid + ask) / 2.0
        exec_px = bid if trade.side == TradeSide.BUY else ask
        return mark, exec_px, price_age_ms, bid, ask

    async def _execute_exit(
        self,
        trade: PaperTrade,
        current_price: float,
        reason: str,
        mark_price: Optional[float] = None,
    ) -> None:
        """Execute position exit."""
        logger.info(
            f"EXIT {trade.side.value} {trade.game_id}: "
            f"entry={trade.entry_price*100:.1f}% -> current={current_price*100:.1f}% "
            f"({reason})"
        )

        closed_trade = await self.paper_engine.close_trade(
            trade, current_price, already_executable=True
        )
        self._positions_closed += 1

        # Log exit with trace
        trace_log(
            service="position_tracker",
            event="position_closed",
            trade_id=closed_trade.trade_id,
            game_id=closed_trade.game_id,
            side=closed_trade.side.value,
            entry_price=closed_trade.entry_price,
            exit_price=current_price,
            mark_price=mark_price,
            pnl=closed_trade.pnl,
            outcome=closed_trade.outcome.value,
            exit_reason=reason,
            hold_duration_s=(closed_trade.exit_time - closed_trade.entry_time).total_seconds() if closed_trade.exit_time and closed_trade.entry_time else None,
        )

        logger.info(
            f"TRADE_CLOSED | trade_id={closed_trade.trade_id} | "
            f"pnl=${closed_trade.pnl:.2f} | outcome={closed_trade.outcome.value}"
        )

        # Record cooldown
        was_win = closed_trade.outcome == TradeOutcome.WIN
        self._game_cooldowns[closed_trade.game_id] = (datetime.now(timezone.utc), was_win)

    async def _emit_position_update(
        self,
        result: ExecutionResult,
        state: PositionState,
        exit_price: Optional[float] = None,
        exit_reason: Optional[str] = None,
        pnl: float = 0.0,
    ) -> None:
        """Emit a position update message."""
        update = PositionUpdate(
            position_id=result.request_id,
            trade_id=result.order_id or result.request_id,
            state=state,
            game_id=result.game_id,
            sport=result.sport,
            platform=result.platform,
            market_id=result.market_id,
            contract_team=result.contract_team,
            side=result.side,
            entry_price=result.avg_price,
            size=result.filled_qty,
            fees_paid=result.fees,
            exit_price=exit_price,
            exit_reason=exit_reason,
            realized_pnl=pnl,
            opened_at=result.executed_at,
        )

        await self.redis.publish(Channel.POSITION_UPDATES.value, update)

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                open_count = len(self.paper_engine.get_open_trades()) if self.paper_engine else 0
                bankroll = self.paper_engine._bankroll if self.paper_engine else None

                logger.info(
                    f"PositionTracker: {open_count} open, "
                    f"{self._positions_opened} opened, {self._positions_closed} closed, "
                    f"bankroll=${bankroll.current_balance:.2f}" if bankroll else ""
                )

                # Update health monitoring heartbeat
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.update_metrics({
                        "positions_open": float(open_count),
                        "positions_opened_total": float(self._positions_opened),
                        "positions_closed_total": float(self._positions_closed),
                        "bankroll": float(bankroll.current_balance) if bankroll else 0.0,
                    })

                if self.paper_engine:
                    await self._save_bankroll()
            except Exception as e:
                logger.warning(f"Heartbeat error: {e}")
            await asyncio.sleep(30)


async def main():
    """Main entry point."""
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    # Build sport-specific stop-loss overrides
    sport_stop_loss = SPORT_STOP_LOSS_DEFAULTS.copy()
    for sport in SPORT_STOP_LOSS_DEFAULTS:
        env_key = f"STOP_LOSS_{sport}"
        if env_val := os.environ.get(env_key):
            sport_stop_loss[sport] = float(env_val)

    tracker = PositionTracker(
        take_profit_pct=float(os.environ.get("TAKE_PROFIT_PCT", "3.0")),
        default_stop_loss_pct=float(os.environ.get("DEFAULT_STOP_LOSS_PCT", "5.0")),
        exit_check_interval=float(os.environ.get("EXIT_CHECK_INTERVAL", "1.0")),
        sport_stop_loss=sport_stop_loss,
        initial_bankroll=float(os.environ.get("INITIAL_BANKROLL", "1000")),
        min_hold_seconds=float(os.environ.get("MIN_HOLD_SECONDS", "10.0")),
        # Hardening options
        price_staleness_ttl=float(os.environ.get("PRICE_STALENESS_TTL", "30.0")),
        require_valid_book=os.environ.get("REQUIRE_VALID_BOOK", "true").lower() in ("1", "true", "yes"),
        debounce_exit_checks=int(os.environ.get("DEBOUNCE_EXIT_CHECKS", "0")),
        exit_team_match_min_confidence=float(os.environ.get("EXIT_TEAM_MATCH_MIN_CONFIDENCE", "0.7")),
    )

    await tracker.start()

    try:
        while True:
            await asyncio.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        await tracker.stop()


if __name__ == "__main__":
    asyncio.run(main())
