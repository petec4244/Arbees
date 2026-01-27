"""Type-safe Redis pub/sub messaging bus with msgpack serialization."""

import asyncio
import logging
import os
from datetime import datetime
from enum import Enum
from typing import Any, Callable, Coroutine, Generic, Optional, TypeVar

import msgpack
import redis.asyncio as redis
from pydantic import BaseModel

logger = logging.getLogger(__name__)

from arbees_shared.models.game import GameState, Play
from arbees_shared.models.market import MarketPrice
from arbees_shared.models.signal import TradingSignal, ArbitrageOpportunity
from arbees_shared.models.trade import PaperTrade

T = TypeVar("T", bound=BaseModel)


class Channel(str, Enum):
    """Predefined Redis channels."""
    # Game channels (use with game_id)
    GAME_STATE = "game:{game_id}:state"
    GAME_PLAY = "game:{game_id}:play"
    GAME_PRICE = "game:{game_id}:price"

    # Global channels
    SIGNALS_NEW = "signals:new"
    SIGNALS_EXECUTED = "signals:executed"
    ARBITRAGE_NEW = "arbitrage:new"
    TRADES_OPENED = "trades:opened"
    TRADES_CLOSED = "trades:closed"

    # ZMQ-bridged channels (from RPi monitor) - LEGACY, prefer VPN monitor
    POLYMARKET_ZMQ = "polymarket:zmq:prices"

    # Service health
    SHARD_HEARTBEAT = "shard:{shard_id}:heartbeat"
    ORCHESTRATOR_COMMAND = "orchestrator:command"

    # VPN-based Polymarket monitor channels
    MARKET_ASSIGNMENTS = "orchestrator:market_assignments"
    SYSTEM_ALERTS = "system:alerts"

    # Low-latency market discovery (Rust service)
    DISCOVERY_REQUESTS = "discovery:requests"
    DISCOVERY_RESULTS = "discovery:results"

    # Execution pipeline (Phase 1 split)
    EXECUTION_REQUESTS = "execution:requests"
    EXECUTION_RESULTS = "execution:results"
    POSITION_UPDATES = "positions:updates"

    # Health monitoring (heartbeats)
    HEALTH_HEARTBEATS = "health:heartbeats"

    # Feedback loop channels (loss analysis)
    FEEDBACK_RULES = "feedback:rules"
    FEEDBACK_LOSS_ANALYZED = "feedback:loss:analyzed"
    FEEDBACK_PATTERN_DETECTED = "feedback:pattern:detected"

    def format(self, **kwargs) -> str:
        """Format channel name with parameters."""
        return self.value.format(**kwargs)


def get_redis_url() -> str:
    """Get Redis URL from environment."""
    return os.environ.get("REDIS_URL", "redis://localhost:6379")


def serialize(data: Any) -> bytes:
    """Serialize data to msgpack bytes."""
    if isinstance(data, BaseModel):
        # Convert Pydantic model to dict
        payload = data.model_dump(mode="json")
    elif isinstance(data, dict):
        payload = data
    else:
        payload = {"value": data}

    return msgpack.packb(payload, default=_default_encoder)


def deserialize(data: bytes) -> dict:
    """Deserialize msgpack or JSON bytes to dict."""
    try:
        return msgpack.unpackb(data, raw=False, timestamp=3)
    except Exception:
        # Fallback to JSON for Rust services that publish JSON
        import json
        return json.loads(data.decode("utf-8"))


def _default_encoder(obj: Any) -> Any:
    """Custom encoder for non-standard types."""
    if isinstance(obj, datetime):
        return obj.isoformat()
    if isinstance(obj, Enum):
        return obj.value
    raise TypeError(f"Cannot serialize {type(obj)}")


