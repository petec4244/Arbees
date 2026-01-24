"""
Position Manager service for executing trades and managing positions.

Responsibilities:
- Subscribe to trading signals from Redis
- Execute signals through paper trading engine
- Monitor for arbitrage opportunities
- Close positions when games end
- Track and report performance
- Enforce risk limits via RiskController
"""

import asyncio
import logging
import os
from datetime import datetime
from typing import Optional

import json
import time

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.models.game import Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection, ArbitrageOpportunity
from arbees_shared.models.trade import PaperTrade, TradeStatus, TradeSide
from arbees_shared.risk import RiskController, RiskDecision, RiskRejection
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from markets.paper.engine import PaperTradingEngine

logger = logging.getLogger(__name__)

# region agent log (helper)
def _agent_dbg(hypothesisId: str, location: str, message: str, data: dict) -> None:
    """Write a single NDJSON debug line to the host-mounted .cursor/debug.log (DEBUG MODE ONLY)."""
    try:
        payload = {
            "sessionId": "debug-session",
            "runId": os.environ.get("DEBUG_RUN_ID", "pre-fix"),
            "hypothesisId": hypothesisId,
            "location": location,
            "message": message,
            "data": data,
            "timestamp": int(time.time() * 1000),
        }
        with open("/app/.cursor/debug.log", "a", encoding="utf-8") as f:
            f.write(json.dumps(payload, default=str) + "\n")
    except Exception:
        pass
# endregion


def _side_display(side: str) -> str:
    """Convert buy/sell to HOME/AWAY for display."""
    return "HOME" if side == "buy" else "AWAY"


# Sport-specific stop-loss thresholds (% probability move against us)
# Higher-scoring sports (basketball) = tighter stop-loss (more frequent score changes)
# Lower-scoring sports (hockey, soccer) = wider stop-loss (score changes are bigger swings)
SPORT_STOP_LOSS_DEFAULTS: dict[str, float] = {
    "NBA": 3.0,      # Fast-paced, frequent scoring
    "NCAAB": 3.0,    # Similar to NBA
    "NFL": 5.0,      # Medium pace, touchdowns are 7pts
    "NCAAF": 5.0,    # Similar to NFL
    "NHL": 7.0,      # Low scoring, each goal is significant
    "MLB": 6.0,      # Low scoring, but innings can swing
    "MLS": 7.0,      # Low scoring like hockey
    "SOCCER": 7.0,   # Low scoring
    "TENNIS": 4.0,   # Point-by-point volatility
    "MMA": 8.0,      # Binary outcome, big swings possible
}


