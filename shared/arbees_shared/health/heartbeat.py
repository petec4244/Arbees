"""
Heartbeat publisher for service health monitoring.

Each service creates a HeartbeatPublisher instance and calls start() to begin
publishing heartbeats. The orchestrator monitors these to detect failures.
"""

import asyncio
import json
import logging
import os
import socket
from datetime import datetime
from typing import Optional

import redis.asyncio as redis

from arbees_shared.models.health import Heartbeat, ServiceStatus

logger = logging.getLogger(__name__)

# Default configuration (can be overridden via environment variables)
DEFAULT_HEARTBEAT_INTERVAL_SECS = 10
DEFAULT_HEARTBEAT_TTL_SECS = 35  # 3x interval + buffer

# Redis key/channel patterns
HEARTBEAT_KEY_PREFIX = "health:hb"
HEARTBEAT_CHANNEL = "health:heartbeats"


def _get_redis_url() -> str:
    """Get Redis URL from environment."""
    return os.environ.get("REDIS_URL", "redis://localhost:6379")


class HeartbeatPublisher:
    """
    Publishes periodic heartbeats for a service instance.
    
    Usage:
        publisher = HeartbeatPublisher(
            service="game_shard",
            instance_id="shard-1",
        )
        await publisher.start()
        
        # Update status as conditions change
        publisher.set_status(ServiceStatus.DEGRADED)
        publisher.update_checks({"ws_ok": False, "redis_ok": True})
        publisher.update_metrics({"games_monitored": 5})
        
        # On shutdown
        await publisher.stop()
    """

    def __init__(
        self,
        service: str,
        instance_id: str,
        interval_secs: Optional[float] = None,
        ttl_secs: Optional[int] = None,
        redis_url: Optional[str] = None,
        version: Optional[str] = None,
    ):
        """
        Initialize heartbeat publisher.
        
        Args:
            service: Service name (e.g., "game_shard", "polymarket_monitor")
            instance_id: Instance identifier (e.g., container name, shard ID)
            interval_secs: Heartbeat interval (default: 10s)
            ttl_secs: Redis key TTL (default: 35s)
            redis_url: Redis connection URL
            version: Optional version/build identifier
        """
        self.service = service
        self.instance_id = instance_id
        self.interval_secs = interval_secs or float(
            os.environ.get("HEARTBEAT_INTERVAL_SECS", DEFAULT_HEARTBEAT_INTERVAL_SECS)
        )
        self.ttl_secs = ttl_secs or int(
            os.environ.get("HEARTBEAT_TTL_SECS", DEFAULT_HEARTBEAT_TTL_SECS)
        )
        self.redis_url = redis_url or _get_redis_url()
        self.version = version or os.environ.get("BUILD_VERSION", "unknown")

        # State
        self._status = ServiceStatus.STARTING
        self._checks: dict[str, bool] = {}
        self._metrics: dict[str, float] = {}
        self._started_at = datetime.utcnow()
        self._hostname = socket.gethostname()

        # Redis connection
        self._redis: Optional[redis.Redis] = None
        self._task: Optional[asyncio.Task] = None
        self._running = False

    @property
    def redis_key(self) -> str:
        """Redis key for this instance's heartbeat."""
        return f"{HEARTBEAT_KEY_PREFIX}:{self.service}:{self.instance_id}"

    def set_status(self, status: ServiceStatus) -> None:
        """Update the service status."""
        if self._status != status:
            logger.info(f"Heartbeat status changed: {self._status.value} -> {status.value}")
            self._status = status

    def update_checks(self, checks: dict[str, bool]) -> None:
        """Update health check results."""
        self._checks.update(checks)
        
        # Auto-determine status from checks
        if not all(checks.values()):
            if self._status == ServiceStatus.HEALTHY:
                self._status = ServiceStatus.DEGRADED

    def update_metrics(self, metrics: dict[str, float]) -> None:
        """Update runtime metrics."""
        self._metrics.update(metrics)

    def set_healthy(self) -> None:
        """Mark service as healthy (convenience method)."""
        self._status = ServiceStatus.HEALTHY

    def set_degraded(self, reason: Optional[str] = None) -> None:
        """Mark service as degraded (convenience method)."""
        self._status = ServiceStatus.DEGRADED
        if reason:
            logger.warning(f"Service degraded: {reason}")

    def set_unhealthy(self, reason: Optional[str] = None) -> None:
        """Mark service as unhealthy (convenience method)."""
        self._status = ServiceStatus.UNHEALTHY
        if reason:
            logger.error(f"Service unhealthy: {reason}")

    async def start(self) -> None:
        """Start publishing heartbeats."""
        if self._running:
            return

        logger.info(
            f"Starting heartbeat publisher: {self.service}/{self.instance_id} "
            f"(interval={self.interval_secs}s, ttl={self.ttl_secs}s)"
        )

        # Connect to Redis
        self._redis = redis.from_url(self.redis_url, decode_responses=True)
        await self._redis.ping()

        self._running = True
        self._started_at = datetime.utcnow()

        # Start background task
        self._task = asyncio.create_task(self._publish_loop())

    async def stop(self) -> None:
        """Stop publishing heartbeats (graceful shutdown)."""
        if not self._running:
            return

        logger.info(f"Stopping heartbeat publisher: {self.service}/{self.instance_id}")
        self._status = ServiceStatus.STOPPING
        
        # Publish final heartbeat
        await self._publish_heartbeat()

        self._running = False

        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

        # Delete the key so orchestrator sees immediate absence
        if self._redis:
            try:
                await self._redis.delete(self.redis_key)
            except Exception:
                pass
            await self._redis.close()

    async def _publish_loop(self) -> None:
        """Background loop that publishes heartbeats."""
        while self._running:
            try:
                await self._publish_heartbeat()
            except Exception as e:
                logger.warning(f"Heartbeat publish failed: {e}")

            await asyncio.sleep(self.interval_secs)

    async def _publish_heartbeat(self) -> None:
        """Publish a single heartbeat."""
        if not self._redis:
            return

        heartbeat = Heartbeat(
            service=self.service,
            instance_id=self.instance_id,
            status=self._status,
            started_at=self._started_at,
            timestamp=datetime.utcnow(),
            checks=self._checks.copy(),
            metrics=self._metrics.copy(),
            version=self.version,
            hostname=self._hostname,
        )

        # Serialize to JSON
        payload = heartbeat.model_dump(mode="json")
        payload_str = json.dumps(payload)

        # SETEX for liveness (authoritative)
        await self._redis.setex(self.redis_key, self.ttl_secs, payload_str)

        # Publish for real-time observability
        await self._redis.publish(HEARTBEAT_CHANNEL, payload_str)

        logger.debug(
            f"Heartbeat: {self.service}/{self.instance_id} "
            f"status={self._status.value} checks={self._checks}"
        )

    async def publish_once(self) -> None:
        """Publish a single heartbeat immediately (for testing or manual trigger)."""
        if not self._redis:
            self._redis = redis.from_url(self.redis_url, decode_responses=True)
            await self._redis.ping()
        await self._publish_heartbeat()


async def get_all_heartbeats(redis_url: Optional[str] = None) -> dict[str, Heartbeat]:
    """
    Read all current heartbeat keys from Redis.
    
    Returns a dict mapping "{service}:{instance_id}" to Heartbeat.
    Useful for orchestrator to scan state on startup.
    """
    url = redis_url or _get_redis_url()
    client = redis.from_url(url, decode_responses=True)
    
    try:
        pattern = f"{HEARTBEAT_KEY_PREFIX}:*"
        keys = []
        async for key in client.scan_iter(pattern):
            keys.append(key)

        heartbeats = {}
        for key in keys:
            try:
                data = await client.get(key)
                if data:
                    payload = json.loads(data)
                    hb = Heartbeat(**payload)
                    heartbeats[f"{hb.service}:{hb.instance_id}"] = hb
            except Exception as e:
                logger.warning(f"Failed to parse heartbeat {key}: {e}")

        return heartbeats
    finally:
        await client.close()