class RedisBus:
    """Type-safe Redis pub/sub messaging bus."""

    def __init__(self, url: Optional[str] = None):
        self.url = url or get_redis_url()
        self._client: Optional[redis.Redis] = None
        self._pubsub: Optional[redis.client.PubSub] = None
        self._subscriptions: dict[str, list[Callable]] = {}
        self._running = False
        self._listener_task: Optional[asyncio.Task] = None

    async def connect(self) -> None:
        """Connect to Redis."""
        if self._client is None:
            self._client = redis.from_url(self.url, decode_responses=False)
            await self._client.ping()

    async def disconnect(self) -> None:
        """Disconnect from Redis."""
        self._running = False
        if self._listener_task:
            self._listener_task.cancel()
            try:
                await self._listener_task
            except asyncio.CancelledError:
                pass
        if self._pubsub:
            await self._pubsub.close()
        if self._client:
            await self._client.close()
            self._client = None

    async def _ensure_connected(self) -> redis.Redis:
        """Ensure we're connected and return client."""
        if self._client is None:
            await self.connect()
        return self._client  # type: ignore

    # ==========================================================================
    # Publishing
    # ==========================================================================

    async def publish(self, channel: str, message: Any) -> int:
        """Publish a message to a channel."""
        client = await self._ensure_connected()
        data = serialize(message)
        return await client.publish(channel, data)

    async def publish_game_state(self, game_id: str, state: GameState) -> int:
        """Publish a game state update."""
        channel = Channel.GAME_STATE.format(game_id=game_id)
        return await self.publish(channel, state)

    async def publish_play(self, game_id: str, play: Play) -> int:
        """Publish a new play."""
        channel = Channel.GAME_PLAY.format(game_id=game_id)
        return await self.publish(channel, play)

    async def publish_market_price(self, game_id: str, price: MarketPrice) -> int:
        """Publish a market price update."""
        channel = Channel.GAME_PRICE.format(game_id=game_id)
        return await self.publish(channel, price)

    async def publish_signal(self, signal: TradingSignal) -> int:
        """Publish a new trading signal."""
        return await self.publish(Channel.SIGNALS_NEW.value, signal)

    async def publish_arbitrage(self, opportunity: ArbitrageOpportunity) -> int:
        """Publish a new arbitrage opportunity."""
        return await self.publish(Channel.ARBITRAGE_NEW.value, opportunity)

    async def publish_trade_opened(self, trade: PaperTrade) -> int:
        """Publish a trade opened event."""
        return await self.publish(Channel.TRADES_OPENED.value, trade)

    async def publish_trade_closed(self, trade: PaperTrade) -> int:
        """Publish a trade closed event."""
        return await self.publish(Channel.TRADES_CLOSED.value, trade)

    async def publish_shard_heartbeat(self, shard_id: str, status: dict) -> int:
        """Publish a shard heartbeat."""
        channel = Channel.SHARD_HEARTBEAT.format(shard_id=shard_id)
        return await self.publish(channel, status)

    async def publish_zmq_polymarket_price(self, price: MarketPrice) -> int:
        """Publish a ZMQ-sourced Polymarket price update."""
        return await self.publish(Channel.POLYMARKET_ZMQ.value, price)

    # ==========================================================================
    # Subscribing
    # ==========================================================================

    async def subscribe(
        self,
        channel: str,
        callback: Callable[[dict], Coroutine[Any, Any, None]]
    ) -> None:
        """Subscribe to a channel with a callback."""
        client = await self._ensure_connected()

        if self._pubsub is None:
            self._pubsub = client.pubsub()

        # Track subscriptions
        if channel not in self._subscriptions:
            self._subscriptions[channel] = []
            await self._pubsub.subscribe(channel)

        self._subscriptions[channel].append(callback)

    async def psubscribe(
        self,
        pattern: str,
        callback: Callable[[str, dict], Coroutine[Any, Any, None]]
    ) -> None:
        """Subscribe to a channel pattern with a callback."""
        client = await self._ensure_connected()

        if self._pubsub is None:
            self._pubsub = client.pubsub()

        if pattern not in self._subscriptions:
            self._subscriptions[pattern] = []
            await self._pubsub.psubscribe(pattern)

        self._subscriptions[pattern].append(callback)

    async def unsubscribe(self, channel: str) -> None:
        """Unsubscribe from a channel."""
        if self._pubsub and channel in self._subscriptions:
            await self._pubsub.unsubscribe(channel)
            del self._subscriptions[channel]

    async def start_listening(self) -> None:
        """Start the message listener loop."""
        if self._running:
            return

        self._running = True
        self._listener_task = asyncio.create_task(self._listen())

    async def _listen(self) -> None:
        """Internal message listener loop."""
        if self._pubsub is None:
            return

        while self._running:
            try:
                message = await self._pubsub.get_message(
                    ignore_subscribe_messages=True,
                    timeout=1.0
                )
                if message is None:
                    continue

                msg_type = message.get("type")
                channel = message.get("channel")
                pattern = message.get("pattern")
                data = message.get("data")

                if not isinstance(data, bytes):
                    continue

                # Decode channel name
                if isinstance(channel, bytes):
                    channel = channel.decode("utf-8")
                if isinstance(pattern, bytes):
                    pattern = pattern.decode("utf-8")

                # Deserialize message
                try:
                    payload = deserialize(data)
                except Exception:
                    continue

                # Handle pattern subscriptions
                if msg_type == "pmessage" and pattern in self._subscriptions:
                    for callback in self._subscriptions[pattern]:
                        try:
                            await callback(channel, payload)
                        except Exception as e:
                            logger.error("Pattern subscription callback error on %s", pattern, exc_info=True)

                # Handle regular subscriptions
                elif msg_type == "message" and channel in self._subscriptions:
                    for callback in self._subscriptions[channel]:
                        try:
                            await callback(payload)
                        except Exception as e:
                            logger.error("Subscription callback error on %s", channel, exc_info=True)

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Redis listener error", exc_info=True)
                await asyncio.sleep(1)

    # ==========================================================================
    # Key-Value Operations (for state)
    # ==========================================================================

    async def set(self, key: str, value: Any, expire_seconds: Optional[int] = None) -> None:
        """Set a key-value pair."""
        client = await self._ensure_connected()
        data = serialize(value)
        if expire_seconds:
            await client.setex(key, expire_seconds, data)
        else:
            await client.set(key, data)

    async def get(self, key: str) -> Optional[dict]:
        """Get a value by key."""
        client = await self._ensure_connected()
        data = await client.get(key)
        if data is None:
            return None
        return deserialize(data)

    async def delete(self, key: str) -> int:
        """Delete a key."""
        client = await self._ensure_connected()
        return await client.delete(key)

    async def exists(self, key: str) -> bool:
        """Check if a key exists."""
        client = await self._ensure_connected()
        return bool(await client.exists(key))

    # ==========================================================================
    # List Operations (for queues)
    # ==========================================================================

    async def lpush(self, key: str, *values: Any) -> int:
        """Push values to the left of a list."""
        client = await self._ensure_connected()
        data = [serialize(v) for v in values]
        return await client.lpush(key, *data)

    async def rpush(self, key: str, *values: Any) -> int:
        """Push values to the right of a list."""
        client = await self._ensure_connected()
        data = [serialize(v) for v in values]
        return await client.rpush(key, *data)

    async def lpop(self, key: str) -> Optional[dict]:
        """Pop a value from the left of a list."""
        client = await self._ensure_connected()
        data = await client.lpop(key)
        if data is None:
            return None
        return deserialize(data)

    async def rpop(self, key: str) -> Optional[dict]:
        """Pop a value from the right of a list."""
        client = await self._ensure_connected()
        data = await client.rpop(key)
        if data is None:
            return None
        return deserialize(data)

    async def lrange(self, key: str, start: int, end: int) -> list[dict]:
        """Get a range of values from a list."""
        client = await self._ensure_connected()
        data = await client.lrange(key, start, end)
        return [deserialize(d) for d in data]

    async def llen(self, key: str) -> int:
        """Get the length of a list."""
        client = await self._ensure_connected()
        return await client.llen(key)

    # ==========================================================================
    # Hash Operations (for structured data)
    # ==========================================================================

    async def hset(self, key: str, field: str, value: Any) -> int:
        """Set a hash field."""
        client = await self._ensure_connected()
        data = serialize(value)
        return await client.hset(key, field, data)

    async def hget(self, key: str, field: str) -> Optional[dict]:
        """Get a hash field."""
        client = await self._ensure_connected()
        data = await client.hget(key, field)
        if data is None:
            return None
        return deserialize(data)

    async def hgetall(self, key: str) -> dict[str, dict]:
        """Get all hash fields."""
        client = await self._ensure_connected()
        data = await client.hgetall(key)
        return {
            k.decode("utf-8") if isinstance(k, bytes) else k: deserialize(v)
            for k, v in data.items()
        }

    async def hdel(self, key: str, *fields: str) -> int:
        """Delete hash fields."""
        client = await self._ensure_connected()
        return await client.hdel(key, *fields)