class PositionManager:
    """
    Position Manager handles trade execution and position lifecycle.

    Features:
    - Executes trading signals from game shards
    - Manages paper trading portfolio
    - Detects cross-market arbitrage opportunities
    - Closes positions when games settle
    """

    def __init__(
        self,
        initial_bankroll: float = 1000.0,
        min_edge_pct: float = 2.0,
        kelly_fraction: float = 0.25,
        max_position_pct: float = 10.0,
        max_buy_prob: float = 0.95,  # Don't buy above 95% - limited upside
        min_sell_prob: float = 0.05,  # Don't sell below 5% - limited upside
        # Position policy
        allow_hedging: bool = False,
        # Risk management settings
        max_daily_loss: float = 100.0,
        max_game_exposure: float = 50.0,
        max_sport_exposure: float = 200.0,
        max_latency_ms: float = 5000.0,
        # Exit monitoring settings
        take_profit_pct: float = 3.0,         # Exit when prob moves 3%+ in our favor
        default_stop_loss_pct: float = 5.0,   # Fallback if sport not configured
        exit_check_interval: float = 1.0,     # Check every 1 second
        sport_stop_loss: Optional[dict[str, float]] = None,  # Sport-specific overrides
        # Cooldown settings (prevent rapid re-entry)
        win_cooldown_seconds: float = 180.0,   # 3 minutes after a win
        loss_cooldown_seconds: float = 300.0,  # 5 minutes after a loss
    ):
        """
        Initialize Position Manager.

        Args:
            initial_bankroll: Starting capital for paper trading
            min_edge_pct: Minimum edge to take a trade
            kelly_fraction: Fraction of Kelly criterion for sizing
            max_position_pct: Maximum position size as % of bankroll
            max_buy_prob: Maximum probability to BUY at (avoid limited upside)
            min_sell_prob: Minimum probability to SELL at (avoid limited upside)
            max_daily_loss: Maximum loss per day before halting ($)
            max_game_exposure: Maximum exposure to single game ($)
            max_sport_exposure: Maximum exposure to single sport ($)
            max_latency_ms: Maximum signal latency before rejection (ms)
            win_cooldown_seconds: Cooldown after a winning trade (default 3 min)
            loss_cooldown_seconds: Cooldown after a losing trade (default 5 min)
        """
        self.initial_bankroll = initial_bankroll
        self.min_edge_pct = min_edge_pct
        self.kelly_fraction = kelly_fraction
        self.max_position_pct = max_position_pct
        self.max_buy_prob = max_buy_prob
        self.min_sell_prob = min_sell_prob
        self.allow_hedging = allow_hedging

        # Risk management settings
        self.max_daily_loss = max_daily_loss
        self.max_game_exposure = max_game_exposure
        self.max_sport_exposure = max_sport_exposure
        self.max_latency_ms = max_latency_ms

        # Exit monitoring settings
        self.take_profit_pct = take_profit_pct
        self.default_stop_loss_pct = default_stop_loss_pct
        self.exit_check_interval = exit_check_interval
        self.sport_stop_loss = sport_stop_loss or SPORT_STOP_LOSS_DEFAULTS.copy()

        # Cooldown settings (prevent rapid re-entry on same game)
        self.win_cooldown_seconds = win_cooldown_seconds
        self.loss_cooldown_seconds = loss_cooldown_seconds

        # Connections
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None

        # Paper trading engine
        self.paper_engine: Optional[PaperTradingEngine] = None

        # Risk controller
        self.risk_controller: Optional[RiskController] = None

        # Tracking
        self._running = False
        self._signal_count = 0
        self._trade_count = 0
        self._arb_count = 0
        self._risk_rejected_count = 0
        self._edge_rejected_count = 0
        self._prob_rejected_count = 0
        self._duplicate_rejected_count = 0
        self._no_market_rejected_count = 0
        self._cooldown_rejected_count = 0

        # Game cooldown tracking: game_id -> (last_trade_time, was_win)
        self._game_cooldowns: dict[str, tuple[datetime, bool]] = {}

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

    def _is_game_in_cooldown(self, game_id: str) -> tuple[bool, Optional[str]]:
        """Check if a game is in cooldown period after a recent trade.

        Returns:
            (is_in_cooldown, reason) - reason is human-readable if in cooldown
        """
        if game_id not in self._game_cooldowns:
            return False, None

        last_trade_time, was_win = self._game_cooldowns[game_id]
        now = datetime.utcnow()
        elapsed = (now - last_trade_time).total_seconds()

        if was_win:
            cooldown = self.win_cooldown_seconds
            if elapsed < cooldown:
                remaining = cooldown - elapsed
                return True, f"win cooldown ({remaining:.0f}s remaining of {cooldown:.0f}s)"
        else:
            cooldown = self.loss_cooldown_seconds
            if elapsed < cooldown:
                remaining = cooldown - elapsed
                return True, f"loss cooldown ({remaining:.0f}s remaining of {cooldown:.0f}s)"

        # Cooldown expired - remove from tracking
        del self._game_cooldowns[game_id]
        return False, None

    def _record_trade_close_for_cooldown(self, game_id: str, was_win: bool) -> None:
        """Record a trade closure to start the cooldown timer for that game."""
        self._game_cooldowns[game_id] = (datetime.utcnow(), was_win)
        cooldown = self.win_cooldown_seconds if was_win else self.loss_cooldown_seconds
        logger.debug(
            f"Started {cooldown:.0f}s cooldown on game {game_id} after {'win' if was_win else 'loss'}"
        )

    async def start(self) -> None:
        """Start the position manager and connect to services."""
        logger.info("Starting Position Manager")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Initialize risk controller
        self.risk_controller = RiskController(
            pool=pool,
            max_daily_loss=self.max_daily_loss,
            max_game_exposure=self.max_game_exposure,
            max_sport_exposure=self.max_sport_exposure,
            max_latency_ms=self.max_latency_ms,
        )
        logger.info(
            f"Risk Controller initialized: max_daily_loss=${self.max_daily_loss}, "
            f"max_game_exposure=${self.max_game_exposure}, max_sport_exposure=${self.max_sport_exposure}"
        )

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Connect to market clients
        self.kalshi = KalshiClient()
        await self.kalshi.connect()

        self.polymarket = PolymarketClient()
        await self.polymarket.connect()

        # Initialize paper trading engine
        self.paper_engine = PaperTradingEngine(
            initial_bankroll=self.initial_bankroll,
            min_edge_pct=self.min_edge_pct,
            kelly_fraction=self.kelly_fraction,
            max_position_pct=self.max_position_pct,
            db_client=self.db,
            redis_bus=self.redis,
        )

        # Load existing bankroll from database
        await self._load_bankroll()

        self._running = True

        # Subscribe to signals
        await self.redis.subscribe("signals:new", self._handle_signal)

        # Subscribe to game endings
        await self.redis.subscribe("games:ended", self._handle_game_ended)

        # Start listening
        asyncio.create_task(self.redis.start_listening())

        # Start heartbeat
        asyncio.create_task(self._heartbeat_loop())

        # Start arbitrage scanner
        asyncio.create_task(self._arbitrage_scan_loop())

        # Start risk monitoring
        asyncio.create_task(self._risk_monitor_loop())

        # Start position exit monitoring (polling fallback)
        asyncio.create_task(self._position_monitor_loop())

        # Subscribe to game state updates for real-time exit monitoring
        await self.redis.psubscribe("game:*:state", self._handle_game_state_update)

        logger.info(
            f"Position Manager started with active exit monitoring "
            f"(take_profit={self.take_profit_pct}%, default_stop_loss={self.default_stop_loss_pct}%, "
            f"check_interval={self.exit_check_interval}s)"
        )

    async def stop(self) -> None:
        """Stop the position manager gracefully."""
        logger.info("Stopping Position Manager")
        self._running = False

        # Persist bankroll state
        await self._save_bankroll()

        # Disconnect from services
        if self.kalshi:
            await self.kalshi.disconnect()
        if self.polymarket:
            await self.polymarket.disconnect()
        if self.redis:
            await self.redis.disconnect()

        logger.info("Position Manager stopped")

    async def _load_bankroll(self) -> None:
        """Load bankroll state from database."""
        if not self.db or not self.paper_engine:
            return

        pool = await get_pool()
        row = await pool.fetchrow("""
            SELECT * FROM bankroll WHERE account_name = 'default'
        """)

        if row:
            from arbees_shared.models.trade import Bankroll
            # Handle missing piggybank_balance column for backwards compatibility
            piggybank = float(row.get("piggybank_balance") or 0.0)
            self.paper_engine._bankroll = Bankroll(
                initial_balance=float(row["initial_balance"]),
                current_balance=float(row["current_balance"]),
                piggybank_balance=piggybank,
                peak_balance=float(row["peak_balance"]),
                trough_balance=float(row["trough_balance"]),
            )
            logger.info(
                f"Loaded bankroll: trading=${self.paper_engine._bankroll.current_balance:.2f}, "
                f"piggybank=${piggybank:.2f}, "
                f"total=${self.paper_engine._bankroll.total_balance:.2f}"
            )
        else:
            # Create initial bankroll record
            await pool.execute("""
                INSERT INTO bankroll (account_name, initial_balance, current_balance, piggybank_balance, peak_balance, trough_balance)
                VALUES ('default', $1, $1, 0.0, $1, $1)
                ON CONFLICT (account_name) DO NOTHING
            """, self.initial_bankroll)
            logger.info(f"Created initial bankroll: ${self.initial_bankroll:.2f}")

    async def _save_bankroll(self) -> None:
        """Save bankroll state to database."""
        if not self.db or not self.paper_engine:
            return

        pool = await get_pool()
        bankroll = self.paper_engine._bankroll
        await pool.execute("""
            UPDATE bankroll
            SET current_balance = $1, piggybank_balance = $2, peak_balance = $3, trough_balance = $4, updated_at = NOW()
            WHERE account_name = 'default'
        """, bankroll.current_balance, bankroll.piggybank_balance, bankroll.peak_balance, bankroll.trough_balance)

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                risk_metrics = None
                if self.risk_controller:
                    risk_metrics = await self.risk_controller.get_metrics()

                bankroll = self.paper_engine._bankroll if self.paper_engine else None
                status = {
                    "type": "position_manager",
                    "signals_received": self._signal_count,
                    "trades_executed": self._trade_count,
                    "risk_rejected": self._risk_rejected_count,
                    "arb_opportunities": self._arb_count,
                    "bankroll_trading": bankroll.current_balance if bankroll else 0,
                    "bankroll_piggybank": bankroll.piggybank_balance if bankroll else 0,
                    "bankroll_total": bankroll.total_balance if bankroll else 0,
                    "open_positions": len(self.paper_engine.get_open_trades()) if self.paper_engine else 0,
                    "daily_pnl": risk_metrics.daily_pnl if risk_metrics else 0,
                    "circuit_breaker": risk_metrics.circuit_breaker_open if risk_metrics else False,
                    "games_in_cooldown": len(self._game_cooldowns),
                    "timestamp": datetime.utcnow().isoformat(),
                }
                logger.info(
                    f"Position Manager status: {status['trades_executed']} trades, "
                    f"Rejected(Risk: {self._risk_rejected_count}, Edge: {self._edge_rejected_count}, "
                    f"Prob: {self._prob_rejected_count}, Dup: {self._duplicate_rejected_count}, "
                    f"NoMkt: {self._no_market_rejected_count}, Cooldown: {self._cooldown_rejected_count}), "
                    f"trading=${status['bankroll_trading']:.2f}, piggybank=${status['bankroll_piggybank']:.2f}, "
                    f"total=${status['bankroll_total']:.2f}, "
                    f"daily P&L ${status['daily_pnl']:.2f}"
                )

                # Also save bankroll periodically
                await self._save_bankroll()
            except Exception as e:
                logger.warning(f"Heartbeat failed: {e}")

            await asyncio.sleep(30)

    async def _risk_monitor_loop(self) -> None:
        """Periodically log risk status."""
        while self._running:
            try:
                if self.risk_controller:
                    report = await self.risk_controller.get_status_report()
                    logger.info(f"Risk Status:\n{report}")
            except Exception as e:
                logger.warning(f"Risk monitor failed: {e}")

            await asyncio.sleep(60)  # Log risk status every minute

    async def _handle_signal(self, data: dict) -> None:
        """Handle incoming trading signal."""
        self._signal_count += 1

        try:
            # Parse signal
            signal = TradingSignal(**data)
            logger.info(f"Received signal: {signal.signal_type.value} {signal.direction.value} {signal.team} (edge: {signal.edge_pct:.1f}%)")

            # CRITICAL: Only trade with real market prices - no synthetic prices
            # Without real market data, we can't verify edge vs actual market
            if signal.market_prob is None:
                self._no_market_rejected_count += 1
                logger.info(f"Signal rejected: no real market price available (synthetic prices disabled)")
                return

            # Skip if edge below threshold
            if signal.edge_pct < self.min_edge_pct:
                self._edge_rejected_count += 1
                logger.debug(f"Signal rejected: edge {signal.edge_pct}% < min {self.min_edge_pct}%")
                return

            # Probability guardrails - avoid extreme probabilities with poor risk/reward
            if signal.direction == SignalDirection.BUY and signal.model_prob > self.max_buy_prob:
                self._prob_rejected_count += 1
                logger.info(f"Signal rejected: BUY at {signal.model_prob*100:.1f}% > max {self.max_buy_prob*100:.0f}% (poor risk/reward)")
                return
            if signal.direction == SignalDirection.SELL and signal.model_prob < self.min_sell_prob:
                self._prob_rejected_count += 1
                logger.info(f"Signal rejected: SELL at {signal.model_prob*100:.1f}% < min {self.min_sell_prob*100:.0f}% (poor risk/reward)")
                return

            # Check for existing position on this game
            existing_position = await self._get_open_position_for_game(signal.game_id)

            if existing_position and not self.allow_hedging:
                existing_side = existing_position["side"]
                new_side = "buy" if signal.direction == SignalDirection.BUY else "sell"

                if existing_side == new_side:
                    # Same direction - skip, don't double down
                    self._duplicate_rejected_count += 1
                    logger.info(f"Skipping signal: already have {_side_display(existing_side)} position on game {signal.game_id}")
                    return
                else:
                    # Opposite direction - close existing position instead of opening new one
                    logger.info(f"Closing existing {_side_display(existing_side)} position on game {signal.game_id} (opposite signal received)")
                    await self._close_position(existing_position, signal)
                    return

            # Check cooldown - prevents rapid re-entry after closing a position
            in_cooldown, cooldown_reason = self._is_game_in_cooldown(signal.game_id)
            if in_cooldown:
                self._cooldown_rejected_count += 1
                logger.info(f"Signal rejected: game {signal.game_id} in {cooldown_reason}")
                return

            # No existing position - open new one
            # Get market price for execution - REQUIRE real prices
            market_price = await self._get_market_price(signal)
            if not market_price:
                # Use signal's market data if available (from signal generation time)
                # This is real market data captured when the signal was created
                if signal.market_prob is not None:
                    market_price = self._create_price_from_signal(signal)
                    logger.info(f"Using signal's market data: mid={signal.market_prob:.3f}")
                else:
                    self._no_market_rejected_count += 1
                    logger.warning(f"No market price available for signal {signal.signal_id} - skipping (synthetic disabled)")
                    return

            # If hedging is enabled, allow multiple positions per game as long as we aren't opening
            # the exact same position (platform + market_id + side).
            if self.allow_hedging:
                new_side = "buy" if signal.direction == SignalDirection.BUY else "sell"
                existing_same = await self._get_open_position_for_market(
                    platform=market_price.platform.value,
                    market_id=str(market_price.market_id),
                    side=new_side,
                )
                if existing_same:
                    self._duplicate_rejected_count += 1
                    logger.info(
                        "Skipping signal: duplicate position already open (%s on %s:%s) game=%s trade_id=%s",
                        new_side,
                        market_price.platform.value,
                        market_price.market_id,
                        signal.game_id,
                        existing_same.get("trade_id"),
                    )
                    return

            # Calculate proposed position size for risk check
            # Use paper engine's sizing logic to estimate
            proposed_size = self._estimate_position_size(signal, market_price)

            # Risk management check
            if self.risk_controller:
                risk_decision = await self.risk_controller.evaluate_trade(
                    game_id=signal.game_id,
                    sport=signal.sport.value,
                    team=signal.team,
                    side="buy" if signal.direction == SignalDirection.BUY else "sell",
                    proposed_size=proposed_size,
                    signal_timestamp=signal.created_at,
                )

                if not risk_decision.approved:
                    self._risk_rejected_count += 1
                    logger.warning(
                        f"Trade REJECTED by risk controller: {risk_decision.rejection_reason.value} - "
                        f"{risk_decision.rejection_details}"
                    )
                    return

                logger.debug(
                    f"Risk check passed: daily_pnl=${risk_decision.daily_pnl:.2f}, "
                    f"game_exp=${risk_decision.game_exposure:.2f}, "
                    f"sport_exp=${risk_decision.sport_exposure:.2f}"
                )

            # Execute through paper engine
            if self.paper_engine:
                # Fee-aware minimum edge check using configured take-profit/stop-loss (Kalshi only)
                if market_price.platform == Platform.KALSHI:
                    ok = self._fee_aware_take_profit_check(signal, market_price)
                    if not ok:
                        logger.info(
                            f"Trade rejected: fees absorb take-profit edge "
                            f"for {signal.game_id} {signal.team}"
                        )
                        return

                # region agent log
                _agent_dbg(
                    "H1_H3",
                    "services/position_manager/position_manager.py:_handle_signal",
                    "about_to_execute_trade",
                    {
                        "signal_id": signal.signal_id,
                        "game_id": signal.game_id,
                        "direction": signal.direction.value,
                        "signal_team": signal.team,
                        "signal_model_prob": float(signal.model_prob),
                        "signal_market_prob": float(signal.market_prob) if signal.market_prob is not None else None,
                        "signal_edge_pct": float(signal.edge_pct),
                        "price_market_id": market_price.market_id,
                        "price_platform": market_price.platform.value,
                        "price_contract_team": market_price.contract_team,
                        "price_market_title": market_price.market_title,
                        "price_yes_bid": float(market_price.yes_bid),
                        "price_yes_ask": float(market_price.yes_ask),
                        "price_mid": float(market_price.mid_price),
                    },
                )
                # endregion
                logger.info(
                    f"Opening new position: {signal.direction.value} {signal.team} "
                    f"using contract_team='{market_price.contract_team}' "
                    f"(bid={market_price.yes_bid:.3f}, ask={market_price.yes_ask:.3f}, "
                    f"mid={market_price.mid_price:.3f})"
                )
                trade = await self.paper_engine.execute_signal(signal, market_price)
                if trade:
                    self._trade_count += 1
                    logger.info(
                        f"Opened trade {trade.trade_id}: {signal.direction.value} {signal.team} "
                        f"@ {trade.entry_price:.3f} x ${trade.size:.2f}"
                    )
                else:
                    logger.info(f"Trade not executed for signal {signal.signal_id} - rejected by paper engine")

        except Exception as e:
            logger.error(f"Error handling signal: {e}")

    def _estimate_position_size(self, signal: TradingSignal, market_price: MarketPrice) -> float:
        """Estimate position size for risk check (mirrors paper engine logic)."""
        if not self.paper_engine:
            return 0.0

        # Get current bankroll
        bankroll = self.paper_engine._bankroll.available_balance

        # Kelly fraction sizing
        kelly = signal.kelly_fraction if signal.kelly_fraction > 0 else 0.0
        fractional_kelly = kelly * self.kelly_fraction

        # Position as % of bankroll
        position_pct = min(fractional_kelly * 100, self.max_position_pct)
        position_size = bankroll * (position_pct / 100)

        # Minimum position size
        min_size = 1.0
        return max(min_size, position_size)

    def _fee_aware_take_profit_check(self, signal: TradingSignal, market_price: MarketPrice) -> bool:
        """Reject Kalshi trades where fees would erase take-profit edge."""
        if not self.paper_engine:
            return True

        # Determine side and entry (includes slippage)
        side = TradeSide.BUY if signal.direction == SignalDirection.BUY else TradeSide.SELL
        entry_price = market_price.yes_ask if side == TradeSide.BUY else market_price.yes_bid
        exec_price = self.paper_engine.apply_slippage(entry_price, side)

        size = self._estimate_position_size(signal, market_price)
        if size < 1.0:
            return True

        # Target exits based on configured take-profit / stop-loss
        take_profit = self.take_profit_pct / 100.0
        stop_loss = self._get_stop_loss_for_sport(signal.sport.value) / 100.0

        if side == TradeSide.BUY:
            tp_exit = min(1.0, exec_price + take_profit)
            sl_exit = max(0.0, exec_price - stop_loss)
        else:
            tp_exit = max(0.0, exec_price - take_profit)
            sl_exit = min(1.0, exec_price + stop_loss)

        entry_fees = self.paper_engine._estimate_kalshi_fees(exec_price, size)
        exit_fees_tp = self.paper_engine._estimate_kalshi_fees(tp_exit, size)
        exit_fees_sl = self.paper_engine._estimate_kalshi_fees(sl_exit, size)

        gross_tp = size * (tp_exit - exec_price) if side == TradeSide.BUY else size * (exec_price - tp_exit)
        gross_sl = size * (sl_exit - exec_price) if side == TradeSide.BUY else size * (exec_price - sl_exit)

        net_tp = gross_tp - entry_fees - exit_fees_tp
        net_sl = gross_sl - entry_fees - exit_fees_sl

        if net_tp <= 0:
            logger.info(
                f"Fee-aware TP reject: net_tp=${net_tp:.2f} "
                f"(entry={exec_price:.3f}, tp={tp_exit:.3f}, fees=${entry_fees + exit_fees_tp:.2f})"
            )
            return False

        # Log worst-case for visibility (does not reject)
        logger.debug(
            f"Fee-aware SL estimate: net_sl=${net_sl:.2f} "
            f"(entry={exec_price:.3f}, sl={sl_exit:.3f}, fees=${entry_fees + exit_fees_sl:.2f})"
        )
        return True

    async def _get_open_position_for_game(self, game_id: str) -> Optional[dict]:
        """Get existing open position for a game."""
        pool = await get_pool()
        row = await pool.fetchrow("""
            SELECT trade_id, game_id, side, entry_price, size, time
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
        """, game_id)
        return dict(row) if row else None

    async def _get_open_position_for_market(
        self,
        platform: str,
        market_id: str,
        side: str,
    ) -> Optional[dict]:
        """Get existing open position for an exact position identity (platform/market/side)."""
        pool = await get_pool()
        row = await pool.fetchrow(
            """
            SELECT trade_id, game_id, side, entry_price, size, time, platform, market_id
            FROM paper_trades
            WHERE platform = $1
              AND market_id = $2
              AND side = $3
              AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
            """,
            platform,
            market_id,
            side,
        )
        return dict(row) if row else None

    async def _close_position(self, position: dict, signal: TradingSignal) -> None:
        """Close an existing position based on opposite signal."""
        if not self.paper_engine:
            return

        # Find the trade in paper engine's list
        open_trades = self.paper_engine.get_open_trades()
        trade_to_close = None
        for trade in open_trades:
            if trade.trade_id == position["trade_id"]:
                trade_to_close = trade
                break

        if not trade_to_close:
            # Trade not in memory, close directly in database
            logger.warning(f"Trade {position['trade_id']} not in paper engine memory, closing in DB")
            pool = await get_pool()

            # Calculate exit price based on current model probability
            exit_price = signal.model_prob

            # Determine PnL
            entry_price = float(position["entry_price"])
            size = float(position["size"])
            if position["side"] == "buy":
                pnl = size * (exit_price - entry_price)
            else:
                pnl = size * (entry_price - exit_price)

            outcome = "win" if pnl > 0 else ("loss" if pnl < 0 else "push")

            await pool.execute("""
                UPDATE paper_trades
                SET status = 'closed', outcome = $1, exit_price = $2, exit_time = NOW(),
                    pnl = $3, pnl_pct = $4
                WHERE trade_id = $5
            """, outcome, exit_price, pnl, (pnl / (size * entry_price)) * 100 if entry_price > 0 else 0, position["trade_id"])

            logger.info(f"Closed position {position['trade_id']}: {_side_display(position['side'])} -> PnL ${pnl:.2f} ({outcome})")

            # Record cooldown for this game
            self._record_trade_close_for_cooldown(position["game_id"], outcome == "win")
            return

        # Close using paper engine
        exit_price = signal.model_prob
        closed_trade = await self.paper_engine.close_trade(trade_to_close, exit_price)
        logger.info(f"Closed position {closed_trade.trade_id}: PnL ${closed_trade.pnl:.2f} ({closed_trade.outcome.value})")

        # Record cooldown for this game
        from arbees_shared.models.trade import TradeOutcome
        self._record_trade_close_for_cooldown(
            closed_trade.game_id,
            closed_trade.outcome == TradeOutcome.WIN
        )

    async def _get_market_price(self, signal: TradingSignal) -> Optional[MarketPrice]:
        """Get current market price for a signal.

        IMPORTANT: For Polymarket moneyline markets, we need to get the price
        for the CORRECT team's contract (signal.team). If we get the wrong team's
        price, we must invert it.
        """
        target_team = signal.team
        price = None

        # Try Kalshi first
        if signal.platform_buy == Platform.KALSHI or signal.platform_sell == Platform.KALSHI:
            market_id = signal.buy_price if signal.platform_buy == Platform.KALSHI else signal.sell_price
            if market_id and self.kalshi:
                try:
                    # Try to find market ID from database
                    pool = await get_pool()
                    row = await pool.fetchrow("""
                        SELECT market_id FROM market_prices
                        WHERE game_id = $1 AND platform = 'kalshi'
                        ORDER BY time DESC LIMIT 1
                    """, signal.game_id)
                    if row:
                        price = await self.kalshi.get_market_price(row["market_id"])
                        if price:
                            # Kalshi typically has single home-team contract, validate
                            return self._validate_and_maybe_invert_price(price, target_team)
                except Exception as e:
                    logger.warning(f"Error getting Kalshi price: {e}")

        # Try Polymarket - look for the correct team's contract
        if self.polymarket:
            try:
                pool = await get_pool()

                # First, try to find a price for the specific team using contract_team column
                if target_team:
                    # Look for market with matching contract_team
                    row = await pool.fetchrow("""
                        SELECT market_id, market_title, contract_team, yes_bid, yes_ask, yes_bid_size, yes_ask_size, volume, liquidity, time
                        FROM market_prices
                        WHERE game_id = $1 AND platform = 'polymarket'
                          AND contract_team IS NOT NULL
                          AND (contract_team ILIKE $2 OR contract_team ILIKE $3)
                        ORDER BY time DESC LIMIT 1
                    """, signal.game_id, f"%{target_team}%", f"{target_team}%")

                    if row:
                        logger.info(f"Found Polymarket price for team '{target_team}' via contract_team: {row['contract_team']}")
                        # Use DB row data directly to preserve contract_team
                        price = MarketPrice(
                            market_id=row["market_id"],
                            platform=Platform.POLYMARKET,
                            market_title=row["market_title"],
                            contract_team=row["contract_team"],
                            yes_bid=float(row["yes_bid"]),
                            yes_ask=float(row["yes_ask"]),
                            yes_bid_size=float(row.get("yes_bid_size") or 0),
                            yes_ask_size=float(row.get("yes_ask_size") or 0),
                            volume=float(row["volume"] or 0),
                            liquidity=float(row.get("liquidity") or 0),
                        )
                        return self._validate_and_maybe_invert_price(price, target_team)

                    # Fallback: look for market_title containing the team name
                    row = await pool.fetchrow("""
                        SELECT market_id, market_title, contract_team, yes_bid, yes_ask, yes_bid_size, yes_ask_size, volume, liquidity, time
                        FROM market_prices
                        WHERE game_id = $1 AND platform = 'polymarket'
                          AND (market_title ILIKE $2 OR market_title ILIKE $3)
                        ORDER BY time DESC LIMIT 1
                    """, signal.game_id, f"%{target_team}%", f"%[{target_team}]%")

                    if row:
                        logger.info(f"Found Polymarket price for team '{target_team}' via title: {row['market_title']}")
                        # Use DB row data directly to preserve contract_team
                        price = MarketPrice(
                            market_id=row["market_id"],
                            platform=Platform.POLYMARKET,
                            market_title=row["market_title"],
                            contract_team=row.get("contract_team"),
                            yes_bid=float(row["yes_bid"]),
                            yes_ask=float(row["yes_ask"]),
                            yes_bid_size=float(row.get("yes_bid_size") or 0),
                            yes_ask_size=float(row.get("yes_ask_size") or 0),
                            volume=float(row["volume"] or 0),
                            liquidity=float(row.get("liquidity") or 0),
                        )
                        return self._validate_and_maybe_invert_price(price, target_team)

                # Fallback: get any recent price for this game
                row = await pool.fetchrow("""
                    SELECT market_id, market_title, contract_team, yes_bid, yes_ask, yes_bid_size, yes_ask_size, volume, liquidity
                    FROM market_prices
                    WHERE game_id = $1 AND platform = 'polymarket'
                    ORDER BY time DESC LIMIT 1
                """, signal.game_id)
                if row:
                    logger.warning(f"Using generic Polymarket price (team '{target_team}' not found): {row['market_title']} (contract_team={row.get('contract_team')})")
                    # Use DB row data directly
                    price = MarketPrice(
                        market_id=row["market_id"],
                        platform=Platform.POLYMARKET,
                        market_title=row["market_title"],
                        contract_team=row.get("contract_team"),
                        yes_bid=float(row["yes_bid"]),
                        yes_ask=float(row["yes_ask"]),
                        yes_bid_size=float(row.get("yes_bid_size") or 0),
                        yes_ask_size=float(row.get("yes_ask_size") or 0),
                        volume=float(row["volume"] or 0),
                        liquidity=float(row.get("liquidity") or 0),
                    )
                    return self._validate_and_maybe_invert_price(price, target_team)
            except Exception as e:
                logger.warning(f"Error getting Polymarket price: {e}")

        return None

    def _validate_and_maybe_invert_price(
        self,
        price: MarketPrice,
        target_team: Optional[str],
        home_team: Optional[str] = None
    ) -> MarketPrice:
        """Validate that price is for the correct team, invert if not.

        This is a critical safety check to prevent comparing probabilities
        from different teams (the home/away mismatch bug).

        If contract_team is None and home_team is provided, assumes the price
        is for the home team (common pattern for REST-fetched legacy prices).
        """
        if not price or not target_team:
            return price

        contract_team = price.contract_team

        # If contract_team is None but home_team is provided, assume home team
        if not contract_team and home_team:
            contract_team = home_team
            logger.debug(f"Legacy price has no contract_team, assuming HOME team: {home_team}")

        if not contract_team:
            # Still can't determine team - return as-is with warning
            logger.warning(
                f"Cannot validate price team matching: contract_team is None "
                f"(target_team='{target_team}')"
            )
            return price

        if self._teams_match(contract_team, target_team):
            # Teams match - no inversion needed
            logger.debug(
                f"Price team validated: contract='{contract_team}' matches target='{target_team}'"
            )
            return price

        # Teams DON'T match - INVERT the price
        logger.info(
            f"Inverting price: contract_team='{contract_team}' doesn't match "
            f"target_team='{target_team}' - inverting bid/ask"
        )

        inverted_price = MarketPrice(
            market_id=price.market_id,
            platform=price.platform,
            market_title=f"{target_team} (inverted from {contract_team}) [{target_team}]",
            contract_team=target_team,
            yes_bid=1.0 - price.yes_ask,  # Invert bid/ask
            yes_ask=1.0 - price.yes_bid,
            volume=price.volume,
            liquidity=price.liquidity,
            timestamp=price.timestamp if hasattr(price, 'timestamp') else None,
        )

        # region agent log
        _agent_dbg(
            "H3",
            "services/position_manager/position_manager.py:_validate_and_maybe_invert_price",
            "inverted_price",
            {
                "orig_contract_team": contract_team,
                "target_team": target_team,
                "orig_yes_bid": float(price.yes_bid),
                "orig_yes_ask": float(price.yes_ask),
                "inv_yes_bid": float(inverted_price.yes_bid),
                "inv_yes_ask": float(inverted_price.yes_ask),
                "orig_mid": float(price.mid_price),
                "inv_mid": float(inverted_price.mid_price),
                "market_id": price.market_id,
                "platform": price.platform.value,
            },
        )
        # endregion

        return inverted_price

    def _create_price_from_signal(self, signal: TradingSignal) -> MarketPrice:
        """Create market price from signal's captured market data.

        Uses the actual market_prob from the signal (captured at signal generation time)
        rather than the model's probability estimate. This ensures we execute at
        realistic market prices.
        
        IMPORTANT: Sets contract_team to signal.team so downstream knows which
        team's YES contract this price represents.
        """
        # Use the actual market probability from signal, NOT model_prob
        # market_prob is the real market mid-price when signal was generated
        market_prob = signal.market_prob

        # Estimate a realistic spread (typically 2-4% for prediction markets)
        spread = 0.02  # 2% spread

        return MarketPrice(
            market_id=f"signal_{signal.game_id}",
            platform=Platform.PAPER,
            market_title=f"{signal.team} to win",
            contract_team=signal.team,  # Which team's YES contract
            yes_bid=max(0.01, market_prob - spread),
            yes_ask=min(0.99, market_prob + spread),
            volume=0,
            liquidity=10000,  # Assume good liquidity for paper trading
        )

    async def _handle_game_ended(self, data: dict) -> None:
        """Handle game ending - close any open positions based on final result."""
        game_id = data.get("game_id")
        if not game_id or not self.paper_engine:
            return

        try:
            # Get final score info
            home_score = data.get("home_score", 0)
            away_score = data.get("away_score", 0)
            home_team = data.get("home_team", "")
            away_team = data.get("away_team", "")
            home_won = home_score > away_score

            logger.info(
                f"Game {game_id} ended: {home_team} {home_score} - {away_score} {away_team} "
                f"({'HOME' if home_won else 'AWAY'} won)"
            )

            # Find open trades for this game
            open_trades = self.paper_engine.get_open_trades()
            game_trades = [t for t in open_trades if t.game_id == game_id]

            for trade in game_trades:
                # Determine which team this trade was on from market_title
                # Market titles are like "Sacred Heart Pioneers to win" or "Team A vs Team B"
                title = trade.market_title.lower()

                # Check if trade was on home or away team
                trade_on_home = self._teams_match(home_team, title) if home_team else False
                trade_on_away = self._teams_match(away_team, title) if away_team else False

                # Determine if the team we bet on won
                if trade_on_home:
                    team_won = home_won
                    bet_team = home_team
                elif trade_on_away:
                    team_won = not home_won
                    bet_team = away_team
                else:
                    # Couldn't determine team - use home as fallback
                    logger.warning(f"Could not determine team for trade {trade.trade_id}, assuming home")
                    team_won = home_won
                    bet_team = home_team

                # Exit price depends on whether the team we bet on won
                # BUY YES: pays 1.0 if team wins, 0.0 if loses
                # SELL YES: receives entry, pays 1.0 if team wins, keeps entry if loses
                exit_price = 1.0 if team_won else 0.0

                logger.info(
                    f"Settling trade {trade.trade_id}: {trade.side.value} on '{bet_team}' "
                    f"(team_won={team_won}, exit_price={exit_price:.2f})"
                )
                # is_game_settlement=True: skip slippage since game ended at known price
                closed_trade = await self.paper_engine.close_trade(trade, exit_price, is_game_settlement=True)

                # Record cooldown (game ended, but be consistent)
                from arbees_shared.models.trade import TradeOutcome
                was_win = closed_trade.outcome == TradeOutcome.WIN
                self._record_trade_close_for_cooldown(game_id, was_win)

        except Exception as e:
            logger.error(f"Error closing trades for game {game_id}: {e}")

    async def _arbitrage_scan_loop(self) -> None:
        """Periodically scan for arbitrage opportunities across markets."""
        while self._running:
            try:
                await self._scan_for_arbitrage()
            except Exception as e:
                logger.error(f"Error in arbitrage scan: {e}")

            await asyncio.sleep(2)  # Scan every 2 seconds for faster arb detection

    async def _scan_for_arbitrage(self) -> None:
        """Scan market prices for arbitrage opportunities."""
        if not self.db:
            return

        pool = await get_pool()

        # Get recent prices from both platforms
        rows = await pool.fetch("""
            WITH recent_prices AS (
                SELECT DISTINCT ON (game_id, platform, market_type)
                    game_id, platform, market_id, market_type, yes_bid, yes_ask, volume, liquidity, time
                FROM market_prices
                WHERE time > NOW() - INTERVAL '5 minutes'
                ORDER BY game_id, platform, market_type, time DESC
            )
            SELECT
                k.game_id,
                k.market_type,
                k.market_id as kalshi_market,
                k.yes_bid as kalshi_bid,
                k.yes_ask as kalshi_ask,
                k.liquidity as kalshi_liquidity,
                p.market_id as poly_market,
                p.yes_bid as poly_bid,
                p.yes_ask as poly_ask,
                p.liquidity as poly_liquidity
            FROM recent_prices k
            JOIN recent_prices p ON k.game_id = p.game_id AND k.market_type = p.market_type
            WHERE k.platform = 'kalshi' AND p.platform = 'polymarket'
        """)

        for row in rows:
            # Check for cross-market arbitrage
            # Buy on Kalshi, sell on Polymarket
            kalshi_ask = float(row["kalshi_ask"])
            poly_bid = float(row["poly_bid"])

            if poly_bid > kalshi_ask:
                edge = (poly_bid - kalshi_ask) * 100
                if edge >= self.min_edge_pct:
                    await self._record_arbitrage(
                        game_id=row["game_id"],
                        buy_platform=Platform.KALSHI,
                        sell_platform=Platform.POLYMARKET,
                        buy_price=kalshi_ask,
                        sell_price=poly_bid,
                        edge_pct=edge,
                        kalshi_market=row["kalshi_market"],
                        poly_market=row["poly_market"],
                        kalshi_liquidity=float(row["kalshi_liquidity"]),
                        poly_liquidity=float(row["poly_liquidity"]),
                    )

            # Buy on Polymarket, sell on Kalshi
            poly_ask = float(row["poly_ask"])
            kalshi_bid = float(row["kalshi_bid"])

            if kalshi_bid > poly_ask:
                edge = (kalshi_bid - poly_ask) * 100
                if edge >= self.min_edge_pct:
                    await self._record_arbitrage(
                        game_id=row["game_id"],
                        buy_platform=Platform.POLYMARKET,
                        sell_platform=Platform.KALSHI,
                        buy_price=poly_ask,
                        sell_price=kalshi_bid,
                        edge_pct=edge,
                        kalshi_market=row["kalshi_market"],
                        poly_market=row["poly_market"],
                        kalshi_liquidity=float(row["kalshi_liquidity"]),
                        poly_liquidity=float(row["poly_liquidity"]),
                    )

    async def _record_arbitrage(
        self,
        game_id: str,
        buy_platform: Platform,
        sell_platform: Platform,
        buy_price: float,
        sell_price: float,
        edge_pct: float,
        kalshi_market: str,
        poly_market: str,
        kalshi_liquidity: float,
        poly_liquidity: float,
    ) -> None:
        """Record an arbitrage opportunity."""
        self._arb_count += 1

        pool = await get_pool()

        # Get game info for sport
        game_row = await pool.fetchrow("""
            SELECT sport FROM game_states WHERE game_id = $1 LIMIT 1
        """, game_id)
        sport = game_row["sport"] if game_row else "unknown"

        # Calculate profit potential
        liquidity_buy = kalshi_liquidity if buy_platform == Platform.KALSHI else poly_liquidity
        liquidity_sell = poly_liquidity if buy_platform == Platform.KALSHI else kalshi_liquidity
        max_size = min(liquidity_buy, liquidity_sell)
        implied_profit = max_size * (sell_price - buy_price)

        # Insert opportunity
        await pool.execute("""
            INSERT INTO arbitrage_opportunities (
                opportunity_id, opportunity_type, event_id, sport, market_title,
                platform_buy, platform_sell, buy_price, sell_price, edge_pct,
                implied_profit, liquidity_buy, liquidity_sell, is_risk_free, status, time
            ) VALUES (
                gen_random_uuid()::text, 'cross_market', $1, $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, true, 'active', NOW()
            )
        """,
            game_id,
            sport,
            f"Game {game_id} YES",
            buy_platform.value,
            sell_platform.value,
            buy_price,
            sell_price,
            edge_pct,
            implied_profit,
            liquidity_buy,
            liquidity_sell,
        )

        logger.info(
            f"Arbitrage found: Buy {buy_platform.value} @ {buy_price:.3f}, "
            f"Sell {sell_platform.value} @ {sell_price:.3f} (edge: {edge_pct:.1f}%)"
        )

    # ==========================================================================
    # Active Exit Monitoring
    # ==========================================================================

    async def _position_monitor_loop(self) -> None:
        """Actively monitor open positions for exit conditions (polling fallback)."""
        while self._running:
            try:
                await self._check_exit_conditions()
            except Exception as e:
                logger.error(f"Position monitor error: {e}")
            await asyncio.sleep(self.exit_check_interval)

    async def _check_exit_conditions(self) -> None:
        """Check all open positions against current market prices."""
        if not self.paper_engine:
            return

        open_trades = self.paper_engine.get_open_trades()
        if not open_trades:
            return

        for trade in open_trades:
            current_price = await self._get_current_price(trade)
            if current_price is None:
                continue

            sport = (trade.sport.value if trade.sport else "")
            should_exit, reason = self._evaluate_exit(trade, current_price, sport)
            if should_exit:
                await self._execute_exit(trade, current_price, reason)

    def _get_stop_loss_for_sport(self, sport: str) -> float:
        """Get stop-loss threshold for a sport.

        Different sports have different scoring patterns:
        - Basketball: Frequent scoring -> tighter stop-loss (3%)
        - Hockey/Soccer: Rare but significant goals -> wider stop-loss (7%)
        """
        sport_upper = sport.upper() if sport else ""
        return self.sport_stop_loss.get(sport_upper, self.default_stop_loss_pct)

    def _evaluate_exit(
        self,
        trade: PaperTrade,
        current_price: float,
        sport: str
    ) -> tuple[bool, str]:
        """Evaluate if position should be exited.

        Args:
            trade: The open trade to evaluate
            current_price: Current market mid-price (0-1)
            sport: Sport type for stop-loss lookup

        Returns:
            (should_exit: bool, reason: str)

        Note: We compare against entry_price (actual execution price), not model_prob
        (model's estimate at entry). This ensures take-profit/stop-loss decisions
        are based on actual P&L, not theoretical edge.
        """
        # Use entry_price, not model_prob - we want actual P&L movement
        entry_price = trade.entry_price
        stop_loss_pct = self._get_stop_loss_for_sport(sport)

        if trade.side == TradeSide.BUY:
            # BUY position profits when price goes UP
            price_move = current_price - entry_price

            if price_move >= self.take_profit_pct / 100:
                return True, f"take_profit: +{price_move*100:.1f}%"

            if price_move <= -stop_loss_pct / 100:
                return True, f"stop_loss: {price_move*100:.1f}% (limit={stop_loss_pct}%)"

        else:  # SELL position
            # SELL position profits when price goes DOWN
            price_move = entry_price - current_price

            if price_move >= self.take_profit_pct / 100:
                return True, f"take_profit: +{price_move*100:.1f}%"

            if price_move <= -stop_loss_pct / 100:
                return True, f"stop_loss: {price_move*100:.1f}% (limit={stop_loss_pct}%)"

        return False, ""

    async def _get_current_price(self, trade: PaperTrade) -> Optional[float]:
        """Get current *executable* market price (0-1) for an open trade.

        Important: For Polymarket we may be behind geo restrictions in this container,
        so we rely on the `market_prices` table (fed by shard + polymarket_monitor) rather
        than calling Polymarket directly.
        """
        pool = await get_pool()

        # Try to keep exits team-consistent.
        # Polymarket moneyline has one condition_id shared by BOTH teams; we store both rows under the same market_id.
        # If we select only by (platform, market_id), we can accidentally pick the OTHER team's row and instantly stop out.
        team_hint: Optional[str] = None
        title = (trade.market_title or "").strip()
        if "[" in title and "]" in title:
            # Prefer bracket form: "Game Title [Team Name]"
            try:
                team_hint = title.rsplit("[", 1)[-1].split("]", 1)[0].strip()
            except Exception:
                team_hint = None
        if not team_hint and " (inverted from " in title:
            # "Team (inverted from Other Team)"
            team_hint = title.split(" (inverted from ", 1)[0].strip()

        row = None
        if team_hint:
            row = await pool.fetchrow(
                """
                SELECT yes_bid, yes_ask, market_title, time
                FROM market_prices
                WHERE platform = $1 AND market_id = $2
                  AND (market_title ILIKE $3 OR market_title ILIKE $4)
                ORDER BY time DESC
                LIMIT 1
                """,
                trade.platform.value,
                trade.market_id,
                f"%[{team_hint}]%",
                f"%{team_hint}%",
            )

        if not row:
            # Fallback: last known row for this market_id with team safety check
            # Add contract_team filter to prevent picking wrong team's price
            row = await pool.fetchrow(
                """
                SELECT yes_bid, yes_ask, market_title, contract_team, time
                FROM market_prices
                WHERE platform = $1 AND market_id = $2
                  AND (contract_team IS NULL OR contract_team ILIKE $3 OR $3 IS NULL)
                ORDER BY time DESC
                LIMIT 1
                """,
                trade.platform.value,
                trade.market_id,
                f"%{team_hint}%" if team_hint else None,
            )
        if not row:
            return None
        bid = float(row["yes_bid"])
        ask = float(row["yes_ask"])
        mid = (bid + ask) / 2.0

        # Use executable price (this avoids "undeserved positive PnL" from exiting at mid).
        # - BUY (long YES): you can exit by SELLING YES at the bid
        # - SELL (we model as long NO via YES-space): you exit by BUYING YES at the ask
        if trade.side == TradeSide.BUY:
            chosen = bid
            chosen_kind = "yes_bid"
        else:
            chosen = ask
            chosen_kind = "yes_ask"

        # region agent log
        _agent_dbg(
            "H5",
            "services/position_manager/position_manager.py:_get_current_price",
            "exit_price_lookup",
            {
                "trade_id": trade.trade_id,
                "trade_game_id": trade.game_id,
                "trade_platform": trade.platform.value,
                "trade_market_id": trade.market_id,
                "trade_market_title": trade.market_title,
                "trade_side": trade.side.value,
                "team_hint": team_hint,
                "team_filtered": bool(team_hint),
                "row_market_title": row.get("market_title"),
                "row_time": str(row.get("time")),
                "row_yes_bid": bid,
                "row_yes_ask": ask,
                "row_mid": mid,
                "chosen_exit": chosen,
                "chosen_kind": chosen_kind,
            },
        )
        # endregion

        return chosen

    async def _execute_exit(
        self,
        trade: PaperTrade,
        current_price: float,
        reason: str
    ) -> None:
        """Execute position exit."""
        logger.info(
            f"EXIT {_side_display(trade.side.value)} {trade.game_id}: "
            f"entry_price={trade.entry_price*100:.1f}% -> current={current_price*100:.1f}% "
            f"({reason})"
        )

        # current_price is already executable (bid for BUY, ask for SELL) per _get_current_price()
        closed_trade = await self.paper_engine.close_trade(trade, current_price, already_executable=True)

        # Structured closure logging for verification
        logger.info(
            f"TRADE_CLOSED | trade_id={closed_trade.trade_id} | "
            f"game_id={closed_trade.game_id} | side={closed_trade.side.value} | "
            f"entry={closed_trade.entry_price:.3f} | exit={current_price:.3f} | "
            f"reason={reason} | pnl=${closed_trade.pnl:.2f} | outcome={closed_trade.outcome.value}"
        )

        # Record cooldown for this game
        from arbees_shared.models.trade import TradeOutcome
        self._record_trade_close_for_cooldown(
            closed_trade.game_id,
            closed_trade.outcome == TradeOutcome.WIN
        )

    async def _handle_game_state_update(self, channel: str, data: dict) -> None:
        """Handle real-time game state updates.

        We no longer use win probability as a proxy for market price for exits. The polling
        loop (`_position_monitor_loop`) evaluates exits against actual market prices.
        """
        if not self.paper_engine:
            return
        return


