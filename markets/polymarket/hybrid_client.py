"""
Hybrid Polymarket client combining REST and WebSocket capabilities.

Uses:
- REST API (Gamma + CLOB): Market queries, orderbook snapshots
- WebSocket: Real-time price streaming (10-50ms latency)

This provides the best of both worlds:
- Low-latency price updates via WebSocket
- Full API functionality via REST

Note: Polymarket requires token_id resolution for WebSocket subscriptions.
"""

import asyncio
import logging
from typing import Any, AsyncIterator, Optional

from arbees_shared.models.market import MarketPrice, OrderBook, Platform
from markets.polymarket.client import PolymarketClient
from markets.polymarket.ws_client import PolymarketWebSocketClient

logger = logging.getLogger(__name__)


class HybridPolymarketClient:
    """Hybrid Polymarket client with REST + WebSocket capabilities.

    Example usage:
        async with HybridPolymarketClient() as client:
            # REST operations
            markets = await client.get_markets(sport="nfl")

            # Resolve token IDs for WebSocket
            for market in markets:
                token_id = await client.resolve_yes_token_id(market)

            # WebSocket streaming (requires token_ids)
            async for price in client.stream_prices(token_ids):
                print(f"{price.market_id}: {price.mid_price}")
    """

    def __init__(
        self,
        api_key: Optional[str] = None,
        proxy_url: Optional[str] = None,
        use_eu_proxy: bool = False,
        rate_limit: float = 10.0,
        prefer_websocket: bool = True,
    ):
        """
        Initialize hybrid client.

        Args:
            api_key: Optional API key for authenticated endpoints
            proxy_url: Optional proxy URL for routing
            use_eu_proxy: If True, use EU proxy service (for regulatory compliance)
            rate_limit: Max REST requests per second
            prefer_websocket: If True, use WebSocket for prices when subscribed
        """
        self._rest = PolymarketClient(
            api_key=api_key,
            proxy_url=proxy_url,
            use_eu_proxy=use_eu_proxy,
            rate_limit=rate_limit,
        )
        self._ws = PolymarketWebSocketClient()
        self._prefer_websocket = prefer_websocket

        # Cache condition_id -> token_id mappings
        self._token_id_cache: dict[str, str] = {}
        self._condition_id_cache: dict[str, str] = {}  # Reverse mapping

    async def __aenter__(self) -> "HybridPolymarketClient":
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await self.disconnect()

    async def connect(self) -> None:
        """Connect REST client. WebSocket connects on first subscribe."""
        await self._rest.connect()

    async def disconnect(self) -> None:
        """Disconnect both clients."""
        await self._ws.disconnect()
        await self._rest.disconnect()

    @property
    def platform(self) -> Platform:
        return Platform.POLYMARKET

    @property
    def ws_connected(self) -> bool:
        """Check if WebSocket is connected."""
        return self._ws.connected

    @property
    def subscribed_markets(self) -> set[str]:
        """Get set of token_ids subscribed via WebSocket."""
        return self._ws.subscribed_markets

    # ==========================================================================
    # Token ID Resolution
    # ==========================================================================

    async def resolve_yes_token_id(self, market: dict) -> Optional[str]:
        """Resolve YES token_id with caching.

        Args:
            market: Market dictionary from API

        Returns:
            Token ID for YES outcome, or None if not found
        """
        condition_id = market.get("condition_id") or market.get("id")
        if not condition_id:
            return None

        # Check cache
        if condition_id in self._token_id_cache:
            return self._token_id_cache[condition_id]

        # Resolve via REST client
        token_id = await self._rest.resolve_yes_token_id(market)

        # Cache result
        if token_id:
            self._token_id_cache[condition_id] = token_id
            self._condition_id_cache[token_id] = condition_id

        return token_id

    def get_condition_id(self, token_id: str) -> Optional[str]:
        """Get condition_id for a token_id from cache."""
        return self._condition_id_cache.get(token_id)

    # ==========================================================================
    # REST API Delegation
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """Get markets from Polymarket (REST)."""
        return await self._rest.get_markets(sport=sport, status=status, limit=limit)

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get detailed information about a specific market (REST)."""
        return await self._rest.get_market(market_id)

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get order book for a market.

        Args:
            market_id: Either condition_id or token_id

        If subscribed via WebSocket, returns local orderbook state.
        Otherwise falls back to REST.
        """
        # Check if it's a token_id and we have WebSocket data
        if self._prefer_websocket and market_id in self._ws.subscribed_markets:
            book = self._ws.get_orderbook(market_id)
            if book:
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
                    platform=Platform.POLYMARKET,
                    yes_bids=yes_bids,
                    yes_asks=yes_asks,
                    timestamp=book.last_update,
                )

        # Check if it's a condition_id with a cached token_id
        if market_id in self._token_id_cache:
            token_id = self._token_id_cache[market_id]
            if token_id in self._ws.subscribed_markets:
                book = self._ws.get_orderbook(token_id)
                if book:
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
                        platform=Platform.POLYMARKET,
                        yes_bids=yes_bids,
                        yes_asks=yes_asks,
                        timestamp=book.last_update,
                    )

        return await self._rest.get_orderbook(market_id)

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price.

        Args:
            market_id: Either condition_id or token_id

        If subscribed via WebSocket, returns local orderbook state.
        Otherwise falls back to REST.
        """
        # Check if it's a token_id
        if self._prefer_websocket and market_id in self._ws.subscribed_markets:
            price = self._ws.get_market_price(market_id)
            if price:
                return price

        # Check if it's a condition_id
        if market_id in self._token_id_cache:
            token_id = self._token_id_cache[market_id]
            if token_id in self._ws.subscribed_markets:
                price = self._ws.get_market_price(token_id)
                if price:
                    return price

        return await self._rest.get_market_price(market_id)

    async def get_sports_markets(self, limit: int = 100) -> list[dict]:
        """Get all sports-related markets (REST)."""
        return await self._rest.get_sports_markets(limit)

    async def search_markets(self, query: str, limit: int = 50) -> list[dict]:
        """Search for markets by keyword (REST)."""
        return await self._rest.search_markets(query, limit)

    async def get_trades(self, market_id: str, limit: int = 100) -> list[dict]:
        """Get recent trades for a market (REST)."""
        return await self._rest.get_trades(market_id, limit)

    async def health_check(self) -> bool:
        """Check if API is accessible (REST)."""
        return await self._rest.health_check()

    # ==========================================================================
    # WebSocket Streaming
    # ==========================================================================

    async def subscribe(self, token_ids: list[str]) -> None:
        """Subscribe to market updates via WebSocket.

        Args:
            token_ids: List of token IDs (not condition IDs!)
        """
        await self._ws.subscribe(token_ids)

    async def unsubscribe(self, token_ids: list[str]) -> None:
        """Unsubscribe from market updates."""
        await self._ws.unsubscribe(token_ids)

    async def stream_prices(
        self,
        token_ids: list[str],
    ) -> AsyncIterator[MarketPrice]:
        """Stream price updates via WebSocket.

        Args:
            token_ids: Token IDs to subscribe to (not condition IDs!)

        Yields:
            MarketPrice objects on each update
        """
        async for price in self._ws.stream_prices(token_ids):
            yield price

    async def subscribe_with_metadata(
        self,
        markets: list[dict],
    ) -> None:
        """Subscribe to markets with metadata.

        Args:
            markets: List of dicts with 'token_id', 'title', 'game_id', 'condition_id' keys
        """
        # Update condition_id cache
        for m in markets:
            token_id = m.get("token_id")
            condition_id = m.get("condition_id")
            if token_id and condition_id:
                self._token_id_cache[condition_id] = token_id
                self._condition_id_cache[token_id] = condition_id

        await self._ws.subscribe_with_metadata(markets)

    # ==========================================================================
    # Convenience Methods
    # ==========================================================================

    async def get_sports_markets_and_subscribe(
        self,
        sport: Optional[str] = None,
        limit: int = 50,
    ) -> list[dict]:
        """Get sports markets and auto-subscribe to WebSocket.

        This method:
        1. Fetches markets via REST
        2. Resolves token IDs for each market
        3. Subscribes to WebSocket with metadata

        Args:
            sport: Sport to filter (optional)
            limit: Max markets to fetch

        Returns:
            List of market dictionaries (with token_id added)
        """
        if sport:
            markets = await self.get_markets(sport=sport, limit=limit)
        else:
            markets = await self.get_sports_markets(limit=limit)

        # Resolve token IDs
        ws_markets = []
        for market in markets:
            token_id = await self.resolve_yes_token_id(market)
            if token_id:
                market["yes_token_id"] = token_id
                ws_markets.append({
                    "token_id": token_id,
                    "condition_id": market.get("condition_id") or market.get("id"),
                    "title": market.get("question", market.get("title", "")),
                    "game_id": market.get("game_id"),
                    "volume": float(market.get("volume", 0) or 0),
                })

        # Subscribe to WebSocket
        if ws_markets:
            await self.subscribe_with_metadata(ws_markets)

        return markets

    async def get_multi_market_prices(
        self,
        market_ids: list[str],
    ) -> dict[str, Optional[MarketPrice]]:
        """Get prices for multiple markets efficiently.

        Args:
            market_ids: List of market IDs (condition_ids or token_ids)

        Returns:
            Dictionary mapping market_id -> MarketPrice (or None)
        """
        results: dict[str, Optional[MarketPrice]] = {}

        ws_markets = []
        rest_markets = []

        for market_id in market_ids:
            # Check if it's a subscribed token_id
            if market_id in self._ws.subscribed_markets:
                ws_markets.append(market_id)
            # Check if it's a condition_id with subscribed token
            elif market_id in self._token_id_cache:
                token_id = self._token_id_cache[market_id]
                if token_id in self._ws.subscribed_markets:
                    ws_markets.append(market_id)
                else:
                    rest_markets.append(market_id)
            else:
                rest_markets.append(market_id)

        # Get WebSocket prices (instant)
        for market_id in ws_markets:
            if market_id in self._ws.subscribed_markets:
                results[market_id] = self._ws.get_market_price(market_id)
            elif market_id in self._token_id_cache:
                token_id = self._token_id_cache[market_id]
                price = self._ws.get_market_price(token_id)
                results[market_id] = price

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

    async def resolve_and_subscribe_markets(
        self,
        markets: list[dict],
    ) -> dict[str, str]:
        """Resolve token IDs and subscribe in one call.

        Args:
            markets: List of market dicts (from get_markets)

        Returns:
            Dict mapping condition_id -> token_id for resolved markets
        """
        resolved: dict[str, str] = {}
        ws_markets = []

        for market in markets:
            token_id = await self.resolve_yes_token_id(market)
            if token_id:
                condition_id = market.get("condition_id") or market.get("id")
                if condition_id:
                    resolved[condition_id] = token_id
                    ws_markets.append({
                        "token_id": token_id,
                        "condition_id": condition_id,
                        "title": market.get("question", market.get("title", "")),
                        "game_id": market.get("game_id"),
                        "volume": float(market.get("volume", 0) or 0),
                    })

        if ws_markets:
            await self.subscribe_with_metadata(ws_markets)

        return resolved
