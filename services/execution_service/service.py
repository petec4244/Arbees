"""
ExecutionService - Phase 1 split from PositionManager.

Responsibilities:
- Subscribe to ExecutionRequest messages from execution:requests channel
- Place orders (paper or real) via market clients
- Emit ExecutionResult messages to execution:results channel
- Handle in-flight deduplication
- Apply slippage and fee estimation for paper trades
"""

import asyncio
import logging
import os
import time
from datetime import datetime, timezone
from typing import Optional
from uuid import uuid4

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.trade import TradeSide
from arbees_shared.models.execution import (
    ExecutionRequest,
    ExecutionResult,
    ExecutionStatus,
    ExecutionSide,
)
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from arbees_shared.utils.trace_logger import TraceContext
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from markets.paper.engine import PaperTradingEngine

logger = logging.getLogger(__name__)


class ExecutionService:
    """
    Executes trading orders (paper or real).
    
    Subscribes to ExecutionRequest, places orders, emits ExecutionResult.
    """

    def __init__(
        self,
        paper_trading: bool = True,
        initial_bankroll: float = 1000.0,
        min_edge_pct: float = 2.0,
        kelly_fraction: float = 0.25,
        max_position_pct: float = 10.0,
        slippage_bps: float = 10.0,
    ):
        self.paper_trading = paper_trading
        self.initial_bankroll = initial_bankroll
        self.min_edge_pct = min_edge_pct
        self.kelly_fraction = kelly_fraction
        self.max_position_pct = max_position_pct
        self.slippage_bps = slippage_bps

        # Connections
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None
        self.paper_engine: Optional[PaperTradingEngine] = None

        # State
        self._running = False
        self._execution_count = 0
        self._success_count = 0
        self._failure_count = 0

        # In-flight deduplication
        self._in_flight: set[str] = set()
        self._lock = asyncio.Lock()

        # Heartbeat publisher
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

    async def start(self) -> None:
        """Start the execution service."""
        logger.info(f"Starting ExecutionService (paper_trading={self.paper_trading})")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Initialize market clients
        if not self.paper_trading:
            self.kalshi = KalshiClient()
            await self.kalshi.connect()

            self.polymarket = PolymarketClient()
            await self.polymarket.connect()
        else:
            # Paper trading engine
            self.paper_engine = PaperTradingEngine(
                initial_bankroll=self.initial_bankroll,
                min_edge_pct=self.min_edge_pct,
                kelly_fraction=self.kelly_fraction,
                max_position_pct=self.max_position_pct,
                db_client=self.db,
                redis_bus=self.redis,
            )
            await self._load_bankroll()

        self._running = True

        # Subscribe to execution requests
        await self.redis.subscribe(Channel.EXECUTION_REQUESTS.value, self._handle_request)

        # Start listening
        asyncio.create_task(self.redis.start_listening())

        # Start heartbeat
        asyncio.create_task(self._heartbeat_loop())

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="execution_service",
            instance_id=os.environ.get("HOSTNAME", "execution-service-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
            "paper_trading": self.paper_trading,
        })

        logger.info("ExecutionService started")

    async def stop(self) -> None:
        """Stop the execution service."""
        logger.info("Stopping ExecutionService")
        self._running = False

        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        if self.paper_engine:
            await self._save_bankroll()

        if self.kalshi:
            await self.kalshi.disconnect()
        if self.polymarket:
            await self.polymarket.disconnect()
        if self.redis:
            await self.redis.disconnect()

        logger.info("ExecutionService stopped")

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
        else:
            await pool.execute("""
                INSERT INTO bankroll (account_name, initial_balance, current_balance, piggybank_balance, peak_balance, trough_balance)
                VALUES ('default', $1, $1, 0.0, $1, $1)
                ON CONFLICT (account_name) DO NOTHING
            """, self.initial_bankroll)

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

    async def _handle_request(self, data: dict) -> None:
        """Handle incoming execution request."""
        try:
            request = ExecutionRequest(**data)

            # Create trace context
            trace = TraceContext(
                service="execution_service",
                trace_id=request.idempotency_key,
                signal_id=request.signal_id,
                game_id=request.game_id,
                sport=request.sport.value if request.sport else None,
                platform=request.platform.value,
                market_id=request.market_id,
                contract_team=request.contract_team,
                side=request.side.value,
            )

            trace.log(
                "request_received",
                request_id=request.request_id,
                limit_price=request.limit_price,
                size=request.size,
                edge_pct=request.edge_pct,
                model_prob=request.model_prob,
                market_prob=request.market_prob,
            )

            logger.info(
                f"Received ExecutionRequest: {request.request_id} "
                f"({request.side.value} {request.contract_team} @ {request.limit_price:.3f})"
            )

            # Deduplication check
            async with self._lock:
                if request.idempotency_key in self._in_flight:
                    trace.log("request_rejected", reason="duplicate_in_flight")
                    logger.warning(f"Request {request.idempotency_key} already in-flight, skipping")
                    return
                self._in_flight.add(request.idempotency_key)

            self._execution_count += 1
            start_time = time.time()

            try:
                if self.paper_trading:
                    result = await self._execute_paper(request, trace)
                else:
                    result = await self._execute_real(request)

                result_latency = (time.time() - start_time) * 1000

                # Update result with latency
                result = ExecutionResult(
                    **{**result.model_dump(), "latency_ms": result_latency}
                )

                if result.status == ExecutionStatus.FILLED:
                    self._success_count += 1
                    trace.log(
                        "execution_filled",
                        trade_id=result.order_id,
                        avg_price=result.avg_price,
                        filled_qty=result.filled_qty,
                        fees=result.fees,
                        latency_ms=result_latency,
                    )
                    trace.update(trade_id=result.order_id)
                    logger.info(
                        f"Execution SUCCESS: {request.request_id} filled @ {result.avg_price:.3f} "
                        f"(latency={result_latency:.1f}ms)"
                    )
                else:
                    self._failure_count += 1
                    trace.log(
                        "execution_rejected",
                        status=result.status.value,
                        rejection_reason=result.rejection_reason,
                        latency_ms=result_latency,
                    )
                    logger.warning(
                        f"Execution FAILED: {request.request_id} - {result.status.value}: "
                        f"{result.rejection_reason}"
                    )

                # Emit result
                await self.redis.publish(Channel.EXECUTION_RESULTS.value, result)

            finally:
                async with self._lock:
                    self._in_flight.discard(request.idempotency_key)

        except Exception as e:
            logger.error(f"Error handling execution request: {e}", exc_info=True)

    async def _execute_paper(
        self, request: ExecutionRequest, trace: Optional[TraceContext] = None
    ) -> ExecutionResult:
        """Execute a paper trade."""
        if not self.paper_engine:
            return self._create_result(
                request,
                ExecutionStatus.FAILED,
                rejection_reason="Paper engine not initialized",
            )

        # Create a synthetic TradingSignal for paper engine
        from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection

        direction = SignalDirection.BUY if request.side == ExecutionSide.YES else SignalDirection.SELL

        signal = TradingSignal(
            signal_id=request.signal_id,
            signal_type=SignalType(request.signal_type) if request.signal_type else SignalType.WIN_PROB_SHIFT,
            game_id=request.game_id,
            sport=request.sport,
            team=request.contract_team or "",
            direction=direction,
            model_prob=request.model_prob,
            market_prob=request.market_prob,
            edge_pct=request.edge_pct,
            confidence=0.5,
            reason=request.reason,
        )

        # Prefer using the latest real market quote from DB (keeps paper fills consistent
        # with what PositionTracker will later use for exit monitoring).
        yes_bid = None
        yes_ask = None
        price_source = "fallback_synthetic"
        db_price_age_ms = None

        if self.db:
            try:
                row = None
                if request.contract_team:
                    row = await self.db.get_latest_market_price_for_team(
                        market_id=request.market_id,
                        platform=request.platform.value,
                        contract_team=request.contract_team,
                    )
                    if row:
                        price_source = "db_team_match"
                if not row:
                    row = await self.db.get_latest_market_price(
                        market_id=request.market_id,
                        platform=request.platform.value,
                    )
                    if row:
                        price_source = "db_fallback"
                if row:
                    yes_bid = float(row.get("yes_bid")) if row.get("yes_bid") is not None else None
                    yes_ask = float(row.get("yes_ask")) if row.get("yes_ask") is not None else None
                    if row.get("time"):
                        db_price_age_ms = (datetime.now(timezone.utc) - row["time"]).total_seconds() * 1000
            except Exception:
                # Fall back to synthetic pricing
                yes_bid = None
                yes_ask = None
                price_source = "fallback_synthetic"

        # Fallback: synthesize a reasonable spread around the signal's limit_price.
        # Use a *relative* spread so low-probability markets don't get a huge absolute spread.
        if yes_bid is None or yes_ask is None:
            raw = float(request.limit_price)
            spread = max(0.002, min(0.02, raw * 0.20))
            yes_bid = max(0.0, raw - spread)
            yes_ask = min(1.0, raw + spread)
            price_source = "fallback_synthetic"

        # Log the market price being used
        if trace:
            trace.log(
                "market_price_for_fill",
                price_source=price_source,
                yes_bid=yes_bid,
                yes_ask=yes_ask,
                db_price_age_ms=db_price_age_ms,
                limit_price=request.limit_price,
            )

        # Create market price with simulated depth for paper trading
        market_price = MarketPrice(
            market_id=request.market_id,
            platform=request.platform,
            market_title=f"{request.contract_team or ''} to win".strip(),
            contract_team=request.contract_team,
            yes_bid=yes_bid,
            yes_ask=yes_ask,
            yes_bid_size=10000.0,  # Simulated depth for paper trading
            yes_ask_size=10000.0,  # Simulated depth for paper trading
            volume=0,
            liquidity=10000,
        )

        # Execute
        trade = await self.paper_engine.execute_signal(signal, market_price)

        if trade:
            return self._create_result(
                request,
                ExecutionStatus.FILLED,
                order_id=trade.trade_id,
                filled_qty=trade.size,
                avg_price=trade.entry_price,
                fees=trade.entry_fees,
            )
        else:
            # Get specific rejection reason from paper engine
            reason = self.paper_engine._last_rejection_reason or "Paper engine rejected trade"
            return self._create_result(
                request,
                ExecutionStatus.REJECTED,
                rejection_reason=reason,
            )

    async def _execute_real(self, request: ExecutionRequest) -> ExecutionResult:
        """Execute a real trade on Kalshi or Polymarket."""
        try:
            if request.platform == Platform.KALSHI:
                if not self.kalshi:
                    return self._create_result(
                        request,
                        ExecutionStatus.FAILED,
                        rejection_reason="Kalshi client not initialized",
                    )

                # Convert price to cents for Kalshi
                price_cents = int(request.limit_price * 100)
                quantity = int(request.size)

                result = await self.kalshi.place_order(
                    market_id=request.market_id,
                    side=request.side.value,
                    price=price_cents,
                    quantity=quantity,
                )

                if result and result.get("order_id"):
                    return self._create_result(
                        request,
                        ExecutionStatus.FILLED,
                        order_id=result["order_id"],
                        filled_qty=float(result.get("filled_quantity", quantity)),
                        avg_price=float(result.get("avg_price", request.limit_price)),
                    )
                else:
                    return self._create_result(
                        request,
                        ExecutionStatus.REJECTED,
                        rejection_reason=result.get("error", "Unknown Kalshi error") if result else "No response",
                    )

            elif request.platform == Platform.POLYMARKET:
                if not self.polymarket:
                    return self._create_result(
                        request,
                        ExecutionStatus.FAILED,
                        rejection_reason="Polymarket client not initialized",
                    )

                # Polymarket execution (placeholder - needs CLOB integration)
                return self._create_result(
                    request,
                    ExecutionStatus.FAILED,
                    rejection_reason="Real Polymarket execution not yet implemented",
                )

            else:
                return self._create_result(
                    request,
                    ExecutionStatus.FAILED,
                    rejection_reason=f"Unsupported platform: {request.platform.value}",
                )

        except Exception as e:
            logger.error(f"Real execution error: {e}", exc_info=True)
            return self._create_result(
                request,
                ExecutionStatus.FAILED,
                rejection_reason=str(e),
            )

    def _create_result(
        self,
        request: ExecutionRequest,
        status: ExecutionStatus,
        rejection_reason: Optional[str] = None,
        order_id: Optional[str] = None,
        filled_qty: float = 0.0,
        avg_price: float = 0.0,
        fees: float = 0.0,
    ) -> ExecutionResult:
        """Create an ExecutionResult from a request."""
        return ExecutionResult(
            request_id=request.request_id,
            idempotency_key=request.idempotency_key,
            status=status,
            rejection_reason=rejection_reason,
            order_id=order_id,
            filled_qty=filled_qty,
            avg_price=avg_price,
            fees=fees,
            platform=request.platform,
            market_id=request.market_id,
            contract_team=request.contract_team,
            game_id=request.game_id,
            sport=request.sport,
            signal_id=request.signal_id,
            signal_type=request.signal_type,
            edge_pct=request.edge_pct,
            side=request.side,
            requested_at=request.created_at,
            executed_at=datetime.now(timezone.utc),
        )

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                bankroll = self.paper_engine._bankroll if self.paper_engine else None
                status = {
                    "type": "execution_service",
                    "paper_trading": self.paper_trading,
                    "executions": self._execution_count,
                    "successes": self._success_count,
                    "failures": self._failure_count,
                    "in_flight": len(self._in_flight),
                    "bankroll": bankroll.current_balance if bankroll else 0,
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                }
                logger.info(
                    f"ExecutionService: {self._success_count}/{self._execution_count} successful, "
                    f"bankroll=${status['bankroll']:.2f}"
                )

                # Update health monitoring heartbeat
                if self._heartbeat_publisher:
                    success_rate = (
                        self._success_count / self._execution_count * 100
                        if self._execution_count > 0 else 100.0
                    )
                    self._heartbeat_publisher.update_metrics({
                        "executions_total": float(self._execution_count),
                        "executions_success": float(self._success_count),
                        "executions_failed": float(self._failure_count),
                        "success_rate_pct": success_rate,
                        "bankroll": float(status["bankroll"]),
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

    service = ExecutionService(
        paper_trading=os.environ.get("PAPER_TRADING", "1") == "1",
        initial_bankroll=float(os.environ.get("INITIAL_BANKROLL", "1000")),
        min_edge_pct=float(os.environ.get("MIN_EDGE_PCT", "2.0")),
        kelly_fraction=float(os.environ.get("KELLY_FRACTION", "0.25")),
        max_position_pct=float(os.environ.get("MAX_POSITION_PCT", "10.0")),
        slippage_bps=float(os.environ.get("SLIPPAGE_BPS", "10.0")),
    )

    await service.start()

    try:
        while True:
            await asyncio.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        await service.stop()


if __name__ == "__main__":
    asyncio.run(main())
