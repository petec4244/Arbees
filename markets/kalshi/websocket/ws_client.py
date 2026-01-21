"""
Kalshi WebSocket client for real-time market data streaming.

Provides 10-50ms latency vs 500-3000ms with REST polling.
"""

import asyncio
import json
import logging
import time
from datetime import datetime
from typing import AsyncIterator, Optional, Set

import websockets
from websockets.client import WebSocketClientProtocol

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
    
    WS_URL = "wss://api.elections.kalshi.com/trade-api/ws/v2"
    PING_INTERVAL = 30  # seconds
    RECONNECT_DELAY_BASE = 1.0  # seconds
    RECONNECT_DELAY_MAX = 60.0  # seconds
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        private_key_path: Optional[str] = None,
        private_key_str: Optional[str] = None,
    ):
        """
        Initialize Kalshi WebSocket client.

        Args:
            api_key: Kalshi API key for authentication (or KALSHI_API_KEY env var)
            private_key_path: Path to RSA private key PEM file (unused, for compatibility)
            private_key_str: RSA private key as string (unused, for compatibility)
        """
        import os
        self.api_key = api_key or os.environ.get("KALSHI_API_KEY", "")
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
    
    async def connect(self) -> None:
        """Connect to Kalshi WebSocket."""
        if self.is_connected:
            logger.warning("Already connected to Kalshi WebSocket")
            return
        
        try:
            logger.info(f"Connecting to Kalshi WebSocket: {self.WS_URL}")
            
            # Connect with authentication
            self._ws = await websockets.connect(
                self.WS_URL,
                extra_headers={
                    "Authorization": f"Bearer {self.api_key}",
                },
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
                    msg_type = data.get("type")
                    
                    if msg_type == "orderbook_delta":
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
        Parse Kalshi orderbook_delta message to MarketPrice.
        
        Args:
            data: WebSocket message data
            
        Returns:
            MarketPrice or None if parsing fails
        """
        try:
            market_ticker = data.get("market_ticker")
            if not market_ticker:
                return None
            
            # Extract best bid/ask from orderbook delta
            yes_bids = data.get("yes_bids", [])
            yes_asks = data.get("yes_asks", [])
            
            # Bids/asks are [price_cents, quantity] tuples
            yes_bid = yes_bids[0][0] / 100.0 if yes_bids else 0.0
            yes_ask = yes_asks[0][0] / 100.0 if yes_asks else 1.0
            
            # Calculate liquidity (sum of bid quantities)
            liquidity = sum(bid[1] for bid in yes_bids) if yes_bids else 0.0
            
            timestamp_ms = data.get("timestamp") or int(time.time() * 1000)
            
            return MarketPrice(
                market_id=market_ticker,
                platform=Platform.KALSHI,
                market_title=data.get("title", market_ticker),
                yes_bid=yes_bid,
                yes_ask=yes_ask,
                volume=0.0,  # Not in delta updates
                liquidity=liquidity,
                timestamp=datetime.fromtimestamp(timestamp_ms / 1000.0),
            )
            
        except Exception as e:
            logger.error(f"Error parsing Kalshi price update: {e}")
            return None
