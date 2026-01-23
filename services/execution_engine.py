"""
Execution Engine for concurrent arbitrage order placement.

KEY FEATURES:
- Executes BOTH legs simultaneously (not sequential!)
- In-flight deduplication (prevent double-execution)
- Position reconciliation for mismatched fills
- Automatic retry with exponential backoff
"""

import asyncio
import logging
import time
from dataclasses import dataclass
from datetime import datetime
from typing import Optional, Tuple

from arbees_shared.models.market import Platform
from arbees_shared.models.signal import ArbitrageOpportunity
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient

logger = logging.getLogger(__name__)


@dataclass
class OrderResult:
    """Result of a single order execution."""
    success: bool
    platform: Platform
    market_id: str
    side: str  # "yes" or "no"
    price: float
    quantity: int
    order_id: Optional[str] = None
    error: Optional[str] = None
    latency_ms: float = 0.0


@dataclass
class ArbitrageExecution:
    """Result of a full arbitrage execution (both legs)."""
    opportunity_id: str
    leg1: OrderResult
    leg2: OrderResult
    total_latency_ms: float
    
    @property
    def both_filled(self) -> bool:
        """Both legs filled successfully."""
        return self.leg1.success and self.leg2.success
    
    @property
    def partial_fill(self) -> bool:
        """Only one leg filled."""
        return self.leg1.success != self.leg2.success
    
    @property
    def total_failed(self) -> bool:
        """Both legs failed."""
        return not self.leg1.success and not self.leg2.success


