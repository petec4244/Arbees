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

import json
import time

import httpx
from loguru import logger

from arbees_shared.messaging.redis_bus import RedisBus, Channel, deserialize
from arbees_shared.models.market import MarketPrice, MarketStatus, Platform
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from markets.polymarket.hybrid_client import HybridPolymarketClient

# Optional ZMQ support for low-latency messaging
try:
    import zmq
    import zmq.asyncio
    ZMQ_AVAILABLE = True
except ImportError:
    ZMQ_AVAILABLE = False
    logger.warning("pyzmq not installed - ZMQ publishing disabled")


class PolymarketMonitor:
    """
    Monitors Polymarket markets via VPN and publishes prices to Redis.

    Designed to run in Docker container with network_mode: "service:vpn"
    
    IMPORTANT: Polymarket moneyline markets have TWO tokens (one per team).
    We subscribe to BOTH and track which token corresponds to which team,
    so downstream consumers can match signal.team to the correct contract.
    """

    def __init__(self):
        self.redis = RedisBus()
        self.poly_client = HybridPolymarketClient()

        # Subscription tracking
        self.subscribed_tokens: set[str] = set()

        # SIMPLIFIED: token_id -> {condition_id, game_id, market_type}
        # This is all we need to route incoming prices to the right market
        self._token_to_market: dict[str, dict] = {}

        # Active assignment per (game_id, market_type) -> condition_id
        # Used to prevent publishing stale markets after discovery corrections.
        self._active_by_game_type: dict[tuple[str, str], str] = {}

        # State
        self._running = False
        self._health_ok = False
        self._last_price_time: Optional[datetime] = None
        self._prices_published = 0
        self._poll_interval_s = float(os.environ.get("POLYMARKET_POLL_INTERVAL_SECONDS", "2.0"))

        # Heartbeat publisher for health monitoring
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

        # ZMQ publisher for low-latency messaging (HOT PATH - always enabled)
        # ZMQ is the primary transport for price data; Redis is only for slow-path consumers
        self._zmq_context: Optional["zmq.asyncio.Context"] = None
        self._zmq_pub: Optional["zmq.asyncio.Socket"] = None
        self._zmq_seq = 0
        self._zmq_pub_port = int(os.environ.get("ZMQ_PUB_PORT", "5556"))

        # Redis publishing is now optional (only for backward compatibility)
        transport_mode = os.environ.get("ZMQ_TRANSPORT_MODE", "zmq_only").lower()
        self._redis_publish_prices = transport_mode in ("redis_only", "both")

    # region agent log (helper)
    def _agent_dbg(self, hypothesisId: str, location: str, message: str, data: dict) -> None:
        """Write a single NDJSON debug line to the host-mounted .cursor/debug.log (DEBUG MODE ONLY)."""
        try:
            payload = {
                "sessionId": "debug-session",
                "runId": os.environ.get("DEBUG_RUN_ID", "pre-fix"),
                "hypothesisId": hypothesisId,
                "location": location,
                "message": message,
                "data": data,
                "timestamp": int(time.time() * 1000),
            }
            with open("/app/.cursor/debug.log", "a", encoding="utf-8") as f:
                f.write(json.dumps(payload, default=str) + "\n")
        except Exception:
            pass
    # endregion

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

        # Initialize ZMQ publisher (REQUIRED - ZMQ is the primary hot path)
        if not ZMQ_AVAILABLE:
            raise RuntimeError(
                "pyzmq is required for PolymarketMonitor. "
                "Install with: pip install pyzmq"
            )

        try:
            self._zmq_context = zmq.asyncio.Context()
            self._zmq_pub = self._zmq_context.socket(zmq.PUB)
            self._zmq_pub.bind(f"tcp://*:{self._zmq_pub_port}")
            logger.info(f"ZMQ PUB socket bound to port {self._zmq_pub_port} (primary hot path)")
        except Exception as e:
            logger.error(f"Failed to initialize ZMQ: {e}")
            raise RuntimeError(f"ZMQ initialization failed: {e}")

        # Setup signal handlers
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, lambda: asyncio.create_task(self.stop()))
            except NotImplementedError:
                pass  # Windows

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="polymarket_monitor",
            instance_id=os.environ.get("HOSTNAME", "polymarket-monitor-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "vpn_ok": self._health_ok,
            "ws_ok": False,  # Will be updated when WS connects
        })

        logger.info("PolymarketMonitor started successfully")

        # Run concurrent tasks
        # Note: REST poll loop removed - WebSocket streaming is primary transport
        await asyncio.gather(
            self._assignment_listener(),
            self._price_streaming_loop(),
            self._health_check_loop(),
            return_exceptions=True,
        )

    async def stop(self):
        """Graceful shutdown."""
        logger.info("Stopping PolymarketMonitor...")
        self._running = False

        # Stop heartbeat publisher
        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        # Close ZMQ socket
        if self._zmq_pub:
            self._zmq_pub.close()
        if self._zmq_context:
            self._zmq_context.term()

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
        logger.info(f"Starting assignment listener on channel: {Channel.MARKET_ASSIGNMENTS.value}")

        # Subscribe to market assignments channel
        await self.redis.subscribe(
            Channel.MARKET_ASSIGNMENTS.value,
            self._handle_assignment,
        )

        logger.info("Assignment listener subscribed, waiting for market assignments from orchestrator...")

        # Start the listener
        await self.redis.start_listening()

    async def _handle_assignment(self, data: dict):
        """Handle a market assignment message from Orchestrator."""
        msg_type = data.get("type")

        if msg_type != "polymarket_assign":
            return

        # Support both "game_id" (sports) and "event_id" (crypto/multi-market)
        game_id = data.get("game_id") or data.get("event_id")
        sport = data.get("sport")
        market_type = data.get("market_type", "moneyline")  # crypto, economics, politics, or moneyline
        markets = data.get("markets", [])

        if not game_id or not markets:
            logger.warning(f"Invalid assignment - missing game_id/event_id or markets: {data}")
            return

        logger.info(f"Received assignment: game={game_id}, sport={sport}, market_type={market_type}, markets={len(markets)}")

        for market_info in markets:
            condition_id = market_info.get("condition_id")
            market_type = market_info.get("market_type", "moneyline")

            if not condition_id:
                continue

            # Update active mapping even if we're already subscribed; orchestrator may be correcting IDs.
            self._active_by_game_type[(str(game_id), str(market_type))] = str(condition_id)

            if condition_id in self.subscribed_conditions:
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
        home_team: str = "",
        away_team: str = "",
    ):
        """
        Subscribe to a Polymarket market via WebSocket.

        Simplified: Only fetch token IDs, don't parse outcomes or store metadata.
        Team names are inferred from token prices themselves.
        """
        # Fetch market to get token IDs
        market = await self.poly_client.get_market(condition_id)
        if not market:
            logger.warning(f"Market not found: {condition_id}")
            return

        # Get token IDs (may be in clobTokenIds or resolve from market)
        tokens_raw = market.get("clobTokenIds", "[]")

        if isinstance(tokens_raw, str):
            try:
                token_ids = json.loads(tokens_raw)
            except:
                token_ids = []
        else:
            token_ids = tokens_raw or []

        # If no tokens found, try to resolve single token
        if not token_ids:
            logger.debug(f"No clobTokenIds for {condition_id}, attempting resolve_yes_token_id")
            token_id = await self.poly_client.resolve_yes_token_id(market)
            token_ids = [token_id] if token_id else []

        if not token_ids:
            logger.warning(f"Could not determine token IDs for market {condition_id}")
            return

        # Subscribe to all tokens
        for token_id in token_ids:
            if token_id in self.subscribed_tokens:
                logger.debug(f"Already subscribed to token {token_id[:16]}...")
                continue

            # Minimal metadata: just map token to market location
            self._token_to_market[token_id] = {
                "condition_id": condition_id,
                "game_id": game_id,
                "market_type": market_type,
            }

            self.subscribed_tokens.add(token_id)

            # Subscribe via client metadata for tracking
            await self.poly_client.subscribe_with_metadata([{
                "token_id": token_id,
                "condition_id": condition_id,
                "game_id": game_id,
                "market_type": market_type,
            }])

            logger.info(f"Subscribed to token {token_id[:16]}... (game={game_id})")

        logger.info(f"Subscribed to Polymarket: {condition_id[:16]}... ({market_type}) with {len(token_ids)} tokens")

    async def _price_streaming_loop(self):
        """Stream prices from Polymarket WebSocket and publish to Redis."""
        logger.info("Starting price streaming loop...")
        idle_log_counter = 0

        while self._running:
            try:
                # Wait for subscriptions
                if not self.subscribed_tokens:
                    idle_log_counter += 1
                    # Log every 30 iterations (60 seconds) when idle
                    if idle_log_counter % 30 == 1:
                        logger.info(f"Price streaming loop idle - no subscribed tokens (waiting for market assignments)")
                    await asyncio.sleep(2)
                    continue

                idle_log_counter = 0  # Reset when we have tokens

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
        """
        Handle incoming price update and publish to ZMQ/Redis.

        Simplified: Minimal lookup, direct market routing.
        """
        token_id = price.market_id
        market_info = self._token_to_market.get(token_id)

        if not market_info:
            logger.debug(f"Unknown token_id: {token_id[:16]}...")
            return

        condition_id = market_info["condition_id"]
        game_id = market_info["game_id"]
        market_type = market_info["market_type"]

        # Drop stale markets: only publish the currently active assignment.
        active = self._active_by_game_type.get((str(game_id), str(market_type)))
        if active and active != condition_id:
            return

        # Minimal normalization: use price as-is, with market routing info
        normalized_price = MarketPrice(
            market_id=condition_id,
            platform=Platform.POLYMARKET,
            game_id=game_id,
            market_title=price.market_title,
            contract_team=None,  # Simplified: don't track team name
            yes_bid=price.yes_bid,
            yes_ask=price.yes_ask,
            yes_bid_size=price.yes_bid_size,
            yes_ask_size=price.yes_ask_size,
            volume=price.volume,
            liquidity=price.liquidity,
            status=price.status,
            timestamp=price.timestamp,
            last_trade_price=price.last_trade_price,
        )

        # region agent log
        if normalized_price.yes_ask >= 0.99 or normalized_price.yes_bid <= 0.01:
            self._agent_dbg(
                "H1",
                "services/polymarket_monitor/monitor.py:_handle_price_update",
                "published_extreme_price",
                {
                    "game_id": game_id,
                    "condition_id": condition_id,
                    "contract_team": contract_team,
                    "yes_bid": float(normalized_price.yes_bid),
                    "yes_ask": float(normalized_price.yes_ask),
                    "mid": float(normalized_price.mid_price),
                    "market_title": normalized_price.market_title,
                    "source": "ws",
                },
            )
        # endregion

        # PRIMARY: Publish to ZMQ (hot path - always)
        await self._publish_zmq_price(condition_id, game_id, normalized_price)

        # SECONDARY: Optionally publish to Redis for backward compatibility
        if self._redis_publish_prices:
            await self.redis.publish_market_price(game_id, normalized_price)

        self._prices_published += 1
        self._last_price_time = datetime.utcnow()

        logger.debug(
            f"Published Polymarket price: {condition_id[:12]}... team='{contract_team}' "
            f"bid={normalized_price.yes_bid:.3f} ask={normalized_price.yes_ask:.3f} "
            f"game={game_id} (zmq=yes, redis={self._redis_publish_prices})"
        )

    async def _publish_zmq_price(self, condition_id: str, game_id: str, price: MarketPrice):
        """Publish price to ZMQ PUB socket (primary hot path)."""
        try:
            topic = f"prices.poly.{condition_id}".encode()
            envelope = {
                "seq": self._zmq_seq,
                "timestamp_ms": int(time.time() * 1000),
                "source": "polymarket_monitor",
                "payload": {
                    "market_id": price.market_id,
                    "platform": "polymarket",
                    "game_id": game_id,
                    "contract_team": price.contract_team,
                    "yes_bid": price.yes_bid,
                    "yes_ask": price.yes_ask,
                    "mid_price": price.mid_price,
                    "yes_bid_size": price.yes_bid_size,
                    "yes_ask_size": price.yes_ask_size,
                    "volume": price.volume,
                    "liquidity": price.liquidity,
                    "timestamp": price.timestamp.isoformat() if price.timestamp else None,
                },
            }
            self._zmq_seq += 1
            await self._zmq_pub.send_multipart([topic, json.dumps(envelope).encode()])
        except Exception as e:
            logger.warning(f"Failed to publish to ZMQ: {e}")

    async def _rest_poll_loop(self) -> None:
        """
        Fallback poller to ensure we publish prices even if WS is quiet/flaky.

        For moneyline markets, polls BOTH team tokens and publishes separate
        prices for each team.
        """
        logger.info(f"Starting REST poll loop (interval={self._poll_interval_s}s)...")
        idle_log_counter = 0

        while self._running:
            try:
                if not self._active_by_game_type:
                    idle_log_counter += 1
                    # Log every 30 iterations (~60 seconds at 2s interval) when idle
                    if idle_log_counter % 30 == 1:
                        logger.info(f"REST poll loop idle - no active market assignments (subscribed_conditions={len(self.subscribed_conditions)})")
                    await asyncio.sleep(self._poll_interval_s)
                    continue

                idle_log_counter = 0  # Reset when we have active assignments

                active_items = list(self._active_by_game_type.items())
                for (g_id, m_type), condition_id in active_items:
                    meta = self._market_metadata.get(condition_id, {})
                    game_id = meta.get("game_id") or g_id
                    if not game_id:
                        continue

                    # Get all tokens for this condition (both teams for moneyline)
                    token_ids = self._condition_to_tokens.get(condition_id, [])
                    
                    if not token_ids:
                        # Fallback: poll by condition_id (single price, no team info)
                        try:
                            polled = await self.poly_client.get_market_price(condition_id)
                        except Exception as e:
                            logger.warning(f"REST poll failed for {condition_id}: {e}")
                            continue

                        if polled:
                            normalized = MarketPrice(
                                market_id=condition_id,
                                platform=Platform.POLYMARKET,
                                game_id=game_id,
                                market_title=meta.get("title", polled.market_title),
                                contract_team=None,  # Unknown team
                                yes_bid=polled.yes_bid,
                                yes_ask=polled.yes_ask,
                                yes_bid_size=float(getattr(polled, "yes_bid_size", 0.0) or 0.0),
                                yes_ask_size=float(getattr(polled, "yes_ask_size", 0.0) or 0.0),
                                volume=meta.get("volume", polled.volume),
                                liquidity=polled.liquidity,
                                status=polled.status,
                                timestamp=polled.timestamp,
                                last_trade_price=polled.last_trade_price,
                            )
                            # PRIMARY: ZMQ (always)
                            await self._publish_zmq_price(condition_id, game_id, normalized)
                            # SECONDARY: Redis (optional)
                            if self._redis_publish_prices:
                                await self.redis.publish_market_price(game_id, normalized)
                            self._prices_published += 1
                            self._last_price_time = datetime.utcnow()
                        continue
                    
                    # Poll the condition_id once and extract prices for each outcome
                    try:
                        market_data = await self.poly_client.get_market(condition_id)
                    except Exception as e:
                        logger.warning(f"REST poll failed for condition {condition_id[:16]}...: {e}")
                        continue
                    
                    if not market_data:
                        continue
                    
                    # Parse outcome prices from market data
                    # Format: outcomePrices: "[\"0.7\", \"0.3\"]" or list
                    import json as json_mod
                    outcomes = meta.get("outcomes", [])
                    prices_raw = market_data.get("outcomePrices", "[]")
                    
                    if isinstance(prices_raw, str):
                        try:
                            prices = json_mod.loads(prices_raw)
                        except:
                            prices = []
                    else:
                        prices = prices_raw or []
                    
                    if len(outcomes) != len(prices):
                        logger.debug(f"Mismatched outcomes/prices for {condition_id}: {len(outcomes)} vs {len(prices)}")
                        continue
                    
                    # Publish a price for each outcome (team)
                    for i, (outcome, price_str) in enumerate(zip(outcomes, prices)):
                        try:
                            mid_price = float(price_str)
                        except (ValueError, TypeError):
                            continue
                        
                        # Estimate bid/ask from mid price (typical 2% spread)
                        spread = 0.01
                        yes_bid = max(0.0, mid_price - spread)
                        yes_ask = min(1.0, mid_price + spread)
                        
                        title = meta.get("title", "")
                        if outcome and outcome not in title:
                            title = f"{title} [{outcome}]"
                        
                        normalized = MarketPrice(
                            market_id=condition_id,
                            platform=Platform.POLYMARKET,
                            game_id=game_id,
                            market_title=title,
                            contract_team=outcome,
                            yes_bid=yes_bid,
                            yes_ask=yes_ask,
                            yes_bid_size=0.0,
                            yes_ask_size=0.0,
                            volume=meta.get("volume", 0),
                            liquidity=float(market_data.get("liquidity", 0) or 0),
                            status=MarketStatus.OPEN,
                            timestamp=datetime.utcnow(),
                            last_trade_price=float(market_data.get("lastTradePrice", 0) or 0) if market_data.get("lastTradePrice") else None,
                        )
                        # PRIMARY: ZMQ (always)
                        await self._publish_zmq_price(condition_id, game_id, normalized)
                        # SECONDARY: Redis (optional)
                        if self._redis_publish_prices:
                            await self.redis.publish_market_price(game_id, normalized)
                        self._prices_published += 1
                        self._last_price_time = datetime.utcnow()
                        
                        if self._prices_published <= 4:
                            logger.info(
                                f"REST poll publishing: game={game_id} team='{outcome}' "
                                f"mid={mid_price:.3f}"
                            )

            except Exception as e:
                logger.warning(f"REST poll loop error: {e}")

            await asyncio.sleep(self._poll_interval_s)

    async def _health_check_loop(self):
        """Periodic health checks (3-minute interval)."""
        health_check_interval = 180  # 3 minutes instead of 60 seconds
        while self._running:
            try:
                # Skip VPN verification from health check loop (too slow)
                # VPN is verified on startup; if it fails, the service won't start
                # Periodic re-verification can be added later if needed

                # Check WebSocket connection
                ws_ok = self.poly_client.ws_connected
                if self.subscribed_tokens and not ws_ok:
                    logger.warning("Polymarket WS disconnected, reconnecting...")
                    # The stream_prices loop will handle reconnection

                # Check for stale data
                staleness_s = 0.0
                if self._last_price_time:
                    staleness_s = (datetime.utcnow() - self._last_price_time).total_seconds()
                    if staleness_s > 120 and self.subscribed_tokens:
                        logger.warning(f"Price data stale ({staleness_s:.0f}s), may need reconnection")

                self._health_ok = True

                # Update heartbeat publisher
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.update_checks({
                        "redis_ok": True,
                        "vpn_ok": self._health_ok,
                        "ws_ok": ws_ok,
                    })
                    self._heartbeat_publisher.update_metrics({
                        "subscribed_markets": float(len(self.subscribed_conditions)),
                        "prices_published": float(self._prices_published),
                        "last_price_age_s": staleness_s,
                    })
                    if self._health_ok and ws_ok:
                        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
                    elif self._health_ok:
                        self._heartbeat_publisher.set_status(ServiceStatus.DEGRADED)
                    else:
                        self._heartbeat_publisher.set_status(ServiceStatus.UNHEALTHY)

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

                # Update heartbeat publisher
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.set_unhealthy(str(e))

                # Publish alert
                await self.redis.publish(Channel.SYSTEM_ALERTS.value, {
                    "type": "POLYMARKET_MONITOR_UNHEALTHY",
                    "service": "polymarket_monitor",
                    "error": str(e),
                    "timestamp": datetime.utcnow().isoformat(),
                })

            await asyncio.sleep(health_check_interval)


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