class TypedSubscriber(Generic[T]):
    """Type-safe subscriber for a specific message type."""

    def __init__(
        self,
        bus: RedisBus,
        channel: str,
        model_class: type[T]
    ):
        self.bus = bus
        self.channel = channel
        self.model_class = model_class
        self._callbacks: list[Callable[[T], Coroutine[Any, Any, None]]] = []

    async def subscribe(
        self,
        callback: Callable[[T], Coroutine[Any, Any, None]]
    ) -> None:
        """Subscribe with a typed callback."""
        self._callbacks.append(callback)

        async def wrapper(data: dict) -> None:
            try:
                model = self.model_class.model_validate(data)
                for cb in self._callbacks:
                    await cb(model)
            except Exception as e:
                logger.error("Typed subscriber error for %s", self.model_class.__name__, exc_info=True)

        await self.bus.subscribe(self.channel, wrapper)


# Convenience function for creating typed subscribers
def game_state_subscriber(bus: RedisBus, game_id: str) -> TypedSubscriber[GameState]:
    """Create a typed subscriber for game state updates."""
    channel = Channel.GAME_STATE.format(game_id=game_id)
    return TypedSubscriber(bus, channel, GameState)


def play_subscriber(bus: RedisBus, game_id: str) -> TypedSubscriber[Play]:
    """Create a typed subscriber for play updates."""
    channel = Channel.GAME_PLAY.format(game_id=game_id)
    return TypedSubscriber(bus, channel, Play)


def signal_subscriber(bus: RedisBus) -> TypedSubscriber[TradingSignal]:
    """Create a typed subscriber for trading signals."""
    return TypedSubscriber(bus, Channel.SIGNALS_NEW.value, TradingSignal)


def arbitrage_subscriber(bus: RedisBus) -> TypedSubscriber[ArbitrageOpportunity]:
    """Create a typed subscriber for arbitrage opportunities."""
    return TypedSubscriber(bus, Channel.ARBITRAGE_NEW.value, ArbitrageOpportunity)
