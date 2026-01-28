"""
Dedicated Kalshi price monitor (no VPN needed - direct connection).

This service:
1. Subscribes to market assignments from Orchestrator via Redis
2. Connects to Kalshi WebSocket for real-time prices
3. Publishes MarketPrice updates to Redis for GameShards to consume

Key design decisions:
- No VPN needed (Kalshi API is accessible from US)
- Uses existing KalshiWebSocketClient for WebSocket streaming
- Publishes to game:{game_id}:price channel (same format as Polymarket)
- Kalshi markets are simpler: one ticker per market (not multiple tokens like Polymarket)
"""

import asyncio
import json
import os
import signal
import time
from datetime import datetime
from typing import Optional

from loguru import logger

from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.market import MarketPrice, MarketStatus, Platform
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from markets.kalshi.websocket.ws_client import KalshiWebSocketClient

# Optional ZMQ support for low-latency messaging
try:
    import zmq
    import zmq.asyncio
    ZMQ_AVAILABLE = True
except ImportError:
    ZMQ_AVAILABLE = False
    logger.warning("pyzmq not installed - ZMQ publishing disabled")


def parse_kalshi_ticker(ticker: str) -> Optional[dict]:
    """
    Parse Kalshi ticker to extract teams and matchup info.

    Ticker format: KX{SPORT}GAME-{DATE}{AWAY}{HOME}-{CONTRACT_TEAM}

    The middle segment contains both team abbreviations concatenated:
        - PORWAS = POR (Portland) vs WAS (Washington)
        - NSHBOS = NSH (Nashville) vs BOS (Boston)
        - BUFTOR = BUF (Buffalo) vs TOR (Toronto)

    The final segment is the team this contract represents.

    Examples:
        KXNHLGAME-26JAN27NSHBOS-BOS -> {contract_team: BOS, matchup: NSHBOS, other_team: NSH}
        KXNBAGAME-26JAN27PORWAS-POR -> {contract_team: POR, matchup: PORWAS, other_team: WAS}

    Returns:
        Dict with contract_team, matchup, other_team, base_ticker if valid, None otherwise.
    """
    if not ticker:
        return None

    parts = ticker.split("-")
    if len(parts) < 3:
        return None

    # Last segment is the contract team abbreviation
    contract_team = parts[-1].upper()

    # Basic validation: team abbreviations are typically 2-4 letters
    if not (2 <= len(contract_team) <= 4 and contract_team.isalpha()):
        return None

    # Extract matchup segment (e.g., "26JAN27PORWAS" -> get last 6-8 chars for teams)
    # The date portion is like "26JAN27" (7 chars), rest is team matchup
    date_matchup = parts[1] if len(parts) > 1 else ""
    if len(date_matchup) <= 7:
        return None

    # Matchup is after the date (e.g., "PORWAS" from "26JAN27PORWAS")
    matchup = date_matchup[7:].upper()

    # Validate: contract team must appear in the matchup segment
    if contract_team not in matchup:
        logger.warning(
            f"Contract team '{contract_team}' not found in matchup '{matchup}' "
            f"for ticker {ticker}"
        )
        return None

    # Extract the other team by removing contract_team from matchup
    # Team abbrevs are 2-4 chars, and matchup is {AWAY}{HOME}
    other_team = matchup.replace(contract_team, "", 1)

    # Validate other team
    if not (2 <= len(other_team) <= 4 and other_team.isalpha()):
        logger.warning(
            f"Could not extract valid other team from matchup '{matchup}' "
            f"(contract_team={contract_team}) for ticker {ticker}"
        )
        return None

    # Base ticker is everything except the final team segment
    base_ticker = "-".join(parts[:-1])

    return {
        "contract_team": contract_team,
        "other_team": other_team,
        "matchup": matchup,
        "base_ticker": base_ticker,
    }


def extract_team_from_ticker(ticker: str) -> Optional[str]:
    """
    Extract team abbreviation from Kalshi ticker format.
    Convenience wrapper around parse_kalshi_ticker.
    """
    parsed = parse_kalshi_ticker(ticker)
    return parsed["contract_team"] if parsed else None


