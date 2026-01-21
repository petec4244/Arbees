"""
Polymarket WebSocket client for real-time orderbook streaming.

Provides 10-50ms latency for CLOB orderbook updates.
"""

import asyncio
import json
import logging
import time
from datetime import datetime
from typing import AsyncIterator, Optional, Set, Dict

import websockets
from websockets.client import WebSocketClientProtocol

from arbees_shared.models.market import MarketPrice, Platform

logger = logging.getLogger(__name__)


class PolymarketWebSocketClient:
    """
    Real-time WebSocket client for Polymarket CLOB orderbook data.
    
    Features:
    - Sub-50ms orderbook update latency
    - Automatic reconnection with exponential backoff
    - Token ID subscription management
    - Heartbeat to keep connection alive
    """
    
    WS_URL = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
    PING_INTERVAL = 30  # seconds
    RECONNECT_DELAY_BASE = 1.0  # seconds
    RECONNECT_DELAY_MAX = 60.0  # seconds
    
    def __init__(self):
        """Initialize Polymarket WebSocket client."""
        self._ws: Optional[WebSocketClientProtocol] = None
        self._subscribed_token_ids: Set[str] = set()
        self._token_metadata: Dict[str, dict] = {}  # token_id -> {condition_id, title, etc}
        self._running = False
        self._reconnect_count = 0
        
        # Message queue for async iteration
        self._message_queue: asyncio.Queue = asyncio.Queue(maxsize=1000)
        
        # Tasks
        self._receive_task: Optional[asyncio.Task] = None
        self._ping_task: Optional[asyncio.Task] = None
    
    @property
    def subscribed_markets(self) -> Set[str]:
        """Get currently subscribed token IDs."""
        return self._subscribed_token_ids.copy()
    
    @property
    def is_connected(self) -> bool:
        """Check if WebSocket is connected."""
        return self._ws is not None and not self._ws.closed
    
    async def connect(self) -> None:
        """Connect to Polymarket WebSocket."""
        if self.is_connected:
            logger.warning("Already connected to Polymarket WebSocket")
            return
        
        try:
            logger.info(f"Connecting to Polymarket WebSocket: {self.WS_URL}")
            
            self._ws = await websockets.connect(
                self.WS_URL,
                ping_interval=None,  # We handle ping ourselves
            )
            
            self._running = True
            self._reconnect_count = 0
            
            # Start background tasks
            self._receive_task = asyncio.create_task(self._receive_loop())
            self._ping_task = asyncio.create_task(self._ping_loop())
            
            logger.info("Connected to Polymarket WebSocket")
            
            # Re-subscribe to markets after reconnect
            if self._subscribed_token_ids:
                await self._resubscribe_all()
                
        except Exception as e:
            logger.error(f"Failed to connect to Polymarket WebSocket: {e}")
            raise
    
    async def disconnect(self) -> None:
        """Disconnect from Polymarket WebSocket."""
        logger.info("Disconnecting from Polymarket WebSocket")
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
        logger.info("Disconnected from Polymarket WebSocket")
    
    async def subscribe_with_metadata(self, markets: list[dict]) -> None:
        """
        Subscribe to market orderbook updates with metadata.
        
        Args:
            markets: List of dicts with {token_id, condition_id, title, game_id}
        """
        if not self.is_connected:
            raise RuntimeError("Not connected to Polymarket WebSocket")
        
        new_tokens = []
        for market in markets:
            token_id = market.get("token_id")
            if not token_id or token_id in self._subscribed_token_ids:
                continue
            
            new_tokens.append(token_id)
            
            # Store metadata
            self._token_metadata[token_id] = {
                "condition_id": market.get("condition_id", token_id),
                "title": market.get("title", ""),
                "game_id": market.get("game_id"),
            }
        
        if not new_tokens:
            return
        
        # Subscribe to orderbook updates for these tokens
        subscribe_msg = {
            "type": "subscribe",
            "channel": "market",
            "markets": new_tokens,
            "assets_ids": new_tokens,  # Alternative field name
        }
        
        await self._ws.send(json.dumps(subscribe_msg))
        self._subscribed_token_ids.update(new_tokens)
        
        logger.info(f"Subscribed to {len(new_tokens)} Polymarket token IDs: {new_tokens[:5]}...")
    
    async def subscribe(self, token_ids: list[str]) -> None:
        """
        Subscribe to token IDs without metadata.
        
        Args:
            token_ids: List of Polymarket token IDs
        """
        markets = [{"token_id": tid} for tid in token_ids]
        await self.subscribe_with_metadata(markets)
    
    async def unsubscribe(self, token_ids: list[str]) -> None:
        """
        Unsubscribe from token ID updates.
        
        Args:
            token_ids: List of Polymarket token IDs to unsubscribe from
        """
        if not self.is_connected:
            return
        
        tokens_to_remove = set(token_ids) & self._subscribed_token_ids
        if not tokens_to_remove:
            return
        
        # Send unsubscribe message
        unsubscribe_msg = {
            "type": "unsubscribe",
            "markets": list(tokens_to_remove),
        }
        
        await self._ws.send(json.dumps(unsubscribe_msg))
        self._subscribed_token_ids -= tokens_to_remove
        
        # Clean up metadata
        for token_id in tokens_to_remove:
            self._token_metadata.pop(token_id, None)
        
        logger.info(f"Unsubscribed from {len(tokens_to_remove)} Polymarket tokens")
    
    async def stream_prices(self) -> AsyncIterator[MarketPrice]:
        """
        Stream real-time price updates.
        
        Yields:
            MarketPrice objects as they arrive (10-50ms latency)
        """
        if not self.is_connected:
            raise RuntimeError("Not connected to Polymarket WebSocket")
        
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
                logger.error(f"Error streaming Polymarket prices: {e}")
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
                    
                    # Polymarket sends orderbook snapshots and updates
                    msg_type = data.get("type") or data.get("event_type")
                    
                    if msg_type in ("book", "price_change", "last_trade_price"):
                        # Price update - add to queue
                        try:
                            self._message_queue.put_nowait(data)
                        except asyncio.QueueFull:
                            logger.warning("Message queue full, dropping Polymarket price update")
                    
                    elif msg_type == "subscribed":
                        logger.debug(f"Polymarket subscription confirmed: {data.get('market')}")
                    
                    elif msg_type == "error":
                        logger.error(f"Polymarket WebSocket error: {data}")
                    
                except json.JSONDecodeError:
                    logger.warning(f"Invalid JSON from Polymarket WebSocket: {message}")
                except Exception as e:
                    logger.error(f"Error processing Polymarket WebSocket message: {e}")
        
        except websockets.exceptions.ConnectionClosed:
            logger.warning("Polymarket WebSocket connection closed")
            if self._running:
                asyncio.create_task(self._handle_reconnect())
        except Exception as e:
            logger.error(f"Error in Polymarket WebSocket receive loop: {e}")
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
                    logger.debug("Sent ping to Polymarket")
            
            except Exception as e:
                logger.error(f"Error in Polymarket ping loop: {e}")
    
    async def _handle_reconnect(self) -> None:
        """Handle WebSocket reconnection with exponential backoff."""
        if not self._running:
            return
        
        self._reconnect_count += 1
        delay = min(
            self.RECONNECT_DELAY_BASE * (2 ** self._reconnect_count),
            self.RECONNECT_DELAY_MAX
        )
        
        logger.info(f"Reconnecting to Polymarket WebSocket in {delay:.1f}s (attempt {self._reconnect_count})")
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
            logger.error(f"Polymarket reconnect failed: {e}")
            # Will try again on next disconnect
    
    async def _resubscribe_all(self) -> None:
        """Re-subscribe to all markets after reconnect."""
        if not self._subscribed_token_ids:
            return
        
        logger.info(f"Re-subscribing to {len(self._subscribed_token_ids)} Polymarket tokens")
        
        # Clear subscription state and re-subscribe
        markets = [
            {
                "token_id": token_id,
                **self._token_metadata.get(token_id, {})
            }
            for token_id in self._subscribed_token_ids
        ]
        self._subscribed_token_ids.clear()
        await self.subscribe_with_metadata(markets)
    
    def _parse_price_update(self, data: dict) -> Optional[MarketPrice]:
        """
        Parse Polymarket orderbook update to MarketPrice.
        
        Args:
            data: WebSocket message data
            
        Returns:
            MarketPrice or None if parsing fails
        """
        try:
            # Get token_id from various possible fields
            token_id = data.get("asset_id") or data.get("market") or data.get("token_id")
            if not token_id:
                return None
            
            # Get metadata
            metadata = self._token_metadata.get(token_id, {})
            condition_id = metadata.get("condition_id", token_id)
            title = metadata.get("title", token_id)
            game_id = metadata.get("game_id")
            
            # Extract best bid/ask
            # Polymarket format: {"bids": [[price, size], ...], "asks": [[price, size], ...]}
            bids = data.get("bids", [])
            asks = data.get("asks", [])
            
            yes_bid = float(bids[0][0]) if bids else 0.0
            yes_ask = float(asks[0][0]) if asks else 1.0
            
            # Calculate liquidity (sum of bid sizes)
            liquidity = sum(float(bid[1]) for bid in bids) if bids else 0.0
            
            return MarketPrice(
                market_id=condition_id,  # Use condition_id as market_id
                platform=Platform.POLYMARKET,
                market_title=title,
                yes_bid=yes_bid,
                yes_ask=yes_ask,
                volume=0.0,  # Not in orderbook updates
                liquidity=liquidity,
                game_id=game_id,
                timestamp=datetime.utcnow(),
            )
            
        except Exception as e:
            logger.error(f"Error parsing Polymarket price update: {e}")
            return None
