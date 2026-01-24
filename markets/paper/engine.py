"""
Paper trading engine for simulation and backtesting.

Features:
- Kelly criterion position sizing
- Execution tracking with slippage modeling
- PnL calculation and reporting
- Integration with TimescaleDB for persistence
"""

import asyncio
import logging
import uuid
from datetime import datetime, timezone
from typing import AsyncIterator, Optional

import json
import os
import time

from arbees_shared.db.connection import DatabaseClient
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.models.game import Sport
from arbees_shared.models.market import MarketPrice, OrderBook, OrderBookLevel, Platform
from arbees_shared.models.signal import TradingSignal, SignalDirection
from arbees_shared.models.trade import (
    Bankroll,
    PaperTrade,
    PerformanceStats,
    Position,
    TradeOutcome,
    TradeSide,
    TradeStatus,
)
from markets.base import BaseMarketClient

logger = logging.getLogger(__name__)

# Import shared trace logger
from arbees_shared.utils.trace_logger import trace_log


# region agent log (helper) - kept for backwards compat, delegates to trace_log
def _agent_dbg(hypothesisId: str, location: str, message: str, data: dict) -> None:
    """Write a single NDJSON debug line to the host-mounted .cursor/debug.log (DEBUG MODE ONLY)."""
    trace_log(
        service="paper_engine",
        event=message,
        hypothesis_id=hypothesisId,
        location=location,
        **data,
    )
# endregion


def _side_display(side: str) -> str:
    """Convert buy/sell to HOME/AWAY for display."""
    return "HOME" if side == "buy" else "AWAY"


