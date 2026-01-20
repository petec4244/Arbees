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

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.models.game import Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection, ArbitrageOpportunity
from arbees_shared.models.trade import TradeStatus
from arbees_shared.risk import RiskController, RiskDecision, RiskRejection
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from markets.paper.engine import PaperTradingEngine

logger = logging.getLogger(__name__)


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
        max_buy_prob: float = 0.85,  # Don't buy above 85% - limited upside
        min_sell_prob: float = 0.15,  # Don't sell below 15% - limited upside
        # Risk management settings
        max_daily_loss: float = 100.0,
        max_game_exposure: float = 50.0,
        max_sport_exposure: float = 200.0,
        max_latency_ms: float = 5000.0,
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
        """
        self.initial_bankroll = initial_bankroll
        self.min_edge_pct = min_edge_pct
        self.kelly_fraction = kelly_fraction
        self.max_position_pct = max_position_pct
        self.max_buy_prob = max_buy_prob
        self.min_sell_prob = min_sell_prob

        # Risk management settings
        self.max_daily_loss = max_daily_loss
        self.max_game_exposure = max_game_exposure
        self.max_sport_exposure = max_sport_exposure
        self.max_latency_ms = max_latency_ms

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

        logger.info("Position Manager started")

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
            self.paper_engine._bankroll = Bankroll(
                initial_balance=float(row["initial_balance"]),
                current_balance=float(row["current_balance"]),
                peak_balance=float(row["peak_balance"]),
                trough_balance=float(row["trough_balance"]),
            )
            logger.info(f"Loaded bankroll: ${self.paper_engine._bankroll.current_balance:.2f}")
        else:
            # Create initial bankroll record
            await pool.execute("""
                INSERT INTO bankroll (account_name, initial_balance, current_balance, peak_balance, trough_balance)
                VALUES ('default', $1, $1, $1, $1)
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
            SET current_balance = $1, peak_balance = $2, trough_balance = $3, updated_at = NOW()
            WHERE account_name = 'default'
        """, bankroll.current_balance, bankroll.peak_balance, bankroll.trough_balance)

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                risk_metrics = None
                if self.risk_controller:
                    risk_metrics = await self.risk_controller.get_metrics()

                status = {
                    "type": "position_manager",
                    "signals_received": self._signal_count,
                    "trades_executed": self._trade_count,
                    "risk_rejected": self._risk_rejected_count,
                    "arb_opportunities": self._arb_count,
                    "bankroll": self.paper_engine._bankroll.current_balance if self.paper_engine else 0,
                    "open_positions": len(self.paper_engine.get_open_trades()) if self.paper_engine else 0,
                    "daily_pnl": risk_metrics.daily_pnl if risk_metrics else 0,
                    "circuit_breaker": risk_metrics.circuit_breaker_open if risk_metrics else False,
                    "timestamp": datetime.utcnow().isoformat(),
                }
                logger.info(
                    f"Position Manager status: {status['trades_executed']} trades, "
                    f"{status['risk_rejected']} risk-rejected, "
                    f"${status['bankroll']:.2f} bankroll, "
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

            # Skip if edge below threshold
            if signal.edge_pct < self.min_edge_pct:
                logger.debug(f"Signal rejected: edge {signal.edge_pct}% < min {self.min_edge_pct}%")
                return

            # Probability guardrails - avoid extreme probabilities with poor risk/reward
            if signal.direction == SignalDirection.BUY and signal.model_prob > self.max_buy_prob:
                logger.info(f"Signal rejected: BUY at {signal.model_prob*100:.1f}% > max {self.max_buy_prob*100:.0f}% (poor risk/reward)")
                return
            if signal.direction == SignalDirection.SELL and signal.model_prob < self.min_sell_prob:
                logger.info(f"Signal rejected: SELL at {signal.model_prob*100:.1f}% < min {self.min_sell_prob*100:.0f}% (poor risk/reward)")
                return

            # Check for existing position on this game
            existing_position = await self._get_open_position_for_game(signal.game_id)

            if existing_position:
                existing_side = existing_position["side"]
                new_side = "buy" if signal.direction == SignalDirection.BUY else "sell"

                if existing_side == new_side:
                    # Same direction - skip, don't double down
                    logger.info(f"Skipping signal: already have {existing_side.upper()} position on game {signal.game_id}")
                    return
                else:
                    # Opposite direction - close existing position instead of opening new one
                    logger.info(f"Closing existing {existing_side.upper()} position on game {signal.game_id} (opposite signal received)")
                    await self._close_position(existing_position, signal)
                    return

            # No existing position - open new one
            # Get market price for execution
            market_price = await self._get_market_price(signal)
            if not market_price:
                logger.warning(f"No market price available for signal {signal.signal_id}")
                # Create synthetic market price for paper trading
                market_price = self._create_synthetic_price(signal)

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
                logger.info(f"Opening new position with price: bid={market_price.yes_bid:.3f}, ask={market_price.yes_ask:.3f}")
                trade = await self.paper_engine.execute_signal(signal, market_price)
                if trade:
                    self._trade_count += 1
                    logger.info(f"Opened trade: {trade.trade_id}")
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

            logger.info(f"Closed position {position['trade_id']}: {position['side'].upper()} -> PnL ${pnl:.2f} ({outcome})")
            return

        # Close using paper engine
        exit_price = signal.model_prob
        closed_trade = await self.paper_engine.close_trade(trade_to_close, exit_price)
        logger.info(f"Closed position {closed_trade.trade_id}: PnL ${closed_trade.pnl:.2f} ({closed_trade.outcome.value})")

    async def _get_market_price(self, signal: TradingSignal) -> Optional[MarketPrice]:
        """Get current market price for a signal."""
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
                            return price
                except Exception as e:
                    logger.warning(f"Error getting Kalshi price: {e}")

        # Try Polymarket
        if self.polymarket:
            try:
                pool = await get_pool()
                row = await pool.fetchrow("""
                    SELECT market_id FROM market_prices
                    WHERE game_id = $1 AND platform = 'polymarket'
                    ORDER BY time DESC LIMIT 1
                """, signal.game_id)
                if row:
                    price = await self.polymarket.get_market_price(row["market_id"])
                    if price:
                        return price
            except Exception as e:
                logger.warning(f"Error getting Polymarket price: {e}")

        return None

    def _create_synthetic_price(self, signal: TradingSignal) -> MarketPrice:
        """Create synthetic market price for paper trading when no real price available."""
        # Use model probability as market price proxy
        model_prob = signal.model_prob
        return MarketPrice(
            market_id=f"synthetic_{signal.game_id}",
            platform=Platform.PAPER,
            market_title=f"{signal.team} to win",
            yes_bid=max(0.01, model_prob - 0.02),
            yes_ask=min(0.99, model_prob + 0.02),
            volume=0,
            liquidity=10000,  # Assume unlimited liquidity for paper trading
        )

    async def _handle_game_ended(self, data: dict) -> None:
        """Handle game ending - close any open positions."""
        game_id = data.get("game_id")
        if not game_id or not self.paper_engine:
            return

        try:
            # Get final score info
            home_score = data.get("home_score", 0)
            away_score = data.get("away_score", 0)
            home_won = home_score > away_score

            # Find open trades for this game
            open_trades = self.paper_engine.get_open_trades()
            game_trades = [t for t in open_trades if t.game_id == game_id]

            for trade in game_trades:
                # Determine if trade won (simplified: assumes YES on home team)
                # TODO: Properly track which team the trade was on
                exit_price = 1.0 if home_won else 0.0

                await self.paper_engine.close_trade(trade, exit_price)
                logger.info(f"Closed trade {trade.trade_id} for game {game_id}")

        except Exception as e:
            logger.error(f"Error closing trades for game {game_id}: {e}")

    async def _arbitrage_scan_loop(self) -> None:
        """Periodically scan for arbitrage opportunities across markets."""
        while self._running:
            try:
                await self._scan_for_arbitrage()
            except Exception as e:
                logger.error(f"Error in arbitrage scan: {e}")

            await asyncio.sleep(10)  # Scan every 10 seconds

    async def _scan_for_arbitrage(self) -> None:
        """Scan market prices for arbitrage opportunities."""
        if not self.db:
            return

        pool = await get_pool()

        # Get recent prices from both platforms
        rows = await pool.fetch("""
            WITH recent_prices AS (
                SELECT DISTINCT ON (game_id, platform)
                    game_id, platform, market_id, yes_bid, yes_ask, volume, liquidity, time
                FROM market_prices
                WHERE time > NOW() - INTERVAL '5 minutes'
                ORDER BY game_id, platform, time DESC
            )
            SELECT
                k.game_id,
                k.market_id as kalshi_market,
                k.yes_bid as kalshi_bid,
                k.yes_ask as kalshi_ask,
                k.liquidity as kalshi_liquidity,
                p.market_id as poly_market,
                p.yes_bid as poly_bid,
                p.yes_ask as poly_ask,
                p.liquidity as poly_liquidity
            FROM recent_prices k
            JOIN recent_prices p ON k.game_id = p.game_id
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


async def main():
    """Main entry point."""
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    manager = PositionManager(
        # Trading parameters
        initial_bankroll=float(os.environ.get("INITIAL_BANKROLL", "1000")),
        min_edge_pct=float(os.environ.get("MIN_EDGE_PCT", "2.0")),
        kelly_fraction=float(os.environ.get("KELLY_FRACTION", "0.25")),
        max_position_pct=float(os.environ.get("MAX_POSITION_PCT", "10.0")),
        max_buy_prob=float(os.environ.get("MAX_BUY_PROB", "0.85")),  # Don't buy above 85%
        min_sell_prob=float(os.environ.get("MIN_SELL_PROB", "0.15")),  # Don't sell below 15%
        # Risk management parameters
        max_daily_loss=float(os.environ.get("MAX_DAILY_LOSS", "100.0")),  # $100 max daily loss
        max_game_exposure=float(os.environ.get("MAX_GAME_EXPOSURE", "50.0")),  # $50 max per game
        max_sport_exposure=float(os.environ.get("MAX_SPORT_EXPOSURE", "200.0")),  # $200 max per sport
        max_latency_ms=float(os.environ.get("MAX_LATENCY_MS", "5000.0")),  # 5 second max latency
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
