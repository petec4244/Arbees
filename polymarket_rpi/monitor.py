#!/usr/bin/env python3
"""
Polymarket RPi Monitor - ZMQ publisher with polling loop.

Polls Polymarket for sports market prices and publishes them via ZMQ.
Designed to run on Raspberry Pi with OpenVPN for geo-bypass.

Usage:
    python -m polymarket_rpi.monitor

Environment Variables:
    ZMQ_PUBLISH_ENABLED=true
    ZMQ_PUBLISH_ADDRESS=tcp://*:5555
    POLL_INTERVAL_SECONDS=10
    LOG_LEVEL=INFO
"""

import asyncio
import json
import logging
import os
import signal
from datetime import datetime
from typing import Optional

import zmq
import zmq.asyncio

from client import MarketPrice, PolymarketRPiClient

# Configure logging
LOG_LEVEL = os.environ.get("LOG_LEVEL", "INFO").upper()
logging.basicConfig(
    level=getattr(logging, LOG_LEVEL, logging.INFO),
    format="%(asctime)s | %(levelname)-8s | %(name)s | %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger(__name__)


class ZMQPolymarketMonitor:
    """Monitors Polymarket and publishes prices via ZMQ."""

    def __init__(
        self,
        zmq_address: str = "tcp://*:5555",
        zmq_enabled: bool = True,
        poll_interval: float = 10.0,
        health_port: int = 8001,
    ):
        """
        Initialize the monitor.

        Args:
            zmq_address: ZMQ bind address for publishing
            zmq_enabled: Whether to enable ZMQ publishing
            poll_interval: Seconds between poll cycles
            health_port: Port for health check HTTP server
        """
        self.zmq_address = zmq_address
        self.zmq_enabled = zmq_enabled
        self.poll_interval = poll_interval
        self.health_port = health_port

        self._running = False
        self._client: Optional[PolymarketRPiClient] = None
        self._zmq_context: Optional[zmq.asyncio.Context] = None
        self._zmq_socket: Optional[zmq.asyncio.Socket] = None

        # Stats
        self._polls_completed = 0
        self._prices_published = 0
        self._last_poll_time: Optional[datetime] = None
        self._errors = 0

    async def start(self) -> None:
        """Start the monitor."""
        logger.info("Starting Polymarket RPi Monitor")
        logger.info(f"  ZMQ Enabled:     {self.zmq_enabled}")
        logger.info(f"  ZMQ Address:     {self.zmq_address}")
        logger.info(f"  Poll Interval:   {self.poll_interval}s")

        # Initialize client
        self._client = PolymarketRPiClient()
        await self._client.connect()

        # Health check
        healthy = await self._client.health_check()
        if not healthy:
            logger.error("Polymarket API health check failed!")
            return

        logger.info("Polymarket API connection healthy")

        # Initialize ZMQ
        if self.zmq_enabled:
            self._zmq_context = zmq.asyncio.Context()
            self._zmq_socket = self._zmq_context.socket(zmq.PUB)
            self._zmq_socket.bind(self.zmq_address)
            logger.info(f"ZMQ publisher bound to {self.zmq_address}")

        self._running = True

        # Start tasks
        tasks = [
            asyncio.create_task(self._poll_loop()),
            asyncio.create_task(self._heartbeat_loop()),
        ]

        try:
            await asyncio.gather(*tasks)
        except asyncio.CancelledError:
            pass
        finally:
            await self.stop()

    async def stop(self) -> None:
        """Stop the monitor."""
        logger.info("Stopping monitor...")
        self._running = False

        if self._client:
            await self._client.disconnect()

        if self._zmq_socket:
            self._zmq_socket.close()
        if self._zmq_context:
            self._zmq_context.term()

        self._print_stats()

    async def _poll_loop(self) -> None:
        """Main polling loop."""
        while self._running:
            try:
                await self._poll_and_publish()
                self._polls_completed += 1
                self._last_poll_time = datetime.utcnow()
            except Exception as e:
                self._errors += 1
                logger.error(f"Poll error: {e}")

            await asyncio.sleep(self.poll_interval)

    async def _poll_and_publish(self) -> None:
        """Poll markets and publish prices."""
        logger.debug("Polling Polymarket sports markets...")

        # Get sports markets
        markets = await self._client.get_sports_markets(limit=50)
        if not markets:
            logger.warning("No sports markets found")
            return

        logger.info(f"Found {len(markets)} sports markets")

        # Fetch prices for each market
        prices: list[dict] = []
        for market in markets:
            market_id = market.get("condition_id") or market.get("id")
            if not market_id:
                continue

            try:
                price = await self._client.get_market_price(market_id)
                if price:
                    # Detect sport from tags
                    sport = self._detect_sport(market)
                    price.sport = sport
                    prices.append(price.to_dict())
            except Exception as e:
                logger.debug(f"Error fetching price for {market_id}: {e}")

        if not prices:
            logger.warning("No prices fetched")
            return

        logger.info(f"Fetched {len(prices)} prices")

        # Publish via ZMQ
        if self.zmq_enabled and self._zmq_socket:
            await self._publish_prices(prices)

    def _detect_sport(self, market: dict) -> Optional[str]:
        """Detect sport from market tags/title."""
        tags = market.get("tags", []) or []
        title = (market.get("question", "") + market.get("title", "")).lower()

        sport_keywords = {
            "NFL": ["nfl", "football", "touchdown", "super bowl", "quarterback"],
            "NBA": ["nba", "basketball", "lakers", "celtics", "lebron"],
            "NHL": ["nhl", "hockey", "stanley cup"],
            "MLB": ["mlb", "baseball", "world series", "home run"],
            "MMA": ["mma", "ufc", "fight", "knockout"],
            "Soccer": ["soccer", "premier league", "champions league", "fifa", "world cup"],
            "Tennis": ["tennis", "wimbledon", "open", "grand slam"],
        }

        # Check tags first
        for sport, keywords in sport_keywords.items():
            for tag in tags:
                if any(kw in tag.lower() for kw in keywords):
                    return sport

        # Check title
        for sport, keywords in sport_keywords.items():
            if any(kw in title for kw in keywords):
                return sport

        return None

    async def _publish_prices(self, prices: list[dict]) -> None:
        """Publish prices via ZMQ."""
        message = {
            "type": "polymarket_prices",
            "timestamp": datetime.utcnow().isoformat() + "Z",
            "source": "rpi_zmq",
            "prices": prices,
        }

        message_bytes = json.dumps(message).encode("utf-8")
        await self._zmq_socket.send(message_bytes)

        self._prices_published += len(prices)
        logger.debug(f"Published {len(prices)} prices via ZMQ")

    async def _heartbeat_loop(self) -> None:
        """Send periodic heartbeat messages."""
        while self._running:
            if self.zmq_enabled and self._zmq_socket:
                heartbeat = {
                    "type": "heartbeat",
                    "timestamp": datetime.utcnow().isoformat() + "Z",
                    "source": "rpi_zmq",
                    "stats": {
                        "polls_completed": self._polls_completed,
                        "prices_published": self._prices_published,
                        "errors": self._errors,
                    },
                }
                message_bytes = json.dumps(heartbeat).encode("utf-8")
                await self._zmq_socket.send(message_bytes)
                logger.debug("Heartbeat sent")

            await asyncio.sleep(30)  # Heartbeat every 30 seconds

    def _print_stats(self) -> None:
        """Print session statistics."""
        logger.info("=" * 50)
        logger.info("Session Statistics")
        logger.info(f"  Polls completed:   {self._polls_completed}")
        logger.info(f"  Prices published:  {self._prices_published}")
        logger.info(f"  Errors:            {self._errors}")
        if self._last_poll_time:
            logger.info(f"  Last poll:         {self._last_poll_time.isoformat()}")
        logger.info("=" * 50)


async def main():
    """Main entry point."""
    # Load configuration from environment
    zmq_enabled = os.environ.get("ZMQ_PUBLISH_ENABLED", "true").lower() == "true"
    zmq_address = os.environ.get("ZMQ_PUBLISH_ADDRESS", "tcp://*:5555")
    poll_interval = float(os.environ.get("POLL_INTERVAL_SECONDS", "10"))
    health_port = int(os.environ.get("HEALTH_PORT", "8001"))

    monitor = ZMQPolymarketMonitor(
        zmq_address=zmq_address,
        zmq_enabled=zmq_enabled,
        poll_interval=poll_interval,
        health_port=health_port,
    )

    # Setup signal handlers
    loop = asyncio.get_event_loop()

    def signal_handler():
        logger.info("Received shutdown signal")
        asyncio.create_task(monitor.stop())

    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, signal_handler)
        except NotImplementedError:
            pass

    try:
        await monitor.start()
    except KeyboardInterrupt:
        await monitor.stop()


if __name__ == "__main__":
    asyncio.run(main())
