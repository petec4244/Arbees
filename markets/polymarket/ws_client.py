"""
Polymarket WebSocket client for real-time market data streaming.

Reference: https://docs.polymarket.com/developers/CLOB/websocket/wss-overview
           https://docs.polymarket.com/developers/CLOB/websocket/market-channel

Key concepts:
- No authentication required for market data
- MANDATORY: Ping every 5 seconds or connection will be terminated
- Subscribe to "market" channel with token_ids (not condition_ids)
- Message types: "book", "price_change", "last_trade_price"
"""

import asyncio
import logging
import time
from datetime import datetime
from typing import Any, Optional

import aiohttp

from arbees_shared.models.market import MarketPrice, Platform
from markets.base_ws import BaseWebSocketClient, LocalOrderBook

logger = logging.getLogger(__name__)


class PolymarketWebSocketClient(BaseWebSocketClient):
    """Polymarket WebSocket client for real-time market data.

    Features:
    - No authentication required
    - Automatic 5-second ping keepalive (mandatory)
    - Book snapshots and price updates
    - Local orderbook maintenance

    Note:
        Polymarket uses token_ids for WebSocket subscriptions, not condition_ids.
        Make sure to resolve token IDs before subscribing.
    """

    WS_URL = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
    PING_INTERVAL = 5.0  # Mandatory ping every 5 seconds

    def __init__(self):
        """Initialize Polymarket WebSocket client."""
        super().__init__(
            ws_url=self.WS_URL,
            platform=Platform.POLYMARKET,
            # Disable aiohttp heartbeat - we use custom ping
            heartbeat_interval=0,
        )

        self._ping_task: Optional[asyncio.Task] = None

    async def connect(self) -> None:
        """Establish WebSocket connection and start ping loop."""
        if self._session is None:
            self._session = aiohttp.ClientSession()

        try:
            # Polymarket doesn't require auth headers
            self._ws = await self._session.ws_connect(
                self.ws_url,
                heartbeat=None,  # We handle our own pings
            )
            self._connected = True
            self._reconnect_delay = self.reconnect_min_delay
            logger.info(f"Polymarket WebSocket connected to {self.ws_url}")

            # Start mandatory ping loop
            if self._ping_task is None or self._ping_task.done():
                self._ping_task = asyncio.create_task(self._ping_loop())

        except Exception as e:
            logger.error(f"Polymarket WebSocket connection failed: {e}")
            raise

    async def disconnect(self) -> None:
        """Close WebSocket connection and stop ping loop."""
        self._connected = False

        if self._ping_task:
            self._ping_task.cancel()
            try:
                await self._ping_task
            except asyncio.CancelledError:
                pass
            self._ping_task = None

        await super().disconnect()

    async def _ping_loop(self) -> None:
        """Send ping every 5 seconds (MANDATORY for Polymarket).

        Polymarket will disconnect clients that don't ping regularly.
        """
        logger.debug("Starting Polymarket ping loop")

        while self._connected:
            try:
                if self._ws and not self._ws.closed:
                    await self._ws.ping()
                    logger.debug("Sent Polymarket ping")
                await asyncio.sleep(self.PING_INTERVAL)
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.warning(f"Polymarket ping error: {e}")
                break

    async def _authenticate(self) -> dict[str, str]:
        """No authentication required for Polymarket market data."""
        return {}

    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        """Build subscribe message for market channel.

        Polymarket subscribe format:
        {
            "type": "market",
            "assets_ids": [token_id_1, token_id_2, ...]
        }

        IMPORTANT: market_ids should be token_ids, not condition_ids.
        """
        return {
            "type": "market",
            "assets_ids": market_ids,
        }

    async def _build_unsubscribe_message(self, market_ids: list[str]) -> Optional[dict]:
        """Polymarket doesn't have explicit unsubscribe - just stop listening."""
        return None

    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        """Handle incoming WebSocket message.

        Message types:
        - book: Full orderbook snapshot
        - price_change: Price update
        - last_trade_price: Last trade price update

        Returns:
            MarketPrice if the message resulted in a price update
        """
        # Polymarket wraps data in an array
        if isinstance(msg, list):
            # Process each event in the array
            result = None
            for event in msg:
                price = await self._process_event(event)
                if price:
                    result = price
            return result

        return await self._process_event(msg)

    async def _process_event(self, event: dict) -> Optional[MarketPrice]:
        """Process a single event from the WebSocket."""
        event_type = event.get("event_type")

        if event_type == "book":
            return await self._handle_book(event)
        elif event_type == "price_change":
            return await self._handle_price_change(event)
        elif event_type == "last_trade_price":
            return await self._handle_last_trade(event)
        elif event_type == "tick_size_change":
            # Ignore tick size changes
            return None
        else:
            logger.debug(f"Unknown Polymarket event: {event_type}")
            return None

    async def _handle_book(self, event: dict) -> Optional[MarketPrice]:
        """Handle book snapshot event.

        Book format:
        {
            "event_type": "book",
            "asset_id": "token_id",
            "market": "condition_id",  # Optional
            "bids": [{"price": "0.55", "size": "100"}, ...],
            "asks": [{"price": "0.56", "size": "200"}, ...],
            "timestamp": "1234567890",
            "hash": "..."
        }
        """
        token_id = event.get("asset_id")
        if not token_id:
            return None

        # Parse bids (YES bids)
        yes_bids = []
        for level in event.get("bids") or []:
            try:
                price = float(level.get("price", 0))
                size = float(level.get("size", 0))
                if size > 0:
                    # Convert to cents
                    yes_bids.append((int(price * 100), size))
            except (ValueError, TypeError):
                continue

        # Parse asks - in Polymarket YES asks directly
        # (Polymarket has separate YES and NO order books)
        # Convert NO bids would be the opposite token
        no_bids = []
        for level in event.get("asks") or []:
            try:
                price = float(level.get("price", 0))
                size = float(level.get("size", 0))
                if size > 0:
                    # YES ask = NO bid at inverted price
                    # Store as NO bid for conversion in LocalOrderBook
                    no_bids.append((int((1.0 - price) * 100), size))
            except (ValueError, TypeError):
                continue

        # Get or create orderbook
        if token_id not in self._orderbooks:
            self._orderbooks[token_id] = LocalOrderBook(
                market_id=token_id,
                platform=Platform.POLYMARKET,
            )

        book = self._orderbooks[token_id]

        # For Polymarket, asks are direct YES asks, not NO bids
        # So we need to handle this differently
        book.yes_bids.clear()
        book.yes_asks.clear()

        for price_cents, qty in yes_bids:
            if qty > 0:
                book.yes_bids[price_cents] = qty

        # Store asks directly (they're YES asks, not NO bids)
        for level in event.get("asks") or []:
            try:
                price = float(level.get("price", 0))
                size = float(level.get("size", 0))
                if size > 0:
                    price_cents = int(price * 100)
                    book.yes_asks[price_cents] = size
            except (ValueError, TypeError):
                continue

        book.last_update = datetime.utcnow()

        logger.debug(
            f"Polymarket book {token_id[:16]}...: "
            f"bid={book.best_yes_bid:.2f if book.best_yes_bid else 0:.2f}, "
            f"ask={book.best_yes_ask:.2f if book.best_yes_ask else 1:.2f}"
        )

        meta = self._market_metadata.get(token_id, {})
        return book.to_market_price(
            market_title=meta.get("title", ""),
            game_id=meta.get("game_id"),
            volume=meta.get("volume", 0.0),
        )

    async def _handle_price_change(self, event: dict) -> Optional[MarketPrice]:
        """Handle price change event.

        Price change format:
        {
            "event_type": "price_change",
            "asset_id": "token_id",
            "price": "0.55",
            "side": "buy" | "sell",
            "size": "100"
        }
        """
        token_id = event.get("asset_id")
        if not token_id:
            return None

        book = self._orderbooks.get(token_id)
        if not book:
            # Create new orderbook
            book = LocalOrderBook(
                market_id=token_id,
                platform=Platform.POLYMARKET,
            )
            self._orderbooks[token_id] = book

        try:
            price = float(event.get("price", 0))
            size = float(event.get("size", 0))
            side = event.get("side", "").lower()

            price_cents = int(price * 100)

            if side == "buy":
                # Buy order = YES bid
                if size > 0:
                    book.yes_bids[price_cents] = size
                elif price_cents in book.yes_bids:
                    del book.yes_bids[price_cents]
            elif side == "sell":
                # Sell order = YES ask
                if size > 0:
                    book.yes_asks[price_cents] = size
                elif price_cents in book.yes_asks:
                    del book.yes_asks[price_cents]

            book.last_update = datetime.utcnow()

        except (ValueError, TypeError) as e:
            logger.warning(f"Error parsing price change: {e}")
            return None

        meta = self._market_metadata.get(token_id, {})
        return book.to_market_price(
            market_title=meta.get("title", ""),
            game_id=meta.get("game_id"),
            volume=meta.get("volume", 0.0),
        )

    async def _handle_last_trade(self, event: dict) -> Optional[MarketPrice]:
        """Handle last trade price event.

        Last trade format:
        {
            "event_type": "last_trade_price",
            "asset_id": "token_id",
            "price": "0.55"
        }
        """
        token_id = event.get("asset_id")
        if not token_id:
            return None

        try:
            price = float(event.get("price", 0))

            # Update metadata with last trade price
            if token_id not in self._market_metadata:
                self._market_metadata[token_id] = {}
            self._market_metadata[token_id]["last_trade_price"] = price

            # Get orderbook if exists
            book = self._orderbooks.get(token_id)
            if book:
                meta = self._market_metadata.get(token_id, {})
                return book.to_market_price(
                    market_title=meta.get("title", ""),
                    game_id=meta.get("game_id"),
                    volume=meta.get("volume", 0.0),
                    last_trade_price=price,
                )

        except (ValueError, TypeError) as e:
            logger.warning(f"Error parsing last trade: {e}")

        return None

    async def set_market_metadata(
        self,
        token_id: str,
        title: str = "",
        game_id: Optional[str] = None,
        condition_id: Optional[str] = None,
        **kwargs: Any,
    ) -> None:
        """Set metadata for a market (title, game_id, etc.).

        Args:
            token_id: The token ID used for WebSocket subscription
            title: Market title/question
            game_id: Associated game ID
            condition_id: The condition ID (alternative market identifier)
        """
        self._market_metadata[token_id] = {
            "title": title,
            "game_id": game_id,
            "condition_id": condition_id,
            **kwargs,
        }

    async def subscribe_with_metadata(
        self,
        markets: list[dict],
    ) -> None:
        """Subscribe to markets with metadata.

        Args:
            markets: List of dicts with 'token_id', 'title', 'game_id', 'condition_id' keys
        """
        token_ids = []
        for m in markets:
            token_id = m.get("token_id")
            if token_id:
                token_ids.append(token_id)
                self._market_metadata[token_id] = {
                    "title": m.get("title", ""),
                    "game_id": m.get("game_id"),
                    "condition_id": m.get("condition_id"),
                    "volume": m.get("volume", 0.0),
                }

        await self.subscribe(token_ids)

    def get_orderbook_by_condition_id(
        self,
        condition_id: str,
    ) -> Optional[LocalOrderBook]:
        """Get orderbook by condition_id (looks up via metadata)."""
        for token_id, meta in self._market_metadata.items():
            if meta.get("condition_id") == condition_id:
                return self._orderbooks.get(token_id)
        return None

    def get_market_price_by_condition_id(
        self,
        condition_id: str,
    ) -> Optional[MarketPrice]:
        """Get market price by condition_id (looks up via metadata)."""
        for token_id, meta in self._market_metadata.items():
            if meta.get("condition_id") == condition_id:
                book = self._orderbooks.get(token_id)
                if book:
                    return book.to_market_price(
                        market_title=meta.get("title", ""),
                        game_id=meta.get("game_id"),
                        volume=meta.get("volume", 0.0),
                        last_trade_price=meta.get("last_trade_price"),
                    )
        return None
