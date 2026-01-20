#!/usr/bin/env python3
"""
ZMQ Polymarket Listener - Test script for validating ZMQ communication from RPi.

Connects to the RPi ZMQ publisher and optionally bridges messages to Redis.

Usage:
    python scripts/zmq_polymarket_listener.py --address tcp://192.168.1.100:5555 --verbose
    python scripts/zmq_polymarket_listener.py --address tcp://192.168.1.100:5555 --redis
"""

import argparse
import asyncio
import json
import logging
import os
import signal
import sys
from datetime import datetime
from typing import Optional

# Optional imports
try:
    import zmq
    import zmq.asyncio
    ZMQ_AVAILABLE = True
except ImportError:
    ZMQ_AVAILABLE = False

try:
    from arbees_shared.messaging.redis_bus import RedisBus, Channel
    from arbees_shared.models.market import MarketPrice, Platform
    REDIS_AVAILABLE = True
except ImportError:
    REDIS_AVAILABLE = False

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)-8s | %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger(__name__)


class ZMQPolymarketListener:
    """Listens to ZMQ messages from RPi Polymarket monitor."""

    def __init__(
        self,
        zmq_address: str,
        enable_redis: bool = False,
        redis_url: Optional[str] = None,
        verbose: bool = False,
    ):
        self.zmq_address = zmq_address
        self.enable_redis = enable_redis
        self.redis_url = redis_url or os.environ.get("REDIS_URL", "redis://localhost:6379")
        self.verbose = verbose

        self._running = False
        self._context: Optional["zmq.asyncio.Context"] = None
        self._socket: Optional["zmq.asyncio.Socket"] = None
        self._redis_bus: Optional["RedisBus"] = None

        # Stats
        self._messages_received = 0
        self._prices_processed = 0
        self._last_message_time: Optional[datetime] = None

    async def start(self) -> None:
        """Start the listener."""
        if not ZMQ_AVAILABLE:
            logger.error("pyzmq not installed. Run: pip install pyzmq")
            return

        logger.info(f"Starting ZMQ listener on {self.zmq_address}")

        # Initialize ZMQ
        self._context = zmq.asyncio.Context()
        self._socket = self._context.socket(zmq.SUB)
        self._socket.connect(self.zmq_address)
        self._socket.setsockopt_string(zmq.SUBSCRIBE, "")  # Subscribe to all

        # Set socket options
        self._socket.setsockopt(zmq.RCVTIMEO, 5000)  # 5 second timeout
        self._socket.setsockopt(zmq.LINGER, 0)

        # Initialize Redis if enabled
        if self.enable_redis:
            if not REDIS_AVAILABLE:
                logger.warning("Redis bridge requested but arbees_shared not available")
            else:
                self._redis_bus = RedisBus(self.redis_url)
                await self._redis_bus.connect()
                logger.info(f"Redis bridge enabled: {self.redis_url}")

        self._running = True
        logger.info("Listener started. Waiting for messages...")

        await self._listen_loop()

    async def stop(self) -> None:
        """Stop the listener."""
        self._running = False
        logger.info("Stopping listener...")

        if self._socket:
            self._socket.close()
        if self._context:
            self._context.term()
        if self._redis_bus:
            await self._redis_bus.disconnect()

        self._print_stats()

    async def _listen_loop(self) -> None:
        """Main message receiving loop."""
        while self._running:
            try:
                # Receive with timeout
                message_bytes = await self._socket.recv()
                await self._handle_message(message_bytes)

            except zmq.Again:
                # Timeout, continue
                if self.verbose:
                    logger.debug("No message received (timeout)")
                continue
            except zmq.ZMQError as e:
                if self._running:
                    logger.error(f"ZMQ error: {e}")
                    await asyncio.sleep(1)
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Error in listen loop: {e}")
                await asyncio.sleep(1)

    async def _handle_message(self, message_bytes: bytes) -> None:
        """Handle a received ZMQ message."""
        self._messages_received += 1
        self._last_message_time = datetime.utcnow()

        try:
            data = json.loads(message_bytes.decode("utf-8"))
        except json.JSONDecodeError as e:
            logger.error(f"Invalid JSON: {e}")
            return

        msg_type = data.get("type", "unknown")
        timestamp = data.get("timestamp", "")
        source = data.get("source", "unknown")

        if self.verbose:
            logger.info(f"Received: type={msg_type}, source={source}, timestamp={timestamp}")

        if msg_type == "polymarket_prices":
            await self._handle_prices(data)
        elif msg_type == "heartbeat":
            logger.debug(f"Heartbeat from {source}")
        else:
            logger.warning(f"Unknown message type: {msg_type}")

    async def _handle_prices(self, data: dict) -> None:
        """Handle polymarket_prices message."""
        prices = data.get("prices", [])
        logger.info(f"Received {len(prices)} price updates")

        for price_data in prices:
            self._prices_processed += 1

            market_id = price_data.get("market_id", "")
            yes_bid = price_data.get("yes_bid", 0)
            yes_ask = price_data.get("yes_ask", 0)
            title = price_data.get("market_title", "")[:50]

            if self.verbose:
                logger.info(
                    f"  {market_id[:12]}... | bid={yes_bid:.3f} ask={yes_ask:.3f} | {title}"
                )

            # Bridge to Redis if enabled
            if self._redis_bus and REDIS_AVAILABLE:
                await self._publish_to_redis(price_data)

    async def _publish_to_redis(self, price_data: dict) -> None:
        """Publish price to Redis channel."""
        try:
            # Create MarketPrice model
            market_price = MarketPrice(
                market_id=price_data.get("market_id", ""),
                platform=Platform.POLYMARKET,
                game_id=price_data.get("game_id"),
                market_title=price_data.get("market_title", ""),
                yes_bid=price_data.get("yes_bid", 0),
                yes_ask=price_data.get("yes_ask", 1),
                volume=price_data.get("volume", 0),
                liquidity=price_data.get("liquidity", 0),
            )

            # Publish to ZMQ-specific channel
            channel = "polymarket:zmq:prices"
            await self._redis_bus.publish(channel, market_price)

            if self.verbose:
                logger.debug(f"Published to Redis: {channel}")

        except Exception as e:
            logger.error(f"Redis publish error: {e}")

    def _print_stats(self) -> None:
        """Print statistics."""
        logger.info("=" * 50)
        logger.info("Session Statistics")
        logger.info(f"  Messages received: {self._messages_received}")
        logger.info(f"  Prices processed:  {self._prices_processed}")
        if self._last_message_time:
            logger.info(f"  Last message:      {self._last_message_time.isoformat()}")
        logger.info("=" * 50)