async def main():
    """Main entry point."""
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    # Build sport-specific stop-loss overrides from environment
    sport_stop_loss = SPORT_STOP_LOSS_DEFAULTS.copy()
    for sport in SPORT_STOP_LOSS_DEFAULTS:
        env_key = f"STOP_LOSS_{sport}"
        if env_val := os.environ.get(env_key):
            sport_stop_loss[sport] = float(env_val)

    manager = PositionManager(
        # Trading parameters
        initial_bankroll=float(os.environ.get("INITIAL_BANKROLL", "1000")),
        min_edge_pct=float(os.environ.get("MIN_EDGE_PCT", "2.0")),
        kelly_fraction=float(os.environ.get("KELLY_FRACTION", "0.25")),
        max_position_pct=float(os.environ.get("MAX_POSITION_PCT", "10.0")),
        max_buy_prob=float(os.environ.get("MAX_BUY_PROB", "0.95")),  # Don't buy above 95%
        min_sell_prob=float(os.environ.get("MIN_SELL_PROB", "0.05")),  # Don't sell below 5%
        allow_hedging=os.environ.get("ALLOW_HEDGING", "false").lower() in ("1", "true", "yes", "y", "on"),
        # Risk management parameters
        max_daily_loss=float(os.environ.get("MAX_DAILY_LOSS", "100.0")),  # $100 max daily loss
        max_game_exposure=float(os.environ.get("MAX_GAME_EXPOSURE", "50.0")),  # $50 max per game
        max_sport_exposure=float(os.environ.get("MAX_SPORT_EXPOSURE", "200.0")),  # $200 max per sport
        max_latency_ms=float(os.environ.get("MAX_LATENCY_MS", "5000.0")),  # 5 second max latency
        # Exit monitoring parameters
        take_profit_pct=float(os.environ.get("TAKE_PROFIT_PCT", "3.0")),  # Exit on 3% profit
        default_stop_loss_pct=float(os.environ.get("DEFAULT_STOP_LOSS_PCT", "5.0")),  # Fallback stop-loss
        exit_check_interval=float(os.environ.get("EXIT_CHECK_INTERVAL", "1.0")),  # Check every 1 second
        sport_stop_loss=sport_stop_loss,
        # Cooldown parameters (prevent rapid re-entry on same game)
        win_cooldown_seconds=float(os.environ.get("WIN_COOLDOWN_SECONDS", "180.0")),  # 3 min after win
        loss_cooldown_seconds=float(os.environ.get("LOSS_COOLDOWN_SECONDS", "300.0")),  # 5 min after loss
    )

    await manager.start()

    # Keep running
    try:
        while True:
            await asyncio.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        await manager.stop()


if __name__ == "__main__":
    asyncio.run(main())
