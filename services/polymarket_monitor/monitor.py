"""
Dedicated Polymarket price monitor running behind VPN.

This service:
1. Runs inside a Docker container with network_mode: "service:vpn" (Gluetun)
2. Subscribes to market assignments from Orchestrator via Redis
3. Connects to Polymarket WebSocket for real-time prices
4. Normalizes token_id -> condition_id for downstream consumers
5. Publishes MarketPrice updates to Redis for GameShards to consume

Key design decisions:
- VPN verification on startup (fail hard if US IP detected)
- Uses existing HybridPolymarketClient for WebSocket streaming
- Publishes to game:{game_id}:price channel (same as direct Kalshi path)
- Maintains token_id <-> condition_id mapping for normalization
"""

import asyncio
import os
import signal
from datetime import datetime
from typing import Optional

import httpx
from loguru import logger

from arbees_shared.messaging.redis_bus import RedisBus, Channel, deserialize
from arbees_shared.models.market import MarketPrice, Platform
from markets.polymarket.hybrid_client import HybridPolymarketClient


class PolymarketMonitor:
    """
    Monitors Polymarket markets via VPN and publishes prices to Redis.

    Designed to run in Docker container with network_mode: "service:vpn"
    """

    def __init__(self):
        self.redis = RedisBus()
        self.poly_client = HybridPolymarketClient()

        # Subscription tracking
        self.subscribed_tokens: set[str] = set()
        self.subscribed_conditions: set[str] = set()

        # token_id <-> condition_id mappings
        self._token_to_condition: dict[str, str] = {}
        self._condition_to_token: dict[str, str] = {}

        # Market metadata (condition_id -> {game_id, market_type, title})
        self._market_metadata: dict[str, dict] = {}

        # State
        self._running = False
        self._health_ok = False
        self._last_price_time: Optional[datetime] = None
        self._prices_published = 0
        self._poll_interval_s = float(os.environ.get("POLYMARKET_POLL_INTERVAL_SECONDS", "2.0"))

    async def start(self):
        """Initialize connections and start monitoring."""
        logger.info("Starting PolymarketMonitor...")

        # Verify VPN before anything else
        await self._verify_vpn()
        self._health_ok = True

        # Connect to Redis
        await self.redis.connect()
        logger.info("Redis connected")

        # Connect to Polymarket (REST only initially, WS on first subscribe)
        await self.poly_client.connect()
        logger.info("Polymarket client connected")

        self._running = True

        # Setup signal handlers
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, lambda: asyncio.create_task(self.stop()))
            except NotImplementedError:
                pass  # Windows

        logger.info("PolymarketMonitor started successfully")

        # Run concurrent tasks
        await asyncio.gather(
            self._assignment_listener(),
            self._price_streaming_loop(),
            self._rest_poll_loop(),
            self._health_check_loop(),
            return_exceptions=True,
        )

    async def stop(self):
        """Graceful shutdown."""
        logger.info("Stopping PolymarketMonitor...")
        self._running = False

        await self.poly_client.disconnect()
        await self.redis.disconnect()

        logger.info(f"PolymarketMonitor stopped. Published {self._prices_published} prices.")

    async def _verify_vpn(self):
        """Verify VPN connection (must NOT be US IP)."""
        logger.info("Verifying VPN connection...")

        async with httpx.AsyncClient(timeout=15) as client:
            try:
                resp = await client.get("https://ipinfo.io/json")
                resp.raise_for_status()
                data = resp.json()

                country = data.get("country", "UNKNOWN")
                ip = data.get("ip", "UNKNOWN")
                city = data.get("city", "")
                org = data.get("org", "")

                if country == "US":
                    raise RuntimeError(
                        f"VPN not working! Detected US IP: {ip} ({city}). "
                        "Check NORDVPN_USER/PASS and VPN container health."
                    )

                logger.info(f"VPN verified: {country} ({city}) - {ip} - {org}")

            except httpx.RequestError as e:
                raise RuntimeError(f"Failed to verify VPN (network error): {e}")

    async def _assignment_listener(self):
        """Subscribe to market assignment messages from Orchestrator."""
        logger.info("Starting assignment listener...")

        # Subscribe to market assignments channel
        await self.redis.subscribe(
            Channel.MARKET_ASSIGNMENTS.value,
            self._handle_assignment,
        )

        # Start the listener
        await self.redis.start_listening()

    async def _handle_assignment(self, data: dict):
        """Handle a market assignment message from Orchestrator."""
        msg_type = data.get("type")

        if msg_type != "polymarket_assign":
            return

        game_id = data.get("game_id")
        sport = data.get("sport")
        markets = data.get("markets", [])

        if not game_id or not markets:
            return

        logger.info(f"Received assignment: game={game_id}, sport={sport}, markets={len(markets)}")

        for market_info in markets:
            condition_id = market_info.get("condition_id")
            market_type = market_info.get("market_type", "moneyline")

            if not condition_id or condition_id in self.subscribed_conditions:
                continue

            try:
                await self._subscribe_to_market(condition_id, game_id, market_type)
            except Exception as e:
                logger.error(f"Failed to subscribe to {condition_id}: {e}")

    async def _subscribe_to_market(
        self,
        condition_id: str,
        game_id: str,
        market_type: str,
    ):
        """Subscribe to a Polymarket market via WebSocket."""
        # Fetch market details
        market = await self.poly_client.get_market(condition_id)
        if not market:
            logger.warning(f"Market not found: {condition_id}")
            return

        # Resolve token_id
        token_id = await self.poly_client.resolve_yes_token_id(market)
        if not token_id:
            logger.warning(f"Could not resolve token_id for {condition_id}")
            return

        if token_id in self.subscribed_tokens:
            logger.debug(f"Already subscribed to token {token_id}")
            return

        # Store mappings
        self._token_to_condition[token_id] = condition_id
        self._condition_to_token[condition_id] = token_id

        # Store metadata
        title = market.get("question", market.get("title", ""))
        volume = float(market.get("volume", 0) or 0)

        self._market_metadata[condition_id] = {
            "game_id": game_id,
            "market_type": market_type,
            "title": title,
            "volume": volume,
        }

        # Subscribe via WebSocket
        await self.poly_client.subscribe_with_metadata([{
            "token_id": token_id,
            "condition_id": condition_id,
            "title": title,
            "game_id": game_id,
            "volume": volume,
            "market_type": market_type,
        }])

        self.subscribed_tokens.add(token_id)
        self.subscribed_conditions.add(condition_id)

        logger.info(f"Subscribed to Polymarket: {condition_id[:16]}... ({market_type}) -> token {token_id[:16]}...")

    async def _price_streaming_loop(self):
        """Stream prices from Polymarket WebSocket and publish to Redis."""
        logger.info("Starting price streaming loop...")

        while self._running:
            try:
                # Wait for subscriptions
                if not self.subscribed_tokens:
                    await asyncio.sleep(2)
                    continue

                token_ids = list(self.subscribed_tokens)
                logger.info(f"Streaming prices for {len(token_ids)} Polymarket markets")

                async for price in self.poly_client.stream_prices(token_ids):
                    if not self._running:
                        break

                    await self._handle_price_update(price)

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Price streaming error: {e}")
                self._health_ok = False
                await asyncio.sleep(5)

    async def _handle_price_update(self, price: MarketPrice):
        """Handle incoming price update, normalize, and publish to Redis."""
        # Normalize: price.market_id is token_id, we need condition_id
        token_id = price.market_id
        condition_id = self._token_to_condition.get(token_id)

        if not condition_id:
            # Try reverse lookup (maybe it's already a condition_id)
            if token_id in self._market_metadata:
                condition_id = token_id
            else:
                logger.debug(f"Unknown token_id: {token_id[:16]}...")
                return

        # Get metadata
        meta = self._market_metadata.get(condition_id, {})
        game_id = meta.get("game_id")

        if not game_id:
            logger.debug(f"No game_id for condition {condition_id[:16]}...")
            return

        # Build normalized MarketPrice with condition_id as market_id
        # (MarketPrice is frozen, so we create a new instance)
        normalized_price = MarketPrice(
            market_id=condition_id,
            platform=Platform.POLYMARKET,
            game_id=game_id,
            market_title=meta.get("title", price.market_title),
            yes_bid=price.yes_bid,
            yes_ask=price.yes_ask,
            volume=meta.get("volume", price.volume),
            liquidity=price.liquidity,
            status=price.status,
            timestamp=price.timestamp,
            last_trade_price=price.last_trade_price,
        )

        # Publish to per-game price channel
        await self.redis.publish_market_price(game_id, normalized_price)

        self._prices_published += 1
        self._last_price_time = datetime.utcnow()

        logger.debug(
            f"Published Polymarket price: {condition_id[:12]}... "
            f"bid={normalized_price.yes_bid:.3f} ask={normalized_price.yes_ask:.3f} "
            f"game={game_id}"
        )

    async def _rest_poll_loop(self) -> None:
        """Fallback poller to ensure we publish prices even if WS is quiet/flaky."""
        logger.info(f"Starting REST poll loop (interval={self._poll_interval_s}s)...")

        while self._running:
            try:
                if not self.subscribed_conditions:
                    await asyncio.sleep(self._poll_interval_s)
                    continue

                condition_ids = list(self.subscribed_conditions)
                for condition_id in condition_ids:
                    meta = self._market_metadata.get(condition_id, {})
                    game_id = meta.get("game_id")
                    if not game_id:
                        continue

                    try:
                        polled = await self.poly_client.get_market_price(condition_id)
                    except Exception as e:
                        logger.warning(f"REST poll failed for {condition_id}: {e}")
                        continue

                    if not polled:
                        continue

                    normalized = MarketPrice(
                        market_id=condition_id,
                        platform=Platform.POLYMARKET,
                        game_id=game_id,
                        market_title=meta.get("title", polled.market_title),
                        yes_bid=polled.yes_bid,
                        yes_ask=polled.yes_ask,
                        volume=meta.get("volume", polled.volume),
                        liquidity=polled.liquidity,
                        status=polled.status,
                        timestamp=polled.timestamp,
                        last_trade_price=polled.last_trade_price,
                    )
                    await self.redis.publish_market_price(game_id, normalized)
                    self._prices_published += 1
                    self._last_price_time = datetime.utcnow()
                    if self._prices_published == 1:
                        logger.info(
                            f"REST poll publishing is working (example): game={game_id} "
                            f"market={condition_id} bid={normalized.yes_bid:.3f} ask={normalized.yes_ask:.3f}"
                        )

            except Exception as e:
                logger.warning(f"REST poll loop error: {e}")

            await asyncio.sleep(self._poll_interval_s)

    async def _health_check_loop(self):
        """Periodic health checks."""
        while self._running:
            try:
                # Verify VPN is still working
                await self._verify_vpn()

                # Check WebSocket connection
                if self.subscribed_tokens and not self.poly_client.ws_connected:
                    logger.warning("Polymarket WS disconnected, reconnecting...")
                    # The stream_prices loop will handle reconnection

                # Check for stale data
                if self._last_price_time:
                    staleness = (datetime.utcnow() - self._last_price_time).total_seconds()
                    if staleness > 120 and self.subscribed_tokens:
                        logger.warning(f"Price data stale ({staleness:.0f}s), may need reconnection")

                self._health_ok = True

                # Publish health status
                await self.redis.publish(Channel.SYSTEM_ALERTS.value, {
                    "type": "POLYMARKET_MONITOR_HEALTH",
                    "service": "polymarket_monitor",
                    "healthy": True,
                    "subscribed_markets": len(self.subscribed_conditions),
                    "prices_published": self._prices_published,
                    "timestamp": datetime.utcnow().isoformat(),
                })

            except Exception as e:
                logger.error(f"Health check failed: {e}")
                self._health_ok = False

                # Publish alert
                await self.redis.publish(Channel.SYSTEM_ALERTS.value, {
                    "type": "POLYMARKET_MONITOR_UNHEALTHY",
                    "service": "polymarket_monitor",
                    "error": str(e),
                    "timestamp": datetime.utcnow().isoformat(),
                })

            await asyncio.sleep(60)


async def main():
    """Entry point."""
    # Configure logging
    import sys
    logger.remove()
    logger.add(
        sys.stderr,
        format="<green>{time:YYYY-MM-DD HH:mm:ss}</green> | <level>{level: <8}</level> | <cyan>{name}</cyan>:<cyan>{function}</cyan>:<cyan>{line}</cyan> - <level>{message}</level>",
        level=os.environ.get("LOG_LEVEL", "INFO"),
    )

    monitor = PolymarketMonitor()

    try:
        await monitor.start()
    except KeyboardInterrupt:
        logger.info("Interrupted by user")
    except Exception as e:
        logger.error(f"Fatal error: {e}")
        raise
    finally:
        await monitor.stop()


if __name__ == "__main__":
    asyncio.run(main())
