"""
Kalshi WebSocket client for real-time orderbook streaming.

Reference: https://docs.kalshi.com/websockets/websocket-connection
           https://docs.kalshi.com/websockets/orderbook-updates

Key concepts:
- Uses RSA-PSS signature authentication in WebSocket upgrade headers
- Subscribes to "orderbook_delta" channel for incremental updates
- Receives orderbook_snapshot on initial subscribe
- Receives orderbook_delta for incremental changes
- YES ask at X = NO bid at (100-X)
"""

import asyncio
import base64
import logging
import os
import time
from datetime import datetime
from typing import Any, Optional

import aiohttp
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.primitives.asymmetric.rsa import RSAPrivateKey

from arbees_shared.models.market import MarketPrice, Platform
from markets.base_ws import BaseWebSocketClient, LocalOrderBook

logger = logging.getLogger(__name__)


class KalshiWebSocketClient(BaseWebSocketClient):
    """Kalshi WebSocket client for real-time orderbook updates.

    Features:
    - RSA-PSS signature authentication
    - Orderbook snapshot and delta processing
    - Local orderbook maintenance
    - Automatic reconnection

    Authentication:
        Kalshi requires RSA-PSS signed headers for WebSocket connections.
        The signature is: timestamp_ms + "GET" + "/trade-api/ws/v2"
    """

    WS_URL = "wss://api.elections.kalshi.com/trade-api/ws/v2"

    def __init__(
        self,
        api_key: Optional[str] = None,
        private_key_path: Optional[str] = None,
        private_key_str: Optional[str] = None,
    ):
        """
        Initialize Kalshi WebSocket client.

        Args:
            api_key: Kalshi API key ID (or KALSHI_API_KEY env var)
            private_key_path: Path to RSA private key PEM file
            private_key_str: RSA private key as string (or KALSHI_PRIVATE_KEY env var)
        """
        super().__init__(
            ws_url=self.WS_URL,
            platform=Platform.KALSHI,
            heartbeat_interval=30.0,
        )

        self.api_key = api_key or os.environ.get("KALSHI_API_KEY", "")
        self._private_key: Optional[RSAPrivateKey] = None

        # Load private key
        if private_key_path:
            self._load_private_key_from_file(private_key_path)
        elif private_key_str:
            self._load_private_key_from_string(private_key_str)
        elif os.environ.get("KALSHI_PRIVATE_KEY"):
            self._load_private_key_from_string(os.environ["KALSHI_PRIVATE_KEY"])
        elif os.environ.get("KALSHI_PRIVATE_KEY_PATH"):
            self._load_private_key_from_file(os.environ["KALSHI_PRIVATE_KEY_PATH"])

        # Track subscription sequence IDs for delta ordering
        self._seq_numbers: dict[str, int] = {}

    def _load_private_key_from_file(self, path: str) -> None:
        """Load RSA private key from PEM file."""
        try:
            with open(path, "rb") as f:
                self._private_key = serialization.load_pem_private_key(
                    f.read(),
                    password=None,
                )
            logger.info("Loaded Kalshi private key from file")
        except Exception as e:
            logger.error(f"Failed to load private key from {path}: {e}")
            raise

    def _load_private_key_from_string(self, key_str: str) -> None:
        """Load RSA private key from PEM string."""
        try:
            self._private_key = serialization.load_pem_private_key(
                key_str.encode(),
                password=None,
            )
            logger.info("Loaded Kalshi private key from string")
        except Exception as e:
            logger.error(f"Failed to load private key from string: {e}")
            raise

    def _generate_signature(self, timestamp_ms: int, method: str, path: str) -> str:
        """Generate RSA-PSS signature for authentication."""
        if not self._private_key:
            return ""

        # Signature message: timestamp + method + path
        message = f"{timestamp_ms}{method}{path}"
        message_bytes = message.encode("utf-8")

        signature = self._private_key.sign(
            message_bytes,
            padding.PSS(
                mgf=padding.MGF1(hashes.SHA256()),
                salt_length=padding.PSS.DIGEST_LENGTH,
            ),
            hashes.SHA256(),
        )

        return base64.b64encode(signature).decode("utf-8")

    async def _authenticate(self) -> dict[str, str]:
        """Generate authentication headers for WebSocket upgrade."""
        if not self.api_key or not self._private_key:
            logger.warning("Kalshi credentials not configured - read-only mode")
            return {}

        timestamp_ms = int(time.time() * 1000)
        # WebSocket path for signature
        path = "/trade-api/ws/v2"
        signature = self._generate_signature(timestamp_ms, "GET", path)

        return {
            "KALSHI-ACCESS-KEY": self.api_key,
            "KALSHI-ACCESS-TIMESTAMP": str(timestamp_ms),
            "KALSHI-ACCESS-SIGNATURE": signature,
        }

    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        """Build subscribe message for orderbook delta channel.

        Kalshi WebSocket subscribe format:
        {
            "id": <unique_request_id>,
            "cmd": "subscribe",
            "params": {
                "channels": ["orderbook_delta"],
                "market_tickers": ["MARKET_1", "MARKET_2", ...]
            }
        }
        """
        return {
            "id": int(time.time() * 1000),
            "cmd": "subscribe",
            "params": {
                "channels": ["orderbook_delta"],
                "market_tickers": market_ids,
            },
        }

    async def _build_unsubscribe_message(self, market_ids: list[str]) -> Optional[dict]:
        """Build unsubscribe message."""
        return {
            "id": int(time.time() * 1000),
            "cmd": "unsubscribe",
            "params": {
                "channels": ["orderbook_delta"],
                "market_tickers": market_ids,
            },
        }

    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        """Handle incoming WebSocket message.

        Message types:
        - orderbook_snapshot: Full orderbook state on initial subscribe
        - orderbook_delta: Incremental updates

        Returns:
            MarketPrice if the message resulted in a price update
        """
        msg_type = msg.get("type")

        if msg_type == "orderbook_snapshot":
            return await self._handle_snapshot(msg)
        elif msg_type == "orderbook_delta":
            return await self._handle_delta(msg)
        elif msg_type == "subscribed":
            logger.debug(f"Kalshi subscription confirmed: {msg}")
            return None
        elif msg_type == "error":
            logger.error(f"Kalshi WebSocket error: {msg}")
            return None
        else:
            # Ignore other message types (heartbeat, etc.)
            return None

    async def _handle_snapshot(self, msg: dict) -> Optional[MarketPrice]:
        """Handle orderbook snapshot message.

        Snapshot format:
        {
            "type": "orderbook_snapshot",
            "msg": {
                "market_ticker": "MARKET_ID",
                "yes": [[price_cents, quantity], ...],  # YES bids
                "no": [[price_cents, quantity], ...]    # NO bids = YES asks at inverted price
            }
        }
        """
        data = msg.get("msg", {})
        market_id = data.get("market_ticker")

        if not market_id:
            return None

        # Parse YES bids
        yes_bids = []
        for level in data.get("yes") or []:
            if isinstance(level, (list, tuple)) and len(level) >= 2:
                price_cents = int(level[0])
                quantity = float(level[1])
                if quantity > 0:
                    yes_bids.append((price_cents, quantity))

        # Parse NO bids (will be converted to YES asks)
        no_bids = []
        for level in data.get("no") or []:
            if isinstance(level, (list, tuple)) and len(level) >= 2:
                price_cents = int(level[0])
                quantity = float(level[1])
                if quantity > 0:
                    no_bids.append((price_cents, quantity))

        # Get or create orderbook
        if market_id not in self._orderbooks:
            self._orderbooks[market_id] = LocalOrderBook(
                market_id=market_id,
                platform=Platform.KALSHI,
            )

        book = self._orderbooks[market_id]
        book.apply_snapshot(yes_bids, no_bids)

        # Reset sequence number
        self._seq_numbers[market_id] = data.get("seq", 0)

        bid = book.best_yes_bid
        ask = book.best_yes_ask
        logger.debug(
            f"Kalshi snapshot {market_id}: "
            f"bid={'N/A' if bid is None else f'{bid:.2f}'}, "
            f"ask={'N/A' if ask is None else f'{ask:.2f}'}"
        )

        return book.to_market_price(
            market_title=self._market_metadata.get(market_id, {}).get("title", ""),
            game_id=self._market_metadata.get(market_id, {}).get("game_id"),
        )

    async def _handle_delta(self, msg: dict) -> Optional[MarketPrice]:
        """Handle orderbook delta message.

        Delta format:
        {
            "type": "orderbook_delta",
            "msg": {
                "market_ticker": "MARKET_ID",
                "price": 50,           # Price level in cents
                "delta": 100,          # Change in quantity (positive = add, negative = remove)
                "side": "yes" | "no",  # Which side of the book
                "seq": 12345           # Sequence number
            }
        }
        """
        data = msg.get("msg", {})
        market_id = data.get("market_ticker")

        if not market_id:
            return None

        book = self._orderbooks.get(market_id)
        if not book:
            logger.warning(f"Delta for unknown market: {market_id}")
            return None

        # Check sequence number
        seq = data.get("seq", 0)
        expected_seq = self._seq_numbers.get(market_id, 0) + 1

        if seq != expected_seq and expected_seq > 1:
            logger.warning(
                f"Kalshi sequence gap for {market_id}: expected {expected_seq}, got {seq}"
            )
            # Request resync by resubscribing
            await self.subscribe([market_id])
            return None

        self._seq_numbers[market_id] = seq

        # Apply delta
        price_cents = int(data.get("price", 0))
        delta = float(data.get("delta", 0))
        side = data.get("side", "yes")

        # Map side to our format
        if side == "yes":
            book.apply_delta(price_cents, delta, "yes_bid")
        elif side == "no":
            # NO bid at X = YES ask at (100-X)
            book.apply_delta(price_cents, delta, "no_bid")

        return book.to_market_price(
            market_title=self._market_metadata.get(market_id, {}).get("title", ""),
            game_id=self._market_metadata.get(market_id, {}).get("game_id"),
        )

    async def set_market_metadata(
        self,
        market_id: str,
        title: str = "",
        game_id: Optional[str] = None,
        **kwargs: Any,
    ) -> None:
        """Set metadata for a market (title, game_id, etc.)."""
        self._market_metadata[market_id] = {
            "title": title,
            "game_id": game_id,
            **kwargs,
        }

    async def subscribe_with_metadata(
        self,
        markets: list[dict],
    ) -> None:
        """Subscribe to markets with metadata.

        Args:
            markets: List of dicts with 'market_id', 'title', 'game_id' keys
        """
        market_ids = []
        for m in markets:
            market_id = m.get("market_id")
            if market_id:
                market_ids.append(market_id)
                self._market_metadata[market_id] = {
                    "title": m.get("title", ""),
                    "game_id": m.get("game_id"),
                    "volume": m.get("volume", 0.0),
                }

        await self.subscribe(market_ids)
