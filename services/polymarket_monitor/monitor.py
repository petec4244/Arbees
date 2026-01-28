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
        self.subscribed_conditions: set[str] = set()

        # token_id -> {condition_id, team_name}
        # For moneyline markets, each token represents one team's YES contract
        self._token_to_info: dict[str, dict] = {}
        
        # condition_id -> list of token_ids (usually 2 for moneyline)
        self._condition_to_tokens: dict[str, list[str]] = {}

        # Market metadata (condition_id -> {game_id, market_type, title, outcomes, home_team, away_team})
        self._market_metadata: dict[str, dict] = {}
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

        # ZMQ publisher for low-latency messaging (hot path)
        transport_mode = os.environ.get("ZMQ_TRANSPORT_MODE", "redis_only").lower()
        self._zmq_enabled = transport_mode in ("zmq_only", "both")
        self._zmq_context: Optional["zmq.asyncio.Context"] = None
        self._zmq_pub: Optional["zmq.asyncio.Socket"] = None
        self._zmq_seq = 0
        self._zmq_pub_port = int(os.environ.get("ZMQ_PUB_PORT", "5556"))

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
            logger.warning(f"ZMQ_TRANSPORT_MODE={transport_mode} but pyzmq not installed")
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
        
        For moneyline markets, subscribes to BOTH team tokens and tracks
        which token corresponds to which team.
        """
        # Fetch market details
        market = await self.poly_client.get_market(condition_id)
        if not market:
            logger.warning(f"Market not found: {condition_id}")
            return

        title = market.get("question", market.get("title", ""))
        volume = float(market.get("volume", 0) or 0)
        
        # Parse outcomes and token IDs
        # Polymarket returns: outcomes=["Team A", "Team B"], clobTokenIds=["token1", "token2"]
        outcomes_raw = market.get("outcomes", "[]")
        tokens_raw = market.get("clobTokenIds", "[]")
        
        # Handle JSON string or list
        import json
        if isinstance(outcomes_raw, str):
            try:
                outcomes = json.loads(outcomes_raw)
            except:
                outcomes = []
        else:
            outcomes = outcomes_raw or []
            
        if isinstance(tokens_raw, str):
            try:
                token_ids = json.loads(tokens_raw)
            except:
                token_ids = []
        else:
            token_ids = tokens_raw or []
        
        if len(outcomes) != len(token_ids) or not outcomes:
            logger.warning(f"Market {condition_id} has mismatched outcomes/tokens: {len(outcomes)} vs {len(token_ids)}")
            # Fallback to single token resolution
            token_id = await self.poly_client.resolve_yes_token_id(market)
            if token_id:
                outcomes = [title]
                token_ids = [token_id]
            else:
                return

        # Store metadata with team info
        self._market_metadata[condition_id] = {
            "game_id": game_id,
            "market_type": market_type,
            "title": title,
            "volume": volume,
            "outcomes": outcomes,
            "home_team": home_team,
            "away_team": away_team,
        }
        
        # Track all tokens for this condition
        self._condition_to_tokens[condition_id] = token_ids
        
        # Subscribe to ALL tokens (both teams for moneyline)
        ws_markets = []
        for i, (outcome, token_id) in enumerate(zip(outcomes, token_ids)):
            if token_id in self.subscribed_tokens:
                logger.debug(f"Already subscribed to token {token_id[:16]}... ({outcome})")
                continue
                
            # Store token -> info mapping
            self._token_to_info[token_id] = {
                "condition_id": condition_id,
                "team_name": outcome,  # e.g., "Binghamton Bearcats"
                "outcome_index": i,
            }
            
            ws_markets.append({
                "token_id": token_id,
                "condition_id": condition_id,
                "title": f"{title} - {outcome}",
                "game_id": game_id,
                "volume": volume,
                "market_type": market_type,
            })
            
            self.subscribed_tokens.add(token_id)
            logger.info(f"Subscribing to Polymarket token: {token_id[:16]}... for '{outcome}'")
        
        if ws_markets:
            await self.poly_client.subscribe_with_metadata(ws_markets)
            
        self.subscribed_conditions.add(condition_id)
        logger.info(f"Subscribed to Polymarket: {condition_id[:16]}... ({market_type}) with {len(token_ids)} outcomes: {outcomes}")

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
        """
        Handle incoming price update, normalize, and publish to Redis.
        
        CRITICAL: Each token represents ONE team's YES contract.
        We include contract_team in the published MarketPrice so downstream
        consumers can match signal.team to the correct contract.
        """
        # Normalize: price.market_id is token_id, we need condition_id + team
        token_id = price.market_id
        token_info = self._token_to_info.get(token_id)

        if not token_info:
            # Try reverse lookup (maybe it's already a condition_id)
            if token_id in self._market_metadata:
                # This is a condition_id, not a token_id - shouldn't happen normally
                condition_id = token_id
                contract_team = None
            else:
                logger.debug(f"Unknown token_id: {token_id[:16]}...")
                return
        else:
            condition_id = token_info["condition_id"]
            contract_team = token_info["team_name"]  # e.g., "Binghamton Bearcats"

        # Get metadata
        meta = self._market_metadata.get(condition_id, {})
        game_id = meta.get("game_id")
        market_type = meta.get("market_type", "moneyline")

        if not game_id:
            logger.debug(f"No game_id for condition {condition_id[:16]}...")
            return

        # Drop stale markets: only publish the currently active (game_id, market_type) assignment.
        active = self._active_by_game_type.get((str(game_id), str(market_type)))
        if active and active != condition_id:
            return

        # Build normalized MarketPrice with:
        # - market_id = condition_id (for market identification)
        # - contract_team = which team this YES contract is for
        # - market_title includes team name for clarity
        title = meta.get("title", price.market_title)
        if contract_team and contract_team not in title:
            title = f"{title} [{contract_team}]"
            
        normalized_price = MarketPrice(
            market_id=condition_id,
            platform=Platform.POLYMARKET,
            game_id=game_id,
            market_title=title,
            contract_team=contract_team,  # CRITICAL: which team's YES contract
            yes_bid=price.yes_bid,
            yes_ask=price.yes_ask,
            yes_bid_size=price.yes_bid_size,
            yes_ask_size=price.yes_ask_size,
            volume=meta.get("volume", price.volume),
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

        # Publish to per-game price channel
        await self.redis.publish_market_price(game_id, normalized_price)

        self._prices_published += 1
        self._last_price_time = datetime.utcnow()

        # Publish to ZMQ for low-latency consumers (hot path)
        if self._zmq_enabled and self._zmq_pub:
            await self._publish_zmq_price(condition_id, game_id, normalized_price)

        logger.debug(
            f"Published Polymarket price: {condition_id[:12]}... team='{contract_team}' "
            f"bid={normalized_price.yes_bid:.3f} ask={normalized_price.yes_ask:.3f} "
            f"game={game_id}"
        )

    async def _publish_zmq_price(self, condition_id: str, game_id: str, price: MarketPrice):
        """Publish price to ZMQ PUB socket for low-latency consumers."""
        if not self._zmq_pub:
            return

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

        while self._running:
            try:
                if not self._active_by_game_type:
                    await asyncio.sleep(self._poll_interval_s)
                    continue

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
        """Periodic health checks."""
        while self._running:
            try:
                # Verify VPN is still working
                await self._verify_vpn()

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