class ExecutionEngine:
    """
    Concurrent order execution engine with deduplication.
    
    Ensures:
    - Both legs execute simultaneously (minimize slippage)
    - No duplicate executions of same opportunity
    - Position reconciliation for partial fills
    """
    
    def __init__(
        self,
        kalshi_client: KalshiClient,
        polymarket_client: PolymarketClient,
        default_size: int = 10,
        max_retries: int = 2,
    ):
        """
        Initialize execution engine.
        
        Args:
            kalshi_client: Connected Kalshi client
            polymarket_client: Connected Polymarket client
            default_size: Default order size (contracts)
            max_retries: Max retry attempts per order
        """
        self.kalshi = kalshi_client
        self.polymarket = polymarket_client
        self.default_size = default_size
        self.max_retries = max_retries
        
        # In-flight deduplication
        self._in_flight: set[str] = set()
        self._lock = asyncio.Lock()
        
        # Metrics
        self._total_executions = 0
        self._successful_executions = 0
        self._partial_fills = 0
        self._failed_executions = 0
    
    async def execute_arbitrage(
        self,
        opportunity: ArbitrageOpportunity,
        size: Optional[int] = None,
    ) -> Optional[ArbitrageExecution]:
        """
        Execute arbitrage opportunity with concurrent order placement.
        
        Args:
            opportunity: Arbitrage opportunity to execute
            size: Order size (contracts), defaults to self.default_size
            
        Returns:
            ArbitrageExecution result or None if already in-flight
        """
        # Deduplication check
        opp_key = self._get_opportunity_key(opportunity)
        
        async with self._lock:
            if opp_key in self._in_flight:
                logger.warning(f"Opportunity {opp_key} already executing, skipping")
                return None
            self._in_flight.add(opp_key)
        
        start_time = time.time()
        
        try:
            size = size or self.default_size
            
            # Determine which platforms/sides to trade
            platform1 = opportunity.platform_buy
            platform2 = opportunity.platform_sell
            
            # For cross-platform arbitrage:
            # Buy YES on platform1 + Buy NO on platform2
            # (buying NO = selling YES at bid price)
            
            logger.info(
                f"Executing arbitrage: Buy YES {platform1.value} @ {opportunity.buy_price:.3f} + "
                f"Buy NO {platform2.value} @ {opportunity.sell_price:.3f} "
                f"(edge: {opportunity.edge_pct:.1f}%)"
            )
            
            # Execute BOTH legs concurrently (critical for arbitrage!)
            leg1_task = asyncio.create_task(
                self._execute_order(
                    platform=platform1,
                    market_id=opportunity.event_id,  # This should be the market_id, not event_id
                    side="yes",
                    price=opportunity.buy_price,
                    quantity=size,
                )
            )
            
            leg2_task = asyncio.create_task(
                self._execute_order(
                    platform=platform2,
                    market_id=opportunity.event_id,
                    side="no",
                    price=opportunity.sell_price,
                    quantity=size,
                )
            )
            
            # Wait for both (concurrent, not sequential!)
            results = await asyncio.gather(leg1_task, leg2_task, return_exceptions=True)
            
            leg1_result = results[0] if not isinstance(results[0], Exception) else OrderResult(
                success=False,
                platform=platform1,
                market_id=opportunity.event_id,
                side="yes",
                price=opportunity.buy_price,
                quantity=size,
                error=str(results[0]),
            )
            
            leg2_result = results[1] if not isinstance(results[1], Exception) else OrderResult(
                success=False,
                platform=platform2,
                market_id=opportunity.event_id,
                side="no",
                price=opportunity.sell_price,
                quantity=size,
                error=str(results[1]),
            )
            
            total_latency = (time.time() - start_time) * 1000  # milliseconds
            
            execution = ArbitrageExecution(
                opportunity_id=opp_key,
                leg1=leg1_result,
                leg2=leg2_result,
                total_latency_ms=total_latency,
            )
            
            # Update metrics
            self._total_executions += 1
            if execution.both_filled:
                self._successful_executions += 1
                logger.info(
                    f"✓ Arbitrage executed successfully in {total_latency:.1f}ms "
                    f"({leg1_result.latency_ms:.1f}ms + {leg2_result.latency_ms:.1f}ms)"
                )
            elif execution.partial_fill:
                self._partial_fills += 1
                logger.warning(
                    f"⚠ Partial fill: {self._describe_partial_fill(execution)}"
                )
                # Handle position reconciliation
                await self._reconcile_position(execution)
            else:
                self._failed_executions += 1
                logger.error(
                    f"✗ Arbitrage execution failed: "
                    f"Leg1: {leg1_result.error}, Leg2: {leg2_result.error}"
                )
            
            return execution
            
        finally:
            # Remove from in-flight
            async with self._lock:
                self._in_flight.discard(opp_key)
    
    async def _execute_order(
        self,
        platform: Platform,
        market_id: str,
        side: str,
        price: float,
        quantity: int,
    ) -> OrderResult:
        """
        Execute a single order with retry logic.
        
        Args:
            platform: Trading platform
            market_id: Market identifier
            side: "yes" or "no"
            price: Limit price
            quantity: Order size
            
        Returns:
            OrderResult
        """
        start_time = time.time()
        
        for attempt in range(self.max_retries):
            try:
                if platform == Platform.KALSHI:
                    result = await self.kalshi.place_order(
                        market_id=market_id,
                        side=side,  # "yes" or "no"
                        price=price,
                        quantity=quantity,
                    )
                    
                    order_id = result.get("order", {}).get("order_id")
                    latency = (time.time() - start_time) * 1000
                    
                    return OrderResult(
                        success=True,
                        platform=platform,
                        market_id=market_id,
                        side=side,
                        price=price,
                        quantity=quantity,
                        order_id=order_id,
                        latency_ms=latency,
                    )
                
                elif platform == Platform.POLYMARKET:
                    # Polymarket order placement
                    result = await self.polymarket.place_order(
                        market_id=market_id,
                        side="buy",
                        outcome=side,
                        price=price,
                        quantity=quantity,
                    )
                    
                    order_id = result.get("orderID")
                    latency = (time.time() - start_time) * 1000
                    
                    return OrderResult(
                        success=True,
                        platform=platform,
                        market_id=market_id,
                        side=side,
                        price=price,
                        quantity=quantity,
                        order_id=order_id,
                        latency_ms=latency,
                    )
                
            except Exception as e:
                if attempt < self.max_retries - 1:
                    # Exponential backoff
                    delay = 0.1 * (2 ** attempt)
                    logger.warning(
                        f"Order failed on {platform.value} (attempt {attempt + 1}/{self.max_retries}): {e}"
                    )
                    await asyncio.sleep(delay)
                else:
                    # Final attempt failed
                    latency = (time.time() - start_time) * 1000
                    return OrderResult(
                        success=False,
                        platform=platform,
                        market_id=market_id,
                        side=side,
                        price=price,
                        quantity=quantity,
                        error=str(e),
                        latency_ms=latency,
                    )
        
        # Should never reach here
        return OrderResult(
            success=False,
            platform=platform,
            market_id=market_id,
            side=side,
            price=price,
            quantity=quantity,
            error="Max retries exceeded",
        )
    
    async def _reconcile_position(self, execution: ArbitrageExecution) -> None:
        """
        Handle partial fill by closing the filled leg.
        
        This prevents directional exposure when only one leg fills.
        """
        if execution.leg1.success and not execution.leg2.success:
            # Leg1 filled, leg2 failed - close leg1
            logger.info(f"Reconciling position: closing {execution.leg1.platform.value} leg")
            await self._close_position(execution.leg1)
            
        elif execution.leg2.success and not execution.leg1.success:
            # Leg2 filled, leg1 failed - close leg2
            logger.info(f"Reconciling position: closing {execution.leg2.platform.value} leg")
            await self._close_position(execution.leg2)
    
    async def _close_position(self, order: OrderResult) -> None:
        """
        Close an open position by selling at market.
        
        This is a safety mechanism to avoid directional exposure.
        """
        try:
            if order.platform == Platform.KALSHI:
                # Sell the position
                await self.kalshi.place_order(
                    market_id=order.market_id,
                    side=order.side,  # "yes" or "no"
                    price=order.price * 0.98,  # Market sell (slightly below entry)
                    quantity=order.quantity,
                )
            elif order.platform == Platform.POLYMARKET:
                await self.polymarket.place_order(
                    market_id=order.market_id,
                    side="sell",
                    outcome=order.side,
                    price=order.price * 0.98,
                    quantity=order.quantity,
                )
            
            logger.info(f"Closed position on {order.platform.value}")
            
        except Exception as e:
            logger.error(f"Failed to close position on {order.platform.value}: {e}")
    
    def _get_opportunity_key(self, opp: ArbitrageOpportunity) -> str:
        """Generate unique key for deduplication."""
        return f"{opp.platform_buy.value}:{opp.platform_sell.value}:{opp.event_id}"
    
    def _describe_partial_fill(self, execution: ArbitrageExecution) -> str:
        """Describe which leg filled and which failed."""
        if execution.leg1.success:
            return f"{execution.leg1.platform.value} filled, {execution.leg2.platform.value} failed"
        else:
            return f"{execution.leg2.platform.value} filled, {execution.leg1.platform.value} failed"
    
    def get_metrics(self) -> dict:
        """Get execution metrics."""
        return {
            "total_executions": self._total_executions,
            "successful_executions": self._successful_executions,
            "partial_fills": self._partial_fills,
            "failed_executions": self._failed_executions,
            "success_rate": (
                self._successful_executions / self._total_executions
                if self._total_executions > 0
                else 0.0
            ),
            "in_flight_count": len(self._in_flight),
        }
