"""Base WebSocket client for real-time market data streaming.

Provides:
- LocalOrderBook: Maintains orderbook state from deltas
- BaseWebSocketClient: Abstract base class for platform-specific WebSocket clients

Key improvements over REST polling:
- Latency: 500-3000ms -> 10-50ms
- API calls: 400+/min -> ~10/min (subscribe only)
- Rate limit risk: High -> Low
"""

import asyncio
import logging
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, AsyncIterator, Optional

import aiohttp

from arbees_shared.models.market import MarketPrice, MarketStatus, Platform

logger = logging.getLogger(__name__)


@dataclass
class LocalOrderBook:
    """Maintains orderbook state from deltas.

    Prediction markets use the convention:
    - YES bid at X cents = willing to buy YES at X%
    - NO bid at Y cents = willing to buy NO at Y% (equivalent to YES ask at (100-Y)%)

    This class stores prices in cents (0-100) internally for efficiency.
    """

    market_id: str
    platform: Platform
    yes_bids: dict[int, float] = field(default_factory=dict)  # price_cents -> quantity
    yes_asks: dict[int, float] = field(default_factory=dict)  # price_cents -> quantity
    last_update: datetime = field(default_factory=datetime.utcnow)

    def apply_delta(self, price_cents: int, delta: float, side: str) -> None:
        """Apply a delta to the orderbook.

        Args:
            price_cents: Price level in cents (0-100)
            delta: Change in quantity (positive to add, negative to remove)
            side: Either "yes_bid", "yes_ask", "no_bid", or "no_ask"
        """
        # Map NO side to YES side (NO bid at X = YES ask at 100-X)
        if side == "no_bid":
            price_cents = 100 - price_cents
            side = "yes_ask"
        elif side == "no_ask":
            price_cents = 100 - price_cents
            side = "yes_bid"

        book = self.yes_bids if side == "yes_bid" else self.yes_asks

        if price_cents in book:
            new_qty = book[price_cents] + delta
            if new_qty <= 0:
                del book[price_cents]
            else:
                book[price_cents] = new_qty
        elif delta > 0:
            book[price_cents] = delta

        self.last_update = datetime.utcnow()

    def apply_snapshot(
        self,
        yes_bids: list[tuple[int, float]],
        no_bids: list[tuple[int, float]],
    ) -> None:
        """Apply a full snapshot to replace the orderbook.

        Args:
            yes_bids: List of (price_cents, quantity) for YES bids
            no_bids: List of (price_cents, quantity) for NO bids
                     NO bid at X = YES ask at (100-X)
        """
        self.yes_bids.clear()
        self.yes_asks.clear()

        for price_cents, qty in yes_bids:
            if qty > 0:
                self.yes_bids[price_cents] = qty

        # Convert NO bids to YES asks
        for price_cents, qty in no_bids:
            if qty > 0:
                yes_ask_price = 100 - price_cents
                self.yes_asks[yes_ask_price] = qty

        self.last_update = datetime.utcnow()

    @property
    def best_yes_bid(self) -> Optional[float]:
        """Best (highest) YES bid price as decimal (0.0-1.0)."""
        if not self.yes_bids:
            return None
        return max(self.yes_bids.keys()) / 100.0

    @property
    def best_yes_ask(self) -> Optional[float]:
        """Best (lowest) YES ask price as decimal (0.0-1.0)."""
        if not self.yes_asks:
            return None
        return min(self.yes_asks.keys()) / 100.0

    @property
    def mid_price(self) -> Optional[float]:
        """Mid price between best bid and ask."""
        bid = self.best_yes_bid
        ask = self.best_yes_ask
        if bid is None or ask is None:
            return None
        return (bid + ask) / 2

    @property
    def spread_cents(self) -> Optional[int]:
        """Spread in cents (ask - bid)."""
        bid = self.best_yes_bid
        ask = self.best_yes_ask
        if bid is None or ask is None:
            return None
        return int((ask - bid) * 100)

    @property
    def total_bid_liquidity(self) -> float:
        """Total quantity on bid side."""
        return sum(self.yes_bids.values())

    @property
    def total_ask_liquidity(self) -> float:
        """Total quantity on ask side."""
        return sum(self.yes_asks.values())

    def to_market_price(
        self,
        market_title: str = "",
        game_id: Optional[str] = None,
        volume: float = 0.0,
        last_trade_price: Optional[float] = None,
    ) -> MarketPrice:
        """Convert orderbook state to MarketPrice snapshot."""
        yes_bid = self.best_yes_bid or 0.0
        yes_ask = self.best_yes_ask or 1.0

        # Ensure valid bid/ask relationship
        if yes_bid >= yes_ask:
            # Crossed book - use mid price with small spread
            mid = (yes_bid + yes_ask) / 2
            yes_bid = max(0.0, mid - 0.01)
            yes_ask = min(1.0, mid + 0.01)

        return MarketPrice(
            market_id=self.market_id,
            platform=self.platform,
            game_id=game_id,
            market_title=market_title,
            yes_bid=yes_bid,
            yes_ask=yes_ask,
            volume=volume,
            liquidity=self.total_bid_liquidity,
            status=MarketStatus.OPEN,
            timestamp=self.last_update,
            last_trade_price=last_trade_price,
        )


