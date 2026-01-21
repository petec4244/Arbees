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
from datetime import datetime
from typing import AsyncIterator, Optional

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
            PaperTrade if executed, None if rejected
        """
        # Validate edge
        if signal.edge_pct < self.min_edge_pct:
            logger.info(f"Signal rejected: edge {signal.edge_pct:.1f}% < min {self.min_edge_pct}%")
            return None

        # Determine side and price
        side = TradeSide.BUY if signal.direction == SignalDirection.BUY else TradeSide.SELL
        entry_price = market_price.yes_ask if side == TradeSide.BUY else market_price.yes_bid

        # Apply slippage
        exec_price = self.apply_slippage(entry_price, side)

        # Calculate position size
        size = self.calculate_position_size(signal, exec_price)

        if size < 1.0:
            logger.info(f"Signal rejected: position size too small (${size:.2f}) - model_prob={signal.model_prob:.3f}, edge={signal.edge_pct:.1f}%")
            return None

        # Check available balance
        cost = size * exec_price if side == TradeSide.BUY else size * (1.0 - exec_price)
        if cost > self.available_balance:
            logger.warning(f"Signal rejected: insufficient balance ({cost} > {self.available_balance})")
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
        )

        # Update bankroll
        self._update_bankroll_for_entry(trade)

        # Track trade
        self._trades.append(trade)

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
    ) -> PaperTrade:
        """
        Close an open trade.

        Args:
            trade: Trade to close
            exit_price: Exit price
            outcome: Trade outcome (win/loss/push)

        Returns:
            Updated trade with PnL
        """
        # Apply slippage to exit
        side = TradeSide.SELL if trade.side == TradeSide.BUY else TradeSide.BUY
        exec_price = self.apply_slippage(exit_price, side)

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
            exit_time=datetime.utcnow(),
            status=TradeStatus.CLOSED,
            outcome=outcome,
            entry_fees=trade.entry_fees,
            exit_fees=0.0,
        )

        # Update bankroll
        self._update_bankroll_for_exit(closed_trade)

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
                exit_time=closed_trade.exit_time if closed_trade.exit_time else datetime.utcnow(),
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

        new_bankroll = Bankroll(
            initial_balance=self._bankroll.initial_balance,
            current_balance=self._bankroll.current_balance,
            reserved_balance=self._bankroll.reserved_balance + cost,
            peak_balance=self._bankroll.peak_balance,
            trough_balance=self._bankroll.trough_balance,
        )
        self._bankroll = new_bankroll

    def _update_bankroll_for_exit(self, trade: PaperTrade) -> None:
        """Update bankroll when closing a trade."""
        if trade.pnl is None:
            return

        cost = trade.size * trade.entry_price if trade.side == TradeSide.BUY else trade.size * (1.0 - trade.entry_price)
        new_balance = self._bankroll.current_balance + trade.pnl

        new_bankroll = Bankroll(
            initial_balance=self._bankroll.initial_balance,
            current_balance=new_balance,
            reserved_balance=max(0, self._bankroll.reserved_balance - cost),
            peak_balance=max(self._bankroll.peak_balance, new_balance),
            trough_balance=min(self._bankroll.trough_balance, new_balance),
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