class PaperTradingEngine(BaseMarketClient):
    """Paper trading engine for simulated trading."""

    def __init__(
        self,
        initial_bankroll: float = 1000.0,
        min_edge_pct: float = 2.0,
        kelly_fraction: float = 0.25,
        max_position_pct: float = 10.0,
        slippage_pct: float = 0.5,
        db_client: Optional[DatabaseClient] = None,
        redis_bus: Optional[RedisBus] = None,
    ):
        """
        Initialize paper trading engine.

        Args:
            initial_bankroll: Starting capital
            min_edge_pct: Minimum edge to take a trade
            kelly_fraction: Fraction of Kelly criterion to use
            max_position_pct: Maximum position size as % of bankroll
            slippage_pct: Assumed slippage on execution
            db_client: Optional database client for persistence
            redis_bus: Optional Redis bus for event publishing
        """
        super().__init__(
            base_url="paper://localhost",
            platform=Platform.PAPER,
        )

        self.min_edge_pct = min_edge_pct
        self.kelly_fraction = kelly_fraction
        self.max_position_pct = max_position_pct
        self.slippage_pct = slippage_pct

        self.db = db_client
        self.redis = redis_bus

        # State
        self._bankroll = Bankroll(
            initial_balance=initial_bankroll,
            current_balance=initial_bankroll,
            peak_balance=initial_bankroll,
            trough_balance=initial_bankroll,
        )
        self._positions: dict[str, Position] = {}
        self._trades: list[PaperTrade] = []
        self._pending_orders: dict[str, dict] = {}
        self._last_rejection_reason: Optional[str] = None  # Set when execute_signal returns None

    @property
    def bankroll(self) -> Bankroll:
        """Get current bankroll state."""
        return self._bankroll

    @property
    def available_balance(self) -> float:
        """Get available balance for trading."""
        return self._bankroll.available_balance

    # ==========================================================================
    # Position Sizing
    # ==========================================================================

    def calculate_kelly_bet(self, edge_pct: float, win_prob: float) -> float:
        """
        Calculate Kelly criterion bet size for prediction markets.

        For prediction markets, Kelly = edge / variance where:
        - edge = edge_pct / 100 (the expected profit per dollar bet)
        - variance = p * (1 - p) (Bernoulli variance)

        This simplifies to: kelly = edge / (p * (1-p))

        Args:
            edge_pct: Edge in percentage points (e.g., 4.5 for 4.5% edge)
            win_prob: Model's probability of winning (0-1)

        Returns:
            Optimal bet as fraction of bankroll
        """
        if edge_pct <= 0 or win_prob <= 0 or win_prob >= 1:
            return 0.0

        # Convert edge to decimal
        edge = edge_pct / 100.0

        # Variance of Bernoulli outcome
        variance = win_prob * (1.0 - win_prob)

        # Full Kelly = edge / variance
        # This can be aggressive, so we apply kelly_fraction
        full_kelly = edge / variance if variance > 0 else 0.0

        # Cap at reasonable maximum (50% of bankroll before kelly_fraction)
        full_kelly = min(full_kelly, 0.5)

        return max(0.0, full_kelly * self.kelly_fraction)

    def calculate_position_size(
        self,
        signal: TradingSignal,
        current_price: float,
    ) -> float:
        """
        Calculate position size for a signal.

        Args:
            signal: Trading signal
            current_price: Current market price

        Returns:
            Position size in dollars
        """
        # Calculate Kelly fraction
        kelly = self.calculate_kelly_bet(signal.edge_pct, signal.model_prob)

        # Apply to available balance
        kelly_size = self.available_balance * kelly

        # Apply max position limit
        max_size = self.available_balance * (self.max_position_pct / 100.0)
        size = min(kelly_size, max_size)

        # Round to reasonable amount
        return round(size, 2)

    # ==========================================================================
    # Trade Execution
    # ==========================================================================

    def apply_slippage(self, price: float, side: TradeSide) -> float:
        """Apply slippage to execution price."""
        slip = self.slippage_pct / 100.0
        if side == TradeSide.BUY:
            return min(1.0, price + slip)
        return max(0.0, price - slip)

    @staticmethod
    def _kalshi_fee_cents(price: float) -> int:
        """Kalshi fee per contract in cents for a given price (0-1)."""
        price_cents = int(round(price * 100))
        if price_cents <= 0 or price_cents >= 100:
            return 0
        numerator = 7 * price_cents * (100 - price_cents) + 9999
        return numerator // 10000

    def _estimate_kalshi_fees(self, price: float, size: float) -> float:
        """Estimate total Kalshi fees in dollars for a trade of `size` contracts."""
        fee_cents = self._kalshi_fee_cents(price)
        return (fee_cents / 100.0) * size

    def _reject(self, reason: str) -> None:
        """Log rejection and store reason for caller inspection."""
        logger.info(f"Signal rejected: {reason}")
        self._last_rejection_reason = reason

    async def execute_signal(
        self,
        signal: TradingSignal,
        market_price: MarketPrice,
    ) -> Optional[PaperTrade]:
        """
        Execute a trading signal.

        Args:
            signal: Trading signal to execute
            market_price: Current market price

        Returns:
            PaperTrade if executed, None if rejected.
            Check self._last_rejection_reason for details if None.
        """
        # Clear previous rejection reason
        self._last_rejection_reason = None

        # Validate edge
        if signal.edge_pct < self.min_edge_pct:
            self._reject(f"edge {signal.edge_pct:.1f}% < min {self.min_edge_pct}%")
            return None

        # Determine side and price
        side = TradeSide.BUY if signal.direction == SignalDirection.BUY else TradeSide.SELL

        # Check for duplicate position on same platform/market/side
        if self.db:
            existing = await self._get_open_position(
                platform=market_price.platform.value,
                market_id=str(market_price.market_id),
                side=side.value,
            )
            if existing:
                self._reject(
                    f"duplicate position - already have {side.value} on "
                    f"{market_price.platform.value}:{market_price.market_id} "
                    f"({signal.game_id} {signal.team}) (trade_id={existing.get('trade_id')})"
                )
                return None

        # Determine entry price (side already computed above)
        entry_price = market_price.yes_ask if side == TradeSide.BUY else market_price.yes_bid

        # Guardrail: if orderbook is effectively empty/unknown (defaults 0/1), do not trade.
        # This prevents pathological entries like BUY @ 1.000 (100%) due to missing asks.
        if market_price.yes_bid <= 0.0 and market_price.yes_ask >= 1.0:
            self._reject(
                f"invalid market book (bid={market_price.yes_bid:.3f}, ask={market_price.yes_ask:.3f}) "
                f"for {signal.game_id} {signal.team}"
            )
            # region agent log
            _agent_dbg(
                "H1",
                "markets/paper/engine.py:execute_signal",
                "rejected_empty_book",
                {
                    "signal_id": signal.signal_id,
                    "game_id": signal.game_id,
                    "signal_team": signal.team,
                    "direction": signal.direction.value,
                    "market_id": market_price.market_id,
                    "platform": market_price.platform.value,
                    "contract_team": getattr(market_price, "contract_team", None),
                    "yes_bid": float(market_price.yes_bid),
                    "yes_ask": float(market_price.yes_ask),
                    "mid": float(market_price.mid_price),
                },
            )
            # endregion
            return None

        # region agent log
        _agent_dbg(
            "H1",
            "markets/paper/engine.py:execute_signal",
            "paper_entry_price_selected",
            {
                "signal_id": signal.signal_id,
                "game_id": signal.game_id,
                "signal_team": signal.team,
                "direction": signal.direction.value,
                "side": side.value,
                "market_id": market_price.market_id,
                "platform": market_price.platform.value,
                "contract_team": getattr(market_price, "contract_team", None),
                "yes_bid": float(market_price.yes_bid),
                "yes_ask": float(market_price.yes_ask),
                "mid": float(market_price.mid_price),
                "entry_price_pre_slip": float(entry_price),
            },
        )
        # endregion

        # Apply slippage
        exec_price = self.apply_slippage(entry_price, side)

        # Calculate position size
        size = self.calculate_position_size(signal, exec_price)

        if size < 1.0:
            self._reject(f"position size too small (${size:.2f}) - model_prob={signal.model_prob:.3f}, edge={signal.edge_pct:.1f}%")
            return None

        # Enforce depth at best price when available (strict for Polymarket)
        bid_size = float(getattr(market_price, "yes_bid_size", 0.0) or 0.0)
        ask_size = float(getattr(market_price, "yes_ask_size", 0.0) or 0.0)
        if market_price.platform == Platform.POLYMARKET:
            if side == TradeSide.BUY:
                if ask_size <= 0:
                    self._reject(f"missing ask depth for BUY ({signal.game_id} {signal.team})")
                    return None
                if size > ask_size:
                    self._reject(f"insufficient ask depth ({size:.2f} > {ask_size:.2f})")
                    return None
            else:
                if bid_size <= 0:
                    self._reject(f"missing bid depth for SELL ({signal.game_id} {signal.team})")
                    return None
                if size > bid_size:
                    self._reject(f"insufficient bid depth ({size:.2f} > {bid_size:.2f})")
                    return None

        # Estimate entry fees (Kalshi only)
        entry_fees = 0.0
        if market_price.platform == Platform.KALSHI:
            entry_fees = self._estimate_kalshi_fees(exec_price, size)

        # region agent log
        _agent_dbg(
            "H7",
            "markets/paper/engine.py:execute_signal",
            "depth_and_fee_snapshot",
            {
                "signal_id": signal.signal_id,
                "game_id": signal.game_id,
                "market_id": market_price.market_id,
                "platform": market_price.platform.value,
                "side": side.value,
                "size": float(size),
                "exec_price": float(exec_price),
                "yes_bid": float(market_price.yes_bid),
                "yes_ask": float(market_price.yes_ask),
                "yes_bid_size": float(getattr(market_price, "yes_bid_size", 0.0) or 0.0),
                "yes_ask_size": float(getattr(market_price, "yes_ask_size", 0.0) or 0.0),
                "entry_fees": float(entry_fees),
                "exit_fees": 0.0,
            },
        )
        # endregion

        # Check available balance
        cost = size * exec_price if side == TradeSide.BUY else size * (1.0 - exec_price)
        cost += entry_fees
        if cost > self.available_balance:
            self._reject(f"insufficient balance (${cost:.2f} > ${self.available_balance:.2f})")
            return None

        # Create trade
        trade = PaperTrade(
            trade_id=str(uuid.uuid4()),
            signal_id=signal.signal_id,
            game_id=signal.game_id,
            sport=signal.sport,
            platform=market_price.platform,
            market_id=market_price.market_id,
            market_title=market_price.market_title,
            side=side,
            signal_type=signal.signal_type,
            entry_price=exec_price,
            size=size,
            model_prob=signal.model_prob,
            edge_at_entry=signal.edge_pct,
            kelly_fraction=self.kelly_fraction,
            status=TradeStatus.OPEN,
            entry_time=datetime.now(timezone.utc),
            entry_fees=entry_fees,
        )

        # Update bankroll
        self._update_bankroll_for_entry(trade)

        # Track trade
        self._trades.append(trade)

        # Log detailed trade entry with model/market probs
        trace_log(
            service="paper_engine",
            event="trade_opened",
            trade_id=trade.trade_id,
            signal_id=trade.signal_id,
            game_id=trade.game_id,
            sport=trade.sport.value if trade.sport else None,
            platform=trade.platform.value,
            market_id=trade.market_id,
            market_title=trade.market_title,
            contract_team=market_price.contract_team,
            side=trade.side.value,
            entry_price=trade.entry_price,
            size=trade.size,
            model_prob=signal.model_prob,
            market_prob=signal.market_prob,
            edge_at_entry=trade.edge_at_entry,
            kelly_fraction=trade.kelly_fraction,
            yes_bid=market_price.yes_bid,
            yes_ask=market_price.yes_ask,
            entry_fees=trade.entry_fees,
        )

        # Persist to database
        if self.db:
            await self.db.insert_paper_trade(
                trade_id=trade.trade_id,
                signal_id=trade.signal_id,
                game_id=trade.game_id,
                sport=trade.sport.value if trade.sport else None,
                platform=trade.platform.value,
                market_id=trade.market_id,
                market_title=trade.market_title,
                side=trade.side.value,
                signal_type=trade.signal_type.value if trade.signal_type else None,
                entry_price=trade.entry_price,
                size=trade.size,
                model_prob=trade.model_prob,
                edge_at_entry=trade.edge_at_entry,
                kelly_fraction=trade.kelly_fraction,
                entry_time=trade.entry_time,  # Pass datetime object, not string
            )

        # Publish event
        if self.redis:
            await self.redis.publish_trade_opened(trade)

        logger.info(
            f"Opened trade: {_side_display(trade.side.value)} ${trade.size:.2f} @ {trade.entry_price:.3f} "
            f"(edge: {trade.edge_at_entry:.1f}%)"
        )

        return trade

    async def close_trade(
        self,
        trade: PaperTrade,
        exit_price: float,
        outcome: Optional[TradeOutcome] = None,
        is_game_settlement: bool = False,
        already_executable: bool = False,
    ) -> PaperTrade:
        """
        Close an open trade.

        Args:
            trade: Trade to close
            exit_price: Exit price
            outcome: Trade outcome (win/loss/push)
            is_game_settlement: If True, skip slippage (game ended at known price)
            already_executable: If True, caller provided an executable price (bid/ask as appropriate),
                               so do not apply additional slippage here.

        Returns:
            Updated trade with PnL
        """
        # Apply slippage only for early exits, NOT for game-end settlements.
        # If the caller already provided an executable price (bid/ask), don't apply extra slippage.
        if is_game_settlement or already_executable:
            exec_price = exit_price
        else:
            side = TradeSide.SELL if trade.side == TradeSide.BUY else TradeSide.BUY
            exec_price = self.apply_slippage(exit_price, side)

        # Estimate exit fees (Kalshi only, not on settlement)
        exit_fees = 0.0
        if trade.platform == Platform.KALSHI and not is_game_settlement:
            exit_fees = self._estimate_kalshi_fees(exec_price, trade.size)

        # Determine outcome if not provided
        if outcome is None:
            if trade.side == TradeSide.BUY:
                pnl = trade.size * (exec_price - trade.entry_price)
            else:
                pnl = trade.size * (trade.entry_price - exec_price)

            if pnl > 0:
                outcome = TradeOutcome.WIN
            elif pnl < 0:
                outcome = TradeOutcome.LOSS
            else:
                outcome = TradeOutcome.PUSH

        # Create closed trade (immutable, so create new)
        closed_trade = PaperTrade(
            trade_id=trade.trade_id,
            signal_id=trade.signal_id,
            game_id=trade.game_id,
            sport=trade.sport,
            platform=trade.platform,
            market_id=trade.market_id,
            market_title=trade.market_title,
            side=trade.side,
            signal_type=trade.signal_type,
            entry_price=trade.entry_price,
            exit_price=exec_price,
            size=trade.size,
            model_prob=trade.model_prob,
            edge_at_entry=trade.edge_at_entry,
            kelly_fraction=trade.kelly_fraction,
            entry_time=trade.entry_time,
            exit_time=datetime.now(timezone.utc),
            status=TradeStatus.CLOSED,
            outcome=outcome,
            entry_fees=trade.entry_fees,
            exit_fees=exit_fees,
        )

        # Update bankroll
        self._update_bankroll_for_exit(closed_trade)

        # region agent log
        _agent_dbg(
            "H7",
            "markets/paper/engine.py:close_trade",
            "close_trade_fees_and_pnl",
            {
                "trade_id": closed_trade.trade_id,
                "game_id": closed_trade.game_id,
                "side": closed_trade.side.value,
                "entry_price": float(closed_trade.entry_price),
                "exit_price": float(exec_price),
                "pnl": float(closed_trade.pnl or 0.0),
                "entry_fees": float(closed_trade.entry_fees or 0.0),
                "exit_fees": float(closed_trade.exit_fees or 0.0),
                "is_game_settlement": is_game_settlement,
                "already_executable": already_executable,
            },
        )
        # endregion

        # Update trade in list
        for i, t in enumerate(self._trades):
            if t.trade_id == trade.trade_id:
                self._trades[i] = closed_trade
                break

        # Persist to database
        if self.db:
            await self.db.close_paper_trade(
                trade_id=closed_trade.trade_id,
                exit_price=exec_price,
                exit_time=closed_trade.exit_time if closed_trade.exit_time else datetime.now(timezone.utc),
                outcome=outcome.value,
            )

        # Publish event
        if self.redis:
            await self.redis.publish_trade_closed(closed_trade)

        logger.info(
            f"Closed trade: PnL ${closed_trade.pnl:.2f} ({closed_trade.pnl_pct:.1f}%) "
            f"[{outcome.value}]"
        )

        return closed_trade

    def _update_bankroll_for_entry(self, trade: PaperTrade) -> None:
        """Update bankroll when opening a trade."""
        cost = trade.size * trade.entry_price if trade.side == TradeSide.BUY else trade.size * (1.0 - trade.entry_price)
        cost += trade.entry_fees or 0.0

        new_bankroll = Bankroll(
            initial_balance=self._bankroll.initial_balance,
            current_balance=self._bankroll.current_balance,
            reserved_balance=self._bankroll.reserved_balance + cost,
            piggybank_balance=self._bankroll.piggybank_balance,  # Preserve piggybank
            peak_balance=self._bankroll.peak_balance,
            trough_balance=self._bankroll.trough_balance,
        )
        self._bankroll = new_bankroll

    def _update_bankroll_for_exit(self, trade: PaperTrade) -> None:
        """Update bankroll when closing a trade.

        Profit split (piggybank system):
        - On WINNING trades: 50% of profit goes to piggybank (protected)
        - The other 50% goes back to current_balance for trading
        - On LOSING trades: Full loss comes from current_balance
        - Piggybank is never touched for losses

        This prevents runaway compounding while still allowing growth.
        """
        if trade.pnl is None:
            return

        cost = trade.size * trade.entry_price if trade.side == TradeSide.BUY else trade.size * (1.0 - trade.entry_price)
        cost += trade.entry_fees or 0.0
        pnl = trade.pnl

        # Calculate new balances with piggybank split
        if pnl > 0:
            # WINNING trade: split profit 50/50
            profit_to_piggybank = pnl * 0.5
            profit_to_trading = pnl * 0.5
            new_current = self._bankroll.current_balance + profit_to_trading
            new_piggybank = self._bankroll.piggybank_balance + profit_to_piggybank
            logger.info(
                f"Profit split: ${pnl:.2f} -> ${profit_to_trading:.2f} to trading, "
                f"${profit_to_piggybank:.2f} to piggybank"
            )
        else:
            # LOSING trade: full loss from current_balance
            new_current = self._bankroll.current_balance + pnl  # pnl is negative
            new_piggybank = self._bankroll.piggybank_balance

        # Calculate total for peak/trough tracking (includes piggybank)
        total_balance = new_current + new_piggybank

        new_bankroll = Bankroll(
            initial_balance=self._bankroll.initial_balance,
            current_balance=new_current,
            reserved_balance=max(0, self._bankroll.reserved_balance - cost),
            piggybank_balance=new_piggybank,
            peak_balance=max(self._bankroll.peak_balance, total_balance),
            trough_balance=min(self._bankroll.trough_balance, total_balance),
        )
        self._bankroll = new_bankroll

    # ==========================================================================
    # Query Methods
    # ==========================================================================

    def get_open_trades(self) -> list[PaperTrade]:
        """Get all open trades."""
        return [t for t in self._trades if t.status == TradeStatus.OPEN]

    def get_closed_trades(self) -> list[PaperTrade]:
        """Get all closed trades."""
        return [t for t in self._trades if t.status == TradeStatus.CLOSED]

    async def _get_open_position(
        self, platform: str, market_id: str, side: str
    ) -> Optional[dict]:
        """Check for existing open position on same platform/market/side."""
        if not self.db:
            return None

        pool = self.db._pool
        row = await pool.fetchrow("""
            SELECT trade_id, game_id, side, entry_price, size, time, platform, market_id
            FROM paper_trades
            WHERE platform = $1
              AND market_id = $2
              AND side = $3
              AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
        """, platform, market_id, side)
        return dict(row) if row else None

    def get_performance_stats(self) -> PerformanceStats:
        """Calculate performance statistics."""
        closed = self.get_closed_trades()

        winning = [t for t in closed if t.outcome == TradeOutcome.WIN]
        losing = [t for t in closed if t.outcome == TradeOutcome.LOSS]

        total_pnl = sum(t.pnl or 0 for t in closed)
        total_fees = sum((t.entry_fees + t.exit_fees) for t in closed)

        arb_trades = [t for t in closed if t.signal_type and "arb" in t.signal_type.value]
        model_trades = [t for t in closed if t.signal_type and "model" in t.signal_type.value]

        return PerformanceStats(
            start_date=min((t.entry_time for t in self._trades), default=datetime.utcnow()),
            total_trades=len(closed),
            winning_trades=len(winning),
            losing_trades=len(losing),
            push_trades=len(closed) - len(winning) - len(losing),
            total_pnl=total_pnl,
            total_fees=total_fees,
            net_pnl=total_pnl - total_fees,
            arb_trades=len(arb_trades),
            arb_pnl=sum(t.pnl or 0 for t in arb_trades),
            model_edge_trades=len(model_trades),
            model_edge_pnl=sum(t.pnl or 0 for t in model_trades),
            starting_bankroll=self._bankroll.initial_balance,
            current_bankroll=self._bankroll.current_balance,
        )

    # ==========================================================================
    # BaseMarketClient Interface
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """Get simulated markets (returns empty for paper trading)."""
        return []

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get simulated market (returns None for paper trading)."""
        return None

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get simulated orderbook."""
        return OrderBook(
            market_id=market_id,
            platform=Platform.PAPER,
            yes_bids=[OrderBookLevel(price=0.50, quantity=1000)],
            yes_asks=[OrderBookLevel(price=0.52, quantity=1000)],
        )

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get simulated market price."""
        return MarketPrice(
            market_id=market_id,
            platform=Platform.PAPER,
            yes_bid=0.50,
            yes_ask=0.52,
            volume=10000,
            liquidity=5000,
        )

    async def stream_prices(
        self,
        market_ids: list[str],
        interval_seconds: float = 5.0,
    ) -> AsyncIterator[MarketPrice]:
        """Stream simulated prices."""
        while True:
            for market_id in market_ids:
                price = await self.get_market_price(market_id)
                if price:
                    yield price
            await asyncio.sleep(interval_seconds)

    async def get_positions(self) -> list[dict]:
        """Get open positions."""
        return [
            {
                "market_id": t.market_id,
                "side": t.side.value,
                "size": t.size,
                "entry_price": t.entry_price,
            }
            for t in self.get_open_trades()
        ]