async def main():
    parser = argparse.ArgumentParser(
        description="ZMQ Polymarket Listener - Test script for RPi communication"
    )
    parser.add_argument(
        "--address",
        default=os.environ.get("ZMQ_RPI_ADDRESS", "tcp://localhost:5555"),
        help="ZMQ address to connect to (default: tcp://localhost:5555)",
    )
    parser.add_argument(
        "--redis",
        action="store_true",
        help="Enable Redis bridge (publishes to polymarket:zmq:prices)",
    )
    parser.add_argument(
        "--redis-url",
        default=os.environ.get("REDIS_URL", "redis://localhost:6379"),
        help="Redis URL for bridge mode",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output",
    )
    args = parser.parse_args()

    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    listener = ZMQPolymarketListener(
        zmq_address=args.address,
        enable_redis=args.redis,
        redis_url=args.redis_url,
        verbose=args.verbose,
    )

    # Setup signal handlers
    loop = asyncio.get_event_loop()

    def signal_handler():
        logger.info("Received shutdown signal")
        asyncio.create_task(listener.stop())

    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, signal_handler)
        except NotImplementedError:
            # Windows doesn't support add_signal_handler
            pass

    try:
        await listener.start()
    except KeyboardInterrupt:
        await listener.stop()


if __name__ == "__main__":
    asyncio.run(main())