def get_complementary_ticker(ticker: str) -> Optional[str]:
    """
    Get the complementary ticker for a Kalshi moneyline market.

    For moneyline markets, Kalshi has TWO contracts per game (one per team).
    Given one ticker, this returns the ticker for the other team.

    Examples:
        KXNHLGAME-26JAN27NSHBOS-BOS -> KXNHLGAME-26JAN27NSHBOS-NSH
        KXNBAGAME-26JAN27PORWAS-POR -> KXNBAGAME-26JAN27PORWAS-WAS

    Returns:
        Complementary ticker if valid, None otherwise.
    """
    parsed = parse_kalshi_ticker(ticker)
    if not parsed:
        return None

    return f"{parsed['base_ticker']}-{parsed['other_team']}"


class KalshiMonitor:
    """
    Monitors Kalshi markets and publishes prices to Redis.

    Unlike Polymarket, Kalshi markets are simpler:
    - One ticker per market (e.g., "KXMLB-12345" for a moneyline market)
    - Each ticker represents one team's YES contract
    - For moneyline markets, we need TWO tickers (one per team)
    """

    def __init__(self):
        self.redis = RedisBus()
        self.kalshi_ws = KalshiWebSocketClient()

        # Subscription tracking
        self.subscribed_tickers: set[str] = set()

        # ticker -> {game_id, market_type, team_name}
        self._ticker_to_info: dict[str, dict] = {}

        # Market metadata (ticker -> {game_id, market_type, title, team})
        self._market_metadata: dict[str, dict] = {}

        # Active assignments per (game_id, market_type) -> set of tickers
        # For moneyline markets, this includes BOTH team contracts
        # Used to prevent publishing stale markets after discovery corrections.
        self._active_by_game_type: dict[tuple[str, str], set[str]] = {}

        # State
        self._running = False
        self._health_ok = False
        self._last_price_time: Optional[datetime] = None
        self._prices_published = 0
        self._poll_interval_s = float(os.environ.get("KALSHI_POLL_INTERVAL_SECONDS", "2.0"))

        # Heartbeat publisher for health monitoring
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

        # ZMQ publisher for low-latency messaging (hot path)
        self._zmq_enabled = os.environ.get("ZMQ_ENABLED", "false").lower() in ("true", "1", "yes")
        self._zmq_context: Optional["zmq.asyncio.Context"] = None
        self._zmq_pub: Optional["zmq.asyncio.Socket"] = None
        self._zmq_seq = 0
        self._zmq_pub_port = int(os.environ.get("ZMQ_PUB_PORT", "5555"))

    async def start(self):
        """Initialize connections and start monitoring."""
        logger.info("Starting KalshiMonitor...")

        # Connect to Redis (retry for DNS/bootstrap delays)
        max_retries = int(os.environ.get("REDIS_CONNECT_RETRIES", "10"))
        retry_delay = float(os.environ.get("REDIS_CONNECT_DELAY_SECS", "2.0"))
        for attempt in range(1, max_retries + 1):
            try:
                await self.redis.connect()
                logger.info("Redis connected")
                break
            except Exception as e:
                if attempt >= max_retries:
                    logger.error(f"Failed to connect to Redis after {attempt} attempts: {e}")
                    raise
                logger.warning(
                    f"Redis connect failed (attempt {attempt}/{max_retries}): {e}"
                )
                await asyncio.sleep(retry_delay)

        # Connect to Kalshi WebSocket
        try:
            await self.kalshi_ws.connect()
            logger.info("Kalshi WebSocket connected")
        except Exception as e:
            logger.error(f"Failed to connect to Kalshi WebSocket: {e}")
            logger.error("Check KALSHI_API_KEY and KALSHI_PRIVATE_KEY environment variables")
            raise

        self._running = True
        self._health_ok = True

        # Initialize ZMQ publisher if enabled
        if self._zmq_enabled and ZMQ_AVAILABLE:
            try:
                self._zmq_context = zmq.asyncio.Context()
                self._zmq_pub = self._zmq_context.socket(zmq.PUB)
                self._zmq_pub.bind(f"tcp://*:{self._zmq_pub_port}")
                logger.info(f"ZMQ PUB socket bound to port {self._zmq_pub_port}")
            except Exception as e:
                logger.error(f"Failed to initialize ZMQ: {e}")
                self._zmq_enabled = False
        elif self._zmq_enabled and not ZMQ_AVAILABLE:
            logger.warning("ZMQ_ENABLED=true but pyzmq not installed")
            self._zmq_enabled = False

        # Setup signal handlers
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, lambda: asyncio.create_task(self.stop()))
            except NotImplementedError:
                pass  # Windows

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="kalshi_monitor",
            instance_id=os.environ.get("HOSTNAME", "kalshi-monitor-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "ws_ok": True,
        })

        logger.info("KalshiMonitor started successfully")

        # Run concurrent tasks
        await asyncio.gather(
            self._assignment_listener(),
            self._price_streaming_loop(),
            self._health_check_loop(),
            return_exceptions=True,
        )

    async def stop(self):
        """Graceful shutdown."""
        logger.info("Stopping KalshiMonitor...")
        self._running = False

        # Stop heartbeat publisher
        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        # Close ZMQ socket
        if self._zmq_pub:
            self._zmq_pub.close()
        if self._zmq_context:
            self._zmq_context.term()

        await self.kalshi_ws.disconnect()
        await self.redis.disconnect()

        logger.info(f"KalshiMonitor stopped. Published {self._prices_published} prices.")

    async def _assignment_listener(self):
        """Subscribe to market assignment messages from Orchestrator."""
        logger.info("Starting assignment listener...")

        # Subscribe to market assignments channel
        await self.redis.subscribe(
            Channel.MARKET_ASSIGNMENTS.value,
            self._handle_assignment,
        )

        # Start the listener (runs in background)
        await self.redis.start_listening()
        
        # Keep this task alive while running
        while self._running:
            await asyncio.sleep(1)

    async def _handle_assignment(self, data: dict):
        """Handle a market assignment message from Orchestrator."""
        msg_type = data.get("type")

        if msg_type != "kalshi_assign":
            return

        game_id = data.get("game_id")
        sport = data.get("sport")
        markets = data.get("markets", [])

        if not game_id or not markets:
            return

        logger.info(f"Received Kalshi assignment: game={game_id}, sport={sport}, markets={len(markets)}")

        for market_info in markets:
            ticker = market_info.get("ticker") or market_info.get("market_id")
            market_type = market_info.get("market_type", "moneyline")
            team_name = market_info.get("team_name")  # Optional: which team this ticker represents

            if not ticker:
                continue

            # Update active mapping - use set to track both team contracts for moneyline
            key = (str(game_id), str(market_type))
            if key not in self._active_by_game_type:
                self._active_by_game_type[key] = set()
            self._active_by_game_type[key].add(str(ticker))

            # Subscribe to this ticker
            if ticker not in self.subscribed_tickers:
                try:
                    await self._subscribe_to_market(ticker, game_id, market_type, team_name)
                except Exception as e:
                    logger.error(f"Failed to subscribe to {ticker}: {e}")

            # For moneyline markets, also subscribe to the complementary ticker (other team)
            # Kalshi has TWO contracts per game - one for each team
            if market_type == "moneyline":
                comp_ticker = get_complementary_ticker(ticker)
                if comp_ticker:
                    # Add complementary ticker to active set too
                    self._active_by_game_type[key].add(comp_ticker)

                    if comp_ticker not in self.subscribed_tickers:
                        try:
                            await self._subscribe_to_market(comp_ticker, game_id, market_type, None)
                        except Exception as e:
                            logger.error(f"Failed to subscribe to complementary ticker {comp_ticker}: {e}")

    async def _subscribe_to_market(
        self,
        ticker: str,
        game_id: str,
        market_type: str,
        team_name: Optional[str] = None,
    ):
        """
        Subscribe to a Kalshi market via WebSocket.

        For moneyline markets, we typically need TWO tickers (one per team).
        The orchestrator should send both tickers in separate assignment messages.
        """
        # Extract team from ticker if not provided
        if not team_name:
            team_name = extract_team_from_ticker(ticker)

        # Store metadata
        self._market_metadata[ticker] = {
            "game_id": game_id,
            "market_type": market_type,
            "team_name": team_name,
            "title": ticker,  # Will be updated if we fetch market details
        }

        # Store ticker -> info mapping
        self._ticker_to_info[ticker] = {
            "game_id": game_id,
            "market_type": market_type,
            "team_name": team_name,
        }

        # Subscribe via WebSocket
        try:
            await self.kalshi_ws.subscribe([ticker])
            self.subscribed_tickers.add(ticker)
            logger.info(f"Subscribed to Kalshi: {ticker} (game={game_id}, team={team_name or 'unknown'})")
        except Exception as e:
            logger.error(f"Failed to subscribe to {ticker}: {e}")
            raise

    async def _price_streaming_loop(self):
        """Stream prices from Kalshi WebSocket and publish to Redis."""
        logger.info("Starting price streaming loop...")

        while self._running:
            try:
                # Wait for subscriptions
                if not self.subscribed_tickers:
                    await asyncio.sleep(2)
                    continue

                # Check connection before streaming
                if not self.kalshi_ws.is_connected:
                    logger.warning("Kalshi WebSocket not connected, waiting...")
                    await asyncio.sleep(5)
                    continue

                logger.debug(f"Streaming prices for {len(self.subscribed_tickers)} Kalshi markets")

                async for price in self.kalshi_ws.stream_prices():
                    if not self._running:
                        break

                    # Double-check connection in case it dropped during streaming
                    if not self.kalshi_ws.is_connected:
                        logger.warning("WebSocket disconnected during streaming")
                        break

                    await self._handle_price_update(price)

            except RuntimeError as e:
                if "Not connected" in str(e):
                    logger.warning("WebSocket not connected, waiting to reconnect...")
                    self._health_ok = False
                    await asyncio.sleep(5)
                else:
                    logger.error(f"Runtime error in price streaming: {e}")
                    self._health_ok = False
                    await asyncio.sleep(5)
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Price streaming error: {e}", exc_info=True)
                self._health_ok = False
                await asyncio.sleep(5)

    async def _handle_price_update(self, price: MarketPrice):
        """
        Handle incoming price update and publish to Redis.
        
        Kalshi markets are simpler than Polymarket:
        - Each ticker represents one team's YES contract
        - We include contract_team in the published MarketPrice so downstream
          consumers can match signal.team to the correct contract.
        """
        ticker = price.market_id
        ticker_info = self._ticker_to_info.get(ticker)

        if not ticker_info:
            logger.debug(f"Unknown ticker: {ticker}")
            return

        game_id = ticker_info["game_id"]
        market_type = ticker_info["market_type"]
        team_name = ticker_info.get("team_name")

        # Drop stale markets: only publish if ticker is in the active set for this (game_id, market_type)
        active_set = self._active_by_game_type.get((str(game_id), str(market_type)))
        if active_set and ticker not in active_set:
            return

        # Get metadata
        meta = self._market_metadata.get(ticker, {})
        title = meta.get("title", price.market_title)
        if team_name and team_name not in title:
            title = f"{title} [{team_name}]"

        normalized_price = MarketPrice(
            market_id=ticker,
            platform=Platform.KALSHI,
            game_id=game_id,
            market_title=title,
            contract_team=team_name,  # Which team's YES contract
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

        # Publish to per-game price channel
        await self.redis.publish_market_price(game_id, normalized_price)

        self._prices_published += 1
        self._last_price_time = datetime.utcnow()

        # Publish to ZMQ for low-latency consumers (hot path)
        if self._zmq_enabled and self._zmq_pub:
            await self._publish_zmq_price(ticker, game_id, normalized_price)

        logger.debug(
            f"Published Kalshi price: {ticker[:12]}... team='{team_name}' "
            f"bid={normalized_price.yes_bid:.3f} ask={normalized_price.yes_ask:.3f} "
            f"game={game_id}"
        )

    async def _publish_zmq_price(self, ticker: str, game_id: str, price: MarketPrice):
        """Publish price to ZMQ PUB socket for low-latency consumers."""
        if not self._zmq_pub:
            return

        try:
            topic = f"prices.kalshi.{ticker}".encode()
            envelope = {
                "seq": self._zmq_seq,
                "timestamp_ms": int(time.time() * 1000),
                "source": "kalshi_monitor",
                "payload": {
                    "market_id": price.market_id,
                    "platform": "kalshi",
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

    async def _health_check_loop(self):
        """Periodic health checks."""
        while self._running:
            try:
                # Check WebSocket connection
                ws_ok = self.kalshi_ws.is_connected

                # Check for stale data
                staleness_s = 0.0
                if self._last_price_time:
                    staleness_s = (datetime.utcnow() - self._last_price_time).total_seconds()
                    if staleness_s > 120 and self.subscribed_tickers:
                        logger.warning(f"Price data stale ({staleness_s:.0f}s), may need reconnection")

                self._health_ok = True

                # Update heartbeat publisher
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.update_checks({
                        "redis_ok": True,
                        "ws_ok": ws_ok,
                    })
                    self._heartbeat_publisher.update_metrics({
                        "subscribed_markets": float(len(self.subscribed_tickers)),
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
                    "type": "KALSHI_MONITOR_HEALTH",
                    "service": "kalshi_monitor",
                    "healthy": True,
                    "subscribed_markets": len(self.subscribed_tickers),
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
                    "type": "KALSHI_MONITOR_UNHEALTHY",
                    "service": "kalshi_monitor",
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

    monitor = KalshiMonitor()

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
