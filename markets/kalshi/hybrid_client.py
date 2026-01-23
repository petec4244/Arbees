"""
Hybrid Kalshi client combining REST and WebSocket capabilities.

Uses:
- REST API: Market queries, order placement, position management
- WebSocket: Real-time price streaming (10-50ms latency)

This provides the best of both worlds:
- Low-latency price updates via WebSocket
- Full API functionality via REST
"""

import asyncio
import logging
from typing import Any, AsyncIterator, Optional

from arbees_shared.models.market import MarketPrice, OrderBook, Platform
from markets.kalshi.client import KalshiClient
from markets.kalshi.websocket.ws_client import KalshiWebSocketClient

logger = logging.getLogger(__name__)


class HybridKalshiClient:
    """Hybrid Kalshi client with REST + WebSocket capabilities.

    Example usage:
        async with HybridKalshiClient() as client:
            # REST operations
            markets = await client.get_markets(sport="nfl")

            # WebSocket streaming
            async for price in client.stream_prices(["MARKET_1", "MARKET_2"]):
                print(f"{price.market_id}: {price.mid_price}")
    """

    def __init__(
        self,
        api_key: Optional[str] = None,
        private_key_path: Optional[str] = None,
        private_key_str: Optional[str] = None,
        rate_limit: float = 2.0,  # Kalshi rate limits to ~2 req/sec
        prefer_websocket: bool = True,
    ):
        """
        Initialize hybrid client.

        Args:
            api_key: Kalshi API key ID (or KALSHI_API_KEY env var)
            private_key_path: Path to RSA private key PEM file
            private_key_str: RSA private key as string (or KALSHI_PRIVATE_KEY env var)
            rate_limit: Max REST requests per second (default 2.0 to avoid rate limits)
            prefer_websocket: If True, use WebSocket for prices when subscribed
        """
        self._rest = KalshiClient(
            api_key=api_key,
            private_key_path=private_key_path,
            private_key_str=private_key_str,
            rate_limit=rate_limit,
        )
        self._ws = KalshiWebSocketClient(
            api_key=api_key,
            private_key_path=private_key_path,
            private_key_str=private_key_str,
        )
        self._prefer_websocket = prefer_websocket
        self._ws_stream_task: Optional[asyncio.Task] = None
        
        # Volume tracking (since WS doesn't provide it)
        self._volume_cache: dict[str, float] = {}
        self._volume_poll_task: Optional[asyncio.Task] = None
        self._volume_poll_interval: float = 60.0  # Poll volume every minute
        self._running = False

    async def __aenter__(self) -> "HybridKalshiClient":
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await self.disconnect()

    async def connect(self) -> None:
        """Connect both REST and WebSocket clients."""
        await self._rest.connect()
        # WebSocket connects on first subscribe
        
        self._running = True
        self._volume_poll_task = asyncio.create_task(self._poll_volume_loop())

    async def disconnect(self) -> None:
        """Disconnect both clients."""
        self._running = False
        
        if self._ws_stream_task:
            self._ws_stream_task.cancel()
            try:
                await self._ws_stream_task
            except asyncio.CancelledError:
                pass
            self._ws_stream_task = None
            
        if self._volume_poll_task:
            self._volume_poll_task.cancel()
            try:
                await self._volume_poll_task
            except asyncio.CancelledError:
                pass
            self._volume_poll_task = None

        await self._ws.disconnect()
        await self._rest.disconnect()

    @property
    def platform(self) -> Platform:
        return Platform.KALSHI

    @property
    def ws_connected(self) -> bool:
        """Check if WebSocket is connected."""
        return self._ws.connected

    @property
    def subscribed_markets(self) -> set[str]:
        """Get set of markets subscribed via WebSocket."""
        return self._ws.subscribed_markets

    # ==========================================================================
    # REST API Delegation
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """Get markets from Kalshi (REST)."""
        return await self._rest.get_markets(sport=sport, status=status, limit=limit)

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get detailed information about a specific market (REST)."""
        return await self._rest.get_market(market_id)

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get order book for a market.

        If subscribed via WebSocket, returns local orderbook state.
        Otherwise falls back to REST.
        """
        if self._prefer_websocket and market_id in self._ws.subscribed_markets:
            book = self._ws.get_orderbook(market_id)
            if book:
                # Convert LocalOrderBook to OrderBook
                from arbees_shared.models.market import OrderBookLevel

                yes_bids = [
                    OrderBookLevel(price=price / 100.0, quantity=qty)
                    for price, qty in sorted(
                        book.yes_bids.items(), key=lambda x: x[0], reverse=True
                    )
                ]
                yes_asks = [
                    OrderBookLevel(price=price / 100.0, quantity=qty)
                    for price, qty in sorted(book.yes_asks.items(), key=lambda x: x[0])
                ]
                return OrderBook(
                    market_id=market_id,
                    platform=Platform.KALSHI,
                    yes_bids=yes_bids,
                    yes_asks=yes_asks,
                    timestamp=book.last_update,
                )
        return await self._rest.get_orderbook(market_id)

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price.

        If subscribed via WebSocket, returns local orderbook state.
        Otherwise falls back to REST.
        """
        if self._prefer_websocket and market_id in self._ws.subscribed_markets:
            price = self._ws.get_market_price(market_id)
            if price:
                return price
        return await self._rest.get_market_price(market_id)

    async def search_markets(self, query: str, limit: int = 50) -> list[dict]:
        """Search for markets by keyword (REST)."""
        return await self._rest.search_markets(query, limit)

    async def get_events(self, series_ticker: Optional[str] = None) -> list[dict]:
        """Get events (REST)."""
        return await self._rest.get_events(series_ticker)

    async def health_check(self) -> bool:
        """Check if API is accessible (REST)."""
        return await self._rest.health_check()

    # ==========================================================================
    # Trading Methods (REST)
    # ==========================================================================

    async def place_order(
        self,
        market_id: str,
        side: str,
        price: float,
        quantity: float,
    ) -> dict:
        """Place an order on Kalshi (REST)."""
        return await self._rest.place_order(market_id, side, price, quantity)

    async def cancel_order(self, order_id: str) -> bool:
        """Cancel an order (REST)."""
        return await self._rest.cancel_order(order_id)

    async def get_positions(self) -> list[dict]:
        """Get current positions (REST)."""
        return await self._rest.get_positions()

    # ==========================================================================
    # WebSocket Streaming
    # ==========================================================================

    async def subscribe(self, market_ids: list[str]) -> None:
        """Subscribe to market updates via WebSocket."""
        await self._ws.subscribe(market_ids)

    async def unsubscribe(self, market_ids: list[str]) -> None:
        """Unsubscribe from market updates."""
        await self._ws.unsubscribe(market_ids)

    async def stream_prices(
        self,
        market_ids: list[str],
    ) -> AsyncIterator[MarketPrice]:
        """Stream price updates via WebSocket.

        Args:
            market_ids: Markets to subscribe to

        Yields:
            MarketPrice objects on each update
        """
        async for price in self._ws.stream_prices(market_ids):
            # Inject cached volume
            if price.market_id in self._volume_cache:
                # Create a new copy with volume set (since pydantic models are immutable/frozen)
                # Using model_copy with update is cleaner
                price = price.model_copy(update={"volume": self._volume_cache[price.market_id]})
            yield price

    async def subscribe_with_metadata(
        self,
        markets: list[dict],
    ) -> None:
        """Subscribe to markets with metadata.

        Args:
            markets: List of dicts with 'market_id', 'title', 'game_id' keys
        """
        await self._ws.subscribe_with_metadata(markets)

    # ==========================================================================
    # Convenience Methods
    # ==========================================================================

    async def get_sports_markets_and_subscribe(
        self,
        sport: str,
        limit: int = 50,
    ) -> list[dict]:
        """Get sports markets and auto-subscribe to WebSocket.

        Args:
            sport: Sport to filter (nfl, nba, etc.)
            limit: Max markets to fetch

        Returns:
            List of market dictionaries
        """
        markets = await self.get_markets(sport=sport, limit=limit)

        # Subscribe to WebSocket with metadata
        if markets:
            ws_markets = [
                {
                    "market_id": m.get("ticker"),
                    "title": m.get("title", ""),
                    "game_id": m.get("event_ticker"),
                }
                for m in markets
                if m.get("ticker")
            ]
            await self.subscribe_with_metadata(ws_markets)

        return markets

    async def get_multi_market_prices(
        self,
        market_ids: list[str],
    ) -> dict[str, Optional[MarketPrice]]:
        """Get prices for multiple markets efficiently.

        Uses WebSocket cache for subscribed markets, REST for others.

        Args:
            market_ids: List of market IDs

        Returns:
            Dictionary mapping market_id -> MarketPrice (or None)
        """
        results: dict[str, Optional[MarketPrice]] = {}

        # Separate subscribed vs unsubscribed markets
        ws_markets = []
        rest_markets = []

        for market_id in market_ids:
            if market_id in self._ws.subscribed_markets:
                ws_markets.append(market_id)
            else:
                rest_markets.append(market_id)

        # Get WebSocket prices (instant)
        for market_id in ws_markets:
            results[market_id] = self._ws.get_market_price(market_id)

        # Get REST prices (parallel)
        if rest_markets:
            tasks = [self._rest.get_market_price(mid) for mid in rest_markets]
            rest_prices = await asyncio.gather(*tasks, return_exceptions=True)

            for market_id, price in zip(rest_markets, rest_prices):
                if isinstance(price, Exception):
                    logger.warning(f"Error fetching {market_id}: {price}")
                    results[market_id] = None
                else:
                    results[market_id] = price

        return results
        
    async def _poll_volume_loop(self) -> None:
        """Periodic task to poll volume data (missing from WS)."""
        logger.info(f"Starting Kalshi volume poller (interval={self._volume_poll_interval}s)")
        
        while self._running:
            try:
                # Get list of subscribed markets
                subscribed = list(self._ws.subscribed_markets)
                if not subscribed:
                    await asyncio.sleep(5.0)
                    continue
                
                # Poll each market via REST
                # TODO: Optimization - use batch endpoint or get_markets with filter if possible
                # For now, simplistic iteration with rate limiting handled by REST client
                for market_id in subscribed:
                    if not self._running:
                        break
                        
                    try:
                        market = await self._rest.get_market(market_id)
                        if market:
                            vol = float(market.get("volume", 0) or 0)
                            self._volume_cache[market_id] = vol
                    except Exception as e:
                        logger.debug(f"Volume poll failed for {market_id}: {e}")
                    
                    # Be nice to the rate limiter
                    await asyncio.sleep(0.5)
                
            except Exception as e:
                logger.error(f"Error in volume poll loop: {e}")
            
            # Wait for next cycle
            await asyncio.sleep(self._volume_poll_interval)
