"""
Kalshi WebSocket client for real-time market data streaming.

Provides 10-50ms latency vs 500-3000ms with REST polling.

Auth: Uses RSA-PSS signed headers (KALSHI-ACCESS-*), same as REST API.
Schema: Kalshi sends 'yes' and 'no' bid arrays. To get YES ask, use 100 - best NO bid.
"""

import asyncio
import base64
import json
import logging
import time
from datetime import datetime
from typing import AsyncIterator, Optional, Set

import websockets
from websockets.client import WebSocketClientProtocol
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.primitives.asymmetric.rsa import RSAPrivateKey

from arbees_shared.models.market import MarketPrice, Platform

logger = logging.getLogger(__name__)


class KalshiWebSocketClient:
    """
    Real-time WebSocket client for Kalshi market data.
    
    Features:
    - Sub-50ms price update latency
    - Automatic reconnection with exponential backoff
    - Subscription management (add/remove markets dynamically)
    - Heartbeat/ping-pong to keep connection alive
    """
    
    # Default URL (overridden by config)
    WS_URL = "wss://api.elections.kalshi.com/trade-api/ws/v2"
    PING_INTERVAL = 30  # seconds
    RECONNECT_DELAY_BASE = 1.0  # seconds
    RECONNECT_DELAY_MAX = 60.0  # seconds
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        private_key_path: Optional[str] = None,
        private_key_str: Optional[str] = None,
        ws_url: Optional[str] = None,
        env: Optional[str] = None,
    ):
        """
        Initialize Kalshi WebSocket client.

        Args:
            api_key: Kalshi API key for authentication (or from env based on KALSHI_ENV)
            private_key_path: Path to RSA private key PEM file (unused, for compatibility)
            private_key_str: RSA private key as string (unused, for compatibility)
            ws_url: Override WebSocket URL (or use KALSHI_WS_URL env var)
            env: Environment name ("prod" or "demo"), defaults to KALSHI_ENV
        """
        import os
        from markets.kalshi.config import (
            KalshiEnvironment,
            get_kalshi_ws_url,
            get_kalshi_api_key,
            get_kalshi_private_key,
            get_kalshi_private_key_path,
        )
        
        # Resolve environment
        kalshi_env = None
        if env:
            try:
                kalshi_env = KalshiEnvironment(env.lower())
            except ValueError:
                pass
        
        # Resolve WebSocket URL
        self._ws_url = get_kalshi_ws_url(env=kalshi_env, override_url=ws_url)
        
        # Resolve API key
        self.api_key = api_key or get_kalshi_api_key(env=kalshi_env)
        
        # Load RSA private key for signing (required for authenticated WS)
        self._private_key: Optional[RSAPrivateKey] = None
        if private_key_str:
            self._load_private_key_from_string(private_key_str)
        elif private_key_path:
            self._load_private_key_from_file(private_key_path)
        else:
            # Try environment-aware key resolution
            env_key_str = get_kalshi_private_key(env=kalshi_env)
            env_key_path = get_kalshi_private_key_path(env=kalshi_env)
            if env_key_str:
                self._load_private_key_from_string(env_key_str)
            elif env_key_path:
                self._load_private_key_from_file(env_key_path)
        self._ws: Optional[WebSocketClientProtocol] = None
        self._subscribed_markets: Set[str] = set()
        self._running = False
        self._reconnect_count = 0
        
        # Message queue for async iteration
        self._message_queue: asyncio.Queue = asyncio.Queue(maxsize=1000)
        
        # Tasks
        self._receive_task: Optional[asyncio.Task] = None
        self._ping_task: Optional[asyncio.Task] = None
    
    @property
    def subscribed_markets(self) -> Set[str]:
        """Get currently subscribed market IDs."""
        return self._subscribed_markets.copy()
    
    @property
    def is_connected(self) -> bool:
        """Check if WebSocket is connected."""
        return self._ws is not None and not self._ws.closed
    
    def _load_private_key_from_file(self, path: str) -> None:
        """Load RSA private key from PEM file."""
        try:
            with open(path, "rb") as f:
                self._private_key = serialization.load_pem_private_key(
                    f.read(),
                    password=None,
                )
            logger.info("Loaded Kalshi WS private key from file")
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
            logger.info("Loaded Kalshi WS private key from string")
        except Exception as e:
            logger.error(f"Failed to load private key from string: {e}")
            raise

    def _generate_signature(self, timestamp_ms: int, method: str, path: str) -> str:
        """Generate RSA-PSS signature for WebSocket authentication."""
        if not self._private_key:
            logger.warning("No private key loaded for Kalshi WS signature")
            return ""
        
        # Signature format: timestamp + method + path
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
    
    async def connect(self) -> None:
        """Connect to Kalshi WebSocket with RSA-signed authentication."""
        if self.is_connected:
            logger.warning("Already connected to Kalshi WebSocket")
            return
        
        try:
            logger.info(f"Connecting to Kalshi WebSocket: {self._ws_url}")
            
            # Generate RSA-PSS signed headers (matches Rust bot implementation)
            timestamp_ms = int(time.time() * 1000)
            # WS path for signature: /trade-api/ws/v2
            ws_path = "/trade-api/ws/v2"
            signature = self._generate_signature(timestamp_ms, "GET", ws_path)
            
            # Auth headers (same as REST API, per Kalshi docs)
            auth_headers = {
                "KALSHI-ACCESS-KEY": self.api_key,
                "KALSHI-ACCESS-TIMESTAMP": str(timestamp_ms),
                "KALSHI-ACCESS-SIGNATURE": signature,
            }
            
            # Connect with RSA-signed authentication
            self._ws = await websockets.connect(
                self._ws_url,
                extra_headers=auth_headers,
                ping_interval=None,  # We handle ping ourselves
            )
            
            self._running = True
            self._reconnect_count = 0
            
            # Start background tasks
            self._receive_task = asyncio.create_task(self._receive_loop())
            self._ping_task = asyncio.create_task(self._ping_loop())
            
            logger.info("Connected to Kalshi WebSocket")
            
            # Re-subscribe to markets after reconnect
            if self._subscribed_markets:
                await self._resubscribe_all()
                
        except Exception as e:
            logger.error(f"Failed to connect to Kalshi WebSocket: {e}")
            raise
    
    async def disconnect(self) -> None:
        """Disconnect from Kalshi WebSocket."""
        logger.info("Disconnecting from Kalshi WebSocket")
        self._running = False
        
        # Cancel tasks
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
        
        if self._ping_task:
            self._ping_task.cancel()
            try:
                await self._ping_task
            except asyncio.CancelledError:
                pass
        
        # Close connection
        if self._ws and not self._ws.closed:
            await self._ws.close()
        
        self._ws = None
        logger.info("Disconnected from Kalshi WebSocket")
    
    async def subscribe(self, market_ids: list[str]) -> None:
        """
        Subscribe to market price updates.
        
        Args:
            market_ids: List of Kalshi market tickers to subscribe to
        """
        if not self.is_connected:
            raise RuntimeError("Not connected to Kalshi WebSocket")
        
        new_markets = set(market_ids) - self._subscribed_markets
        if not new_markets:
            return
        
        # Send subscribe message
        subscribe_msg = {
            "type": "subscribe",
            "channels": [
                {
                    "name": "orderbook_delta",
                    "market_ticker": ticker,
                }
                for ticker in new_markets
            ]
        }
        
        await self._ws.send(json.dumps(subscribe_msg))
        self._subscribed_markets.update(new_markets)
        
        logger.info(f"Subscribed to {len(new_markets)} Kalshi markets: {list(new_markets)[:5]}...")
    
    async def unsubscribe(self, market_ids: list[str]) -> None:
        """
        Unsubscribe from market price updates.
        
        Args:
            market_ids: List of Kalshi market tickers to unsubscribe from
        """
        if not self.is_connected:
            return
        
        markets_to_remove = set(market_ids) & self._subscribed_markets
        if not markets_to_remove:
            return
        
        # Send unsubscribe message
        unsubscribe_msg = {
            "type": "unsubscribe",
            "channels": [
                {
                    "name": "orderbook_delta",
                    "market_ticker": ticker,
                }
                for ticker in markets_to_remove
            ]
        }
        
        await self._ws.send(json.dumps(unsubscribe_msg))
        self._subscribed_markets -= markets_to_remove
        
        logger.info(f"Unsubscribed from {len(markets_to_remove)} Kalshi markets")
    
    async def stream_prices(self) -> AsyncIterator[MarketPrice]:
        """
        Stream real-time price updates.
        
        Yields:
            MarketPrice objects as they arrive (10-50ms latency)
        """
        if not self.is_connected:
            raise RuntimeError("Not connected to Kalshi WebSocket")
        
        while self._running:
            try:
                # Wait for next message (with timeout to allow clean shutdown)
                message = await asyncio.wait_for(
                    self._message_queue.get(),
                    timeout=1.0
                )
                
                # Parse to MarketPrice
                price = self._parse_price_update(message)
                if price:
                    yield price
                    
            except asyncio.TimeoutError:
                continue
            except Exception as e:
                logger.error(f"Error streaming prices: {e}")
                break
    
    # ==========================================================================
    # Internal Methods
    # ==========================================================================
    
    async def _receive_loop(self) -> None:
        """Background task to receive WebSocket messages."""
        try:
            async for message in self._ws:
                if not self._running:
                    break
                
                try:
                    data = json.loads(message)
                    
                    # Handle different message types
                    # Kalshi uses 'type' for top-level message type, or 'msg_type' in wrapped format
                    msg_type = data.get("type") or data.get("msg_type")
                    
                    if msg_type in ("orderbook_delta", "orderbook_snapshot"):
                        # Price update - add to queue
                        try:
                            self._message_queue.put_nowait(data)
                        except asyncio.QueueFull:
                            logger.warning("Message queue full, dropping price update")
                    
                    elif msg_type == "pong":
                        # Pong response to our ping
                        logger.debug("Received pong from Kalshi")
                    
                    elif msg_type == "error":
                        logger.error(f"Kalshi WebSocket error: {data}")
                    
                    elif msg_type == "subscribed":
                        logger.debug(f"Subscription confirmed: {data.get('channel')}")
                    
                    elif msg_type is None:
                        # Unknown format - log for debugging
                        logger.debug(f"Kalshi WS message without type: {str(data)[:200]}")
                    
                except json.JSONDecodeError:
                    logger.warning(f"Invalid JSON from Kalshi WebSocket: {message}")
                except Exception as e:
                    logger.error(f"Error processing WebSocket message: {e}")
        
        except websockets.exceptions.ConnectionClosed:
            logger.warning("Kalshi WebSocket connection closed")
            if self._running:
                asyncio.create_task(self._handle_reconnect())
        except Exception as e:
            logger.error(f"Error in Kalshi WebSocket receive loop: {e}")
            if self._running:
                asyncio.create_task(self._handle_reconnect())
    
    async def _ping_loop(self) -> None:
        """Background task to send periodic pings."""
        while self._running:
            try:
                await asyncio.sleep(self.PING_INTERVAL)
                
                if self.is_connected:
                    ping_msg = {"type": "ping"}
                    await self._ws.send(json.dumps(ping_msg))
                    logger.debug("Sent ping to Kalshi")
            
            except Exception as e:
                logger.error(f"Error in ping loop: {e}")
    
    async def _handle_reconnect(self) -> None:
        """Handle WebSocket reconnection with exponential backoff."""
        if not self._running:
            return
        
        self._reconnect_count += 1
        delay = min(
            self.RECONNECT_DELAY_BASE * (2 ** self._reconnect_count),
            self.RECONNECT_DELAY_MAX
        )
        
        logger.info(f"Reconnecting to Kalshi WebSocket in {delay:.1f}s (attempt {self._reconnect_count})")
        await asyncio.sleep(delay)
        
        try:
            # Close old connection
            if self._ws:
                try:
                    await self._ws.close()
                except:
                    pass
            
            # Reconnect
            await self.connect()
            
        except Exception as e:
            logger.error(f"Reconnect failed: {e}")
            # Will try again on next disconnect
    
    async def _resubscribe_all(self) -> None:
        """Re-subscribe to all markets after reconnect."""
        if not self._subscribed_markets:
            return
        
        logger.info(f"Re-subscribing to {len(self._subscribed_markets)} markets")
        
        # Clear subscription state and re-subscribe
        markets = list(self._subscribed_markets)
        self._subscribed_markets.clear()
        await self.subscribe(markets)
    
    def _parse_price_update(self, data: dict) -> Optional[MarketPrice]:
        """
        Parse Kalshi orderbook_delta/orderbook_snapshot message to MarketPrice.
        
        Kalshi WebSocket schema (per official docs + Rust reference):
        - `msg.yes`: YES side bids (what people will pay to buy YES) - [[price_cents, qty], ...]
        - `msg.no`: NO side bids (what people will pay to buy NO) - [[price_cents, qty], ...]
        
        To compute effective prices:
        - YES bid = best YES bid (highest price in `yes` array)
        - YES ask = 100 - best NO bid (to buy YES, you sell NO at best NO bid)
        
        Args:
            data: WebSocket message data (contains `msg` with market data)
            
        Returns:
            MarketPrice or None if parsing fails
        """
        try:
            # Handle both wrapped (msg.market_ticker) and flat (market_ticker) formats
            msg = data.get("msg", data)
            market_ticker = msg.get("market_ticker")
            if not market_ticker:
                return None
            
            # Kalshi sends 'yes' and 'no' bid arrays (NOT yes_bids/yes_asks!)
            yes_side = msg.get("yes", [])  # YES bids: [[price_cents, qty], ...]
            no_side = msg.get("no", [])     # NO bids: [[price_cents, qty], ...]
            
            # Filter for levels with quantity > 0
            yes_levels = [(l[0], l[1]) for l in yes_side if len(l) >= 2 and l[1] > 0]
            no_levels = [(l[0], l[1]) for l in no_side if len(l) >= 2 and l[1] > 0]
            
            # Best YES bid = highest price in yes_levels
            best_yes_bid = max(yes_levels, key=lambda x: x[0]) if yes_levels else None
            # Best NO bid = highest price in no_levels
            best_no_bid = max(no_levels, key=lambda x: x[0]) if no_levels else None
            
            # YES bid = best YES bid price (what you get when selling YES)
            yes_bid = best_yes_bid[0] / 100.0 if best_yes_bid else 0.0
            yes_bid_size = float(best_yes_bid[1]) if best_yes_bid else 0.0
            
            # YES ask = 100 - best NO bid (to buy YES, you pay 100 - NO_bid)
            # This is because buying YES = selling NO at the NO bid
            if best_no_bid:
                yes_ask = (100 - best_no_bid[0]) / 100.0
                yes_ask_size = float(best_no_bid[1])
            else:
                yes_ask = 1.0
                yes_ask_size = 0.0
            
            # Calculate liquidity (sum of YES bid quantities)
            liquidity = sum(qty for _, qty in yes_levels) if yes_levels else 0.0
            
            timestamp_ms = msg.get("timestamp") or int(time.time() * 1000)
            
            return MarketPrice(
                market_id=market_ticker,
                platform=Platform.KALSHI,
                market_title=msg.get("title", market_ticker),
                yes_bid=yes_bid,
                yes_ask=yes_ask,
                yes_bid_size=yes_bid_size,
                yes_ask_size=yes_ask_size,
                volume=0.0,  # Not in delta updates
                liquidity=liquidity,
                timestamp=datetime.fromtimestamp(timestamp_ms / 1000.0),
            )
            
        except Exception as e:
            logger.error(f"Error parsing Kalshi price update: {e}")
            return None