class BaseWebSocketClient(ABC):
    """Abstract base class for WebSocket market data clients.

    Features:
    - Automatic reconnection with exponential backoff (1s-60s)
    - Heartbeat/ping support
    - Subscription management
    - Local orderbook maintenance via deltas
    """

    def __init__(
        self,
        ws_url: str,
        platform: Platform,
        heartbeat_interval: float = 30.0,
        reconnect_min_delay: float = 1.0,
        reconnect_max_delay: float = 60.0,
    ):
        self.ws_url = ws_url
        self.platform = platform
        self.heartbeat_interval = heartbeat_interval
        self.reconnect_min_delay = reconnect_min_delay
        self.reconnect_max_delay = reconnect_max_delay

        self._session: Optional[aiohttp.ClientSession] = None
        self._ws: Optional[aiohttp.ClientWebSocketResponse] = None
        self._connected = False
        self._subscribed_markets: set[str] = set()
        self._orderbooks: dict[str, LocalOrderBook] = {}
        self._market_metadata: dict[str, dict] = {}  # market_id -> {title, game_id, volume, ...}

        self._heartbeat_task: Optional[asyncio.Task] = None
        self._reconnect_delay = reconnect_min_delay
        self._last_message_time: float = 0
        self._message_count: int = 0
        self._reconnect_count: int = 0

    async def connect(self) -> None:
        """Establish WebSocket connection."""
        if self._connected:
            return

        if self._session is None:
            self._session = aiohttp.ClientSession()

        try:
            auth_headers = await self._authenticate()
            self._ws = await self._session.ws_connect(
                self.ws_url,
                headers=auth_headers,
                heartbeat=self.heartbeat_interval,
            )
            self._connected = True
            self._reconnect_delay = self.reconnect_min_delay
            logger.info(f"{self.platform.value} WebSocket connected to {self.ws_url}")

            # Start heartbeat task if needed
            if self._heartbeat_task is None:
                self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        except Exception as e:
            logger.error(f"{self.platform.value} WebSocket connection failed: {e}")
            raise

    async def disconnect(self) -> None:
        """Close WebSocket connection."""
        self._connected = False

        if self._heartbeat_task:
            self._heartbeat_task.cancel()
            try:
                await self._heartbeat_task
            except asyncio.CancelledError:
                pass
            self._heartbeat_task = None

        if self._ws:
            await self._ws.close()
            self._ws = None

        if self._session:
            await self._session.close()
            self._session = None

        logger.info(f"{self.platform.value} WebSocket disconnected")

    async def _reconnect(self) -> None:
        """Reconnect with exponential backoff."""
        self._connected = False
        self._reconnect_count += 1

        logger.warning(
            f"{self.platform.value} WebSocket reconnecting in {self._reconnect_delay:.1f}s "
            f"(attempt {self._reconnect_count})"
        )

        await asyncio.sleep(self._reconnect_delay)

        # Exponential backoff
        self._reconnect_delay = min(
            self._reconnect_delay * 2,
            self.reconnect_max_delay
        )

        try:
            # Close existing connection
            if self._ws:
                await self._ws.close()
                self._ws = None

            # Reconnect
            await self.connect()

            # Resubscribe to markets
            if self._subscribed_markets:
                markets = list(self._subscribed_markets)
                self._subscribed_markets.clear()
                await self.subscribe(markets)

        except Exception as e:
            logger.error(f"{self.platform.value} reconnection failed: {e}")
            # Schedule another reconnect
            asyncio.create_task(self._reconnect())

    async def _heartbeat_loop(self) -> None:
        """Send periodic heartbeats if required by the platform."""
        # Override in subclasses that need custom heartbeat behavior
        pass

    async def subscribe(self, market_ids: list[str]) -> None:
        """Subscribe to market updates.

        Args:
            market_ids: List of market identifiers to subscribe to
        """
        if not market_ids:
            return

        if not self._connected or not self._ws:
            await self.connect()

        # Build and send subscribe message
        msg = await self._build_subscribe_message(market_ids)
        await self._ws.send_json(msg)

        # Track subscriptions
        self._subscribed_markets.update(market_ids)

        # Initialize orderbooks
        for market_id in market_ids:
            if market_id not in self._orderbooks:
                self._orderbooks[market_id] = LocalOrderBook(
                    market_id=market_id,
                    platform=self.platform,
                )

        logger.info(f"{self.platform.value} subscribed to {len(market_ids)} markets")

    async def unsubscribe(self, market_ids: list[str]) -> None:
        """Unsubscribe from market updates."""
        if not market_ids:
            return

        if self._connected and self._ws:
            msg = await self._build_unsubscribe_message(market_ids)
            if msg:
                await self._ws.send_json(msg)

        self._subscribed_markets.difference_update(market_ids)

        for market_id in market_ids:
            self._orderbooks.pop(market_id, None)
            self._market_metadata.pop(market_id, None)

        logger.info(f"{self.platform.value} unsubscribed from {len(market_ids)} markets")

    async def stream_prices(
        self,
        market_ids: list[str],
    ) -> AsyncIterator[MarketPrice]:
        """Stream price updates for markets via WebSocket.

        Args:
            market_ids: Markets to subscribe to

        Yields:
            MarketPrice objects on each update
        """
        await self.subscribe(market_ids)

        try:
            async for msg in self._ws:
                if msg.type == aiohttp.WSMsgType.TEXT:
                    self._last_message_time = time.monotonic()
                    self._message_count += 1

                    try:
                        data = msg.json()
                        price = await self._handle_message(data)
                        if price:
                            yield price
                    except Exception as e:
                        logger.warning(f"Error handling message: {e}")

                elif msg.type == aiohttp.WSMsgType.ERROR:
                    logger.error(f"WebSocket error: {self._ws.exception()}")
                    await self._reconnect()

                elif msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSED):
                    logger.warning("WebSocket closed, reconnecting...")
                    await self._reconnect()

        except asyncio.CancelledError:
            logger.info("Price stream cancelled")
            raise
        except Exception as e:
            logger.error(f"Price stream error: {e}")
            await self._reconnect()

    def get_orderbook(self, market_id: str) -> Optional[LocalOrderBook]:
        """Get current orderbook state for a market."""
        return self._orderbooks.get(market_id)

    def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current price snapshot from local orderbook."""
        book = self._orderbooks.get(market_id)
        if not book:
            return None

        meta = self._market_metadata.get(market_id, {})
        return book.to_market_price(
            market_title=meta.get("title", ""),
            game_id=meta.get("game_id"),
            volume=meta.get("volume", 0.0),
            last_trade_price=meta.get("last_trade_price"),
        )

    @property
    def connected(self) -> bool:
        """Check if WebSocket is connected."""
        return self._connected and self._ws is not None and not self._ws.closed

    @property
    def subscribed_markets(self) -> set[str]:
        """Get set of subscribed market IDs."""
        return self._subscribed_markets.copy()

    @property
    def message_count(self) -> int:
        """Total messages received."""
        return self._message_count

    @property
    def reconnect_count(self) -> int:
        """Number of reconnection attempts."""
        return self._reconnect_count

    # ==========================================================================
    # Abstract methods - must be implemented by subclasses
    # ==========================================================================

    @abstractmethod
    async def _authenticate(self) -> dict[str, str]:
        """Generate authentication headers for WebSocket upgrade.

        Returns:
            Dictionary of headers to include in the connection request
        """
        ...

    @abstractmethod
    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        """Build the subscribe message for the platform.

        Args:
            market_ids: Markets to subscribe to

        Returns:
            JSON-serializable message to send
        """
        ...

    async def _build_unsubscribe_message(self, market_ids: list[str]) -> Optional[dict]:
        """Build the unsubscribe message for the platform.

        Override in subclasses if unsubscribe is supported.

        Returns:
            JSON-serializable message to send, or None if not supported
        """
        return None

    @abstractmethod
    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        """Handle an incoming WebSocket message.

        Args:
            msg: Parsed JSON message from WebSocket

        Returns:
            MarketPrice if the message resulted in a price update, None otherwise
        """
        ...
