"""
Supervisor module for container health monitoring and auto-restart.

The Supervisor:
1. Monitors heartbeat TTL keys in Redis
2. Performs active health probes for third-party services
3. Detects missing/stale heartbeats
4. Attempts to restart unhealthy containers via Docker API
5. Tracks restart attempts with bounded retries and backoff
6. Escalates to alerts after max attempts exhausted
"""

import asyncio
import json
import logging
import os
from datetime import datetime, timedelta
from typing import Optional

import docker
import redis.asyncio as redis
import httpx
import asyncpg

from arbees_shared.messaging.redis_bus import Channel
from arbees_shared.models.health import (
    Heartbeat,
    RestartAttempt,
    ServiceHealthSummary,
    ServiceStatus,
)
from arbees_shared.health.heartbeat import HEARTBEAT_KEY_PREFIX, get_all_heartbeats

logger = logging.getLogger(__name__)


# Default configuration
DEFAULT_MAX_RESTART_ATTEMPTS = 3
DEFAULT_BACKOFF_SECS = [5, 15, 45]
DEFAULT_COOLDOWN_SECS = 600
DEFAULT_CHECK_INTERVAL_SECS = 15
DEFAULT_MISS_THRESHOLD = 3

# Stateless services that can be safely restarted
# These are the default allowlist - can be overridden via env var
DEFAULT_RESTART_ALLOW_SERVICES = [
    "arbees-game-shard-1",
    "arbees-polymarket-monitor",
    "arbees-futures-monitor",
    "arbees-api",
    "arbees-frontend",
    "arbees-signal-processor",
    "arbees-execution-service",
    "arbees-position-tracker",
    "arbees-analytics",
    "arbees-market-discovery",
]

# Services that should NEVER be auto-restarted
RESTART_DENY_SERVICES = [
    "arbees-timescaledb",
    "arbees-redis",
    "arbees-vpn",
    "arbees-orchestrator",  # Never restart ourselves
]

# Services that don't publish heartbeats (third-party images, static servers)
# These are checked via active health probes instead
NO_HEARTBEAT_SERVICES = [
    "arbees-timescaledb",  # Third-party PostgreSQL - checked via SELECT 1
    "arbees-redis",        # Third-party Redis - checked via PING
    "arbees-vpn",          # Third-party Gluetun VPN - checked via IP API
    "arbees-frontend",     # Static nginx server - checked via HTTP GET
]

# Health probe configuration for third-party services
HEALTH_PROBE_CONFIG = {
    "arbees-redis": {
        "type": "redis",
        "url": "redis://redis:6379",
    },
    "arbees-timescaledb": {
        "type": "postgres",
        "dsn": None,  # Will be built from env vars
    },
    "arbees-vpn": {
        "type": "http",
        "url": "http://vpn:8000/v1/publicip/ip",  # Gluetun control server
        "timeout": 5.0,
    },
    "arbees-frontend": {
        "type": "http",
        "url": "http://frontend:80/",
        "timeout": 3.0,
    },
}


class Supervisor:
    """
    Monitors container health via Redis heartbeats and auto-restarts unhealthy containers.
    """

    def __init__(
        self,
        redis_url: Optional[str] = None,
        max_restart_attempts: int = DEFAULT_MAX_RESTART_ATTEMPTS,
        backoff_secs: Optional[list[int]] = None,
        cooldown_secs: int = DEFAULT_COOLDOWN_SECS,
        check_interval_secs: int = DEFAULT_CHECK_INTERVAL_SECS,
        miss_threshold: int = DEFAULT_MISS_THRESHOLD,
        restart_allow_services: Optional[list[str]] = None,
    ):
        self.redis_url = redis_url or os.environ.get("REDIS_URL", "redis://localhost:6379")
        self.max_restart_attempts = int(
            os.environ.get("MAX_RESTART_ATTEMPTS", max_restart_attempts)
        )
        self.backoff_secs = backoff_secs or [
            int(x) for x in os.environ.get(
                "RESTART_BACKOFF_SECS", "5,15,45"
            ).split(",")
        ]
        self.cooldown_secs = int(os.environ.get("RESTART_COOLDOWN_SECS", cooldown_secs))
        self.check_interval_secs = int(
            os.environ.get("SUPERVISOR_CHECK_INTERVAL_SECS", check_interval_secs)
        )
        self.miss_threshold = int(os.environ.get("HEARTBEAT_MISS_THRESHOLD", miss_threshold))

        # Parse restart allowlist from env or use default
        allow_env = os.environ.get("RESTART_ALLOW_SERVICES", "")
        if allow_env:
            self.restart_allow_services = [s.strip() for s in allow_env.split(",") if s.strip()]
        else:
            self.restart_allow_services = restart_allow_services or DEFAULT_RESTART_ALLOW_SERVICES

        # Docker client
        self._docker: Optional[docker.DockerClient] = None

        # Redis client
        self._redis: Optional[redis.Redis] = None

        # State
        self._running = False
        self._task: Optional[asyncio.Task] = None

        # Track consecutive misses per service
        self._miss_counts: dict[str, int] = {}

        # Expected services (populated from Docker labels or config)
        self._expected_services: set[str] = set()

        # Restart attempt tracking (in Redis for persistence)
        self._restart_key_prefix = "health:restart:"

    async def start(self) -> None:
        """Start the supervisor."""
        logger.info(
            f"Starting Supervisor (max_attempts={self.max_restart_attempts}, "
            f"backoff={self.backoff_secs}, cooldown={self.cooldown_secs}s)"
        )

        # Connect to Redis
        self._redis = redis.from_url(self.redis_url, decode_responses=True)
        await self._redis.ping()

        # Connect to Docker
        try:
            self._docker = docker.from_env()
            self._docker.ping()
            logger.info("Docker client connected")
        except Exception as e:
            logger.error(f"Failed to connect to Docker: {e}")
            logger.warning("Supervisor will run in alert-only mode (no auto-restart)")
            self._docker = None

        # Discover expected services from running containers
        await self._discover_expected_services()

        self._running = True
        self._task = asyncio.create_task(self._monitor_loop())

        logger.info(
            f"Supervisor started. Monitoring {len(self._expected_services)} services. "
            f"Restart allowlist: {self.restart_allow_services}"
        )

    async def stop(self) -> None:
        """Stop the supervisor."""
        logger.info("Stopping Supervisor")
        self._running = False

        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

        if self._redis:
            await self._redis.close()

        if self._docker:
            self._docker.close()

        logger.info("Supervisor stopped")

    async def _discover_expected_services(self) -> None:
        """Discover expected services from Docker containers."""
        if not self._docker:
            return

        try:
            containers = self._docker.containers.list(
                filters={"label": "com.docker.compose.project=arbees"}
            )
            for container in containers:
                name = container.name
                if name and name.startswith("arbees-"):
                    self._expected_services.add(name)
                    logger.debug(f"Discovered service: {name}")

            logger.info(f"Discovered {len(self._expected_services)} arbees containers")
        except Exception as e:
            logger.warning(f"Failed to discover containers: {e}")

    async def _monitor_loop(self) -> None:
        """Main monitoring loop."""
        while self._running:
            try:
                await self._check_health()
            except Exception as e:
                logger.error(f"Error in supervisor monitor loop: {e}")

            await asyncio.sleep(self.check_interval_secs)

    # =========================================================================
    # Health Probes for Third-Party Services
    # =========================================================================

    def _get_database_url(self) -> str:
        """Build database URL from environment variables."""
        user = os.environ.get("POSTGRES_USER", "arbees")
        password = os.environ.get("POSTGRES_PASSWORD", "")
        host = "timescaledb"
        port = "5432"
        db = os.environ.get("POSTGRES_DB", "arbees")
        return f"postgresql://{user}:{password}@{host}:{port}/{db}"

    async def _probe_redis(self) -> tuple[bool, str]:
        """Check Redis connectivity via PING."""
        try:
            client = redis.from_url(self.redis_url, decode_responses=True)
            result = await asyncio.wait_for(client.ping(), timeout=3.0)
            await client.close()
            return (True, "PONG") if result else (False, "No response")
        except asyncio.TimeoutError:
            return (False, "Timeout")
        except Exception as e:
            return (False, str(e)[:50])

    async def _probe_postgres(self) -> tuple[bool, str]:
        """Check PostgreSQL/TimescaleDB connectivity via SELECT 1."""
        try:
            dsn = self._get_database_url()
            conn = await asyncio.wait_for(
                asyncpg.connect(dsn),
                timeout=5.0
            )
            result = await conn.fetchval("SELECT 1")
            await conn.close()
            return (True, "OK") if result == 1 else (False, f"Unexpected: {result}")
        except asyncio.TimeoutError:
            return (False, "Timeout")
        except Exception as e:
            return (False, str(e)[:50])

    async def _probe_http(self, url: str, timeout: float = 5.0) -> tuple[bool, str]:
        """Check HTTP service via GET request."""
        try:
            async with httpx.AsyncClient(timeout=timeout) as client:
                response = await client.get(url)
                if response.status_code < 400:
                    return (True, f"HTTP {response.status_code}")
                else:
                    return (False, f"HTTP {response.status_code}")
        except httpx.TimeoutException:
            return (False, "Timeout")
        except Exception as e:
            return (False, str(e)[:50])

    async def _probe_service(self, container_name: str) -> tuple[bool, str]:
        """Run health probe for a third-party service."""
        config = HEALTH_PROBE_CONFIG.get(container_name)
        if not config:
            return (False, "No probe configured")

        probe_type = config.get("type")

        if probe_type == "redis":
            return await self._probe_redis()
        elif probe_type == "postgres":
            return await self._probe_postgres()
        elif probe_type == "http":
            url = config.get("url", "")
            timeout = config.get("timeout", 5.0)
            return await self._probe_http(url, timeout)
        else:
            return (False, f"Unknown probe type: {probe_type}")

    async def _check_health(self) -> None:
        """Check health of all services and take action if needed."""
        # Get all current heartbeats
        heartbeats = await get_all_heartbeats(self.redis_url)

        # Build summary (exclude services that don't publish heartbeats)
        monitored_services = [
            s for s in self._expected_services 
            if s not in NO_HEARTBEAT_SERVICES
        ]
        summary = ServiceHealthSummary(
            timestamp=datetime.utcnow(),
            total_services=len(monitored_services),
            healthy_count=0,
            degraded_count=0,
            unhealthy_count=0,
            missing_count=0,
            services={},
        )

        # Map heartbeat keys to container names
        # Heartbeat key format: {service}:{instance_id}
        # Container names: arbees-{service} or arbees-{service}-{n}
        heartbeat_by_container: dict[str, Heartbeat] = {}
        for key, hb in heartbeats.items():
            # Try to map to container name
            container_name = self._heartbeat_key_to_container(hb.service, hb.instance_id)
            if container_name:
                heartbeat_by_container[container_name] = hb

        # Check each expected service
        for container_name in self._expected_services:
            # Skip services that don't publish heartbeats (third-party images)
            if container_name in NO_HEARTBEAT_SERVICES:
                continue

            hb = heartbeat_by_container.get(container_name)

            if not hb:
                # Missing heartbeat
                self._miss_counts[container_name] = self._miss_counts.get(container_name, 0) + 1
                summary.missing_count += 1
                summary.services[container_name] = ServiceStatus.UNHEALTHY

                if self._miss_counts[container_name] >= self.miss_threshold:
                    logger.warning(
                        f"Service {container_name} missing heartbeat "
                        f"({self._miss_counts[container_name]} consecutive misses)"
                    )
                    await self._handle_unhealthy_service(container_name, "missing_heartbeat")
            else:
                # Reset miss count
                self._miss_counts[container_name] = 0

                # Check status
                if hb.status == ServiceStatus.HEALTHY:
                    summary.healthy_count += 1
                    summary.services[container_name] = ServiceStatus.HEALTHY
                elif hb.status == ServiceStatus.DEGRADED:
                    summary.degraded_count += 1
                    summary.services[container_name] = ServiceStatus.DEGRADED
                else:
                    summary.unhealthy_count += 1
                    summary.services[container_name] = ServiceStatus.UNHEALTHY
                    await self._handle_unhealthy_service(container_name, f"status_{hb.status.value}")

        # Run health probes for third-party services
        probe_results: dict[str, tuple[bool, str]] = {}
        for container_name in NO_HEARTBEAT_SERVICES:
            if container_name in self._expected_services:
                healthy, msg = await self._probe_service(container_name)
                probe_results[container_name] = (healthy, msg)
                
                if healthy:
                    summary.healthy_count += 1
                    summary.services[container_name] = ServiceStatus.HEALTHY
                    self._miss_counts[container_name] = 0  # Reset miss count
                else:
                    self._miss_counts[container_name] = self._miss_counts.get(container_name, 0) + 1
                    if self._miss_counts[container_name] >= self.miss_threshold:
                        summary.unhealthy_count += 1
                        summary.services[container_name] = ServiceStatus.UNHEALTHY
                        logger.warning(f"Service {container_name} probe failed: {msg}")
                        await self._handle_unhealthy_service(container_name, f"probe_failed:{msg}")
                    else:
                        summary.degraded_count += 1
                        summary.services[container_name] = ServiceStatus.DEGRADED

        # Update total to include probed services
        summary.total_services = len(monitored_services) + len([
            s for s in NO_HEARTBEAT_SERVICES if s in self._expected_services
        ])

        # Log summary periodically
        probe_status = ", ".join(
            f"{name.replace('arbees-', '')}={'OK' if ok else 'FAIL'}"
            for name, (ok, _) in probe_results.items()
        )
        logger.info(
            f"Health summary: {summary.healthy_count} healthy, "
            f"{summary.degraded_count} degraded, {summary.unhealthy_count} unhealthy, "
            f"{summary.missing_count} missing | probes: {probe_status or 'none'}"
        )

        # Publish summary
        if self._redis:
            await self._redis.publish(
                Channel.SYSTEM_ALERTS.value,
                json.dumps({
                    "type": "HEALTH_SUMMARY",
                    "timestamp": summary.timestamp.isoformat(),
                    "total": summary.total_services,
                    "healthy": summary.healthy_count,
                    "degraded": summary.degraded_count,
                    "unhealthy": summary.unhealthy_count,
                    "missing": summary.missing_count,
                    "all_healthy": summary.all_healthy,
                }),
            )

    def _heartbeat_key_to_container(self, service: str, instance_id: str) -> Optional[str]:
        """Map heartbeat service/instance to container name."""
        # Common mappings
        mappings = {
            "orchestrator": "arbees-orchestrator",
            "game_shard": f"arbees-game-shard-{instance_id.split('-')[-1] if '-' in instance_id else '1'}",
            "polymarket_monitor": "arbees-polymarket-monitor",
            "futures_monitor": "arbees-futures-monitor",
            "api": "arbees-api",
            "signal_processor": "arbees-signal-processor",
            "execution_service": "arbees-execution-service",
            "position_tracker": "arbees-position-tracker",
            "analytics_service": "arbees-analytics",
            "market_discovery_rust": "arbees-market-discovery",
        }

        # Try direct mapping
        if service in mappings:
            return mappings[service]

        # Fallback: arbees-{service}
        return f"arbees-{service.replace('_', '-')}"

    async def _handle_unhealthy_service(self, container_name: str, reason: str) -> None:
        """Handle an unhealthy service - attempt restart or escalate."""
        # Check if this service can be restarted
        if container_name in RESTART_DENY_SERVICES:
            logger.warning(f"Service {container_name} is unhealthy but in deny list - alerting only")
            await self._publish_alert(container_name, reason, "deny_list")
            return

        if container_name not in self.restart_allow_services:
            logger.warning(f"Service {container_name} is unhealthy but not in allow list - alerting only")
            await self._publish_alert(container_name, reason, "not_in_allowlist")
            return

        # Get restart attempt state
        attempt = await self._get_restart_attempt(container_name)

        # Check if in cooldown
        if attempt.cooldown_until and datetime.utcnow() < attempt.cooldown_until:
            remaining = (attempt.cooldown_until - datetime.utcnow()).total_seconds()
            logger.info(f"Service {container_name} in cooldown ({remaining:.0f}s remaining)")
            return

        # Check if max attempts reached
        if attempt.attempt_count >= self.max_restart_attempts:
            logger.error(
                f"Service {container_name} exhausted restart attempts "
                f"({attempt.attempt_count}/{self.max_restart_attempts})"
            )
            await self._publish_alert(container_name, reason, "max_attempts_exhausted")

            # Set cooldown
            new_attempt = RestartAttempt(
                container_name=container_name,
                attempt_count=attempt.attempt_count,
                last_attempt_at=attempt.last_attempt_at,
                last_failure_reason=reason,
                cooldown_until=datetime.utcnow() + timedelta(seconds=self.cooldown_secs),
            )
            await self._save_restart_attempt(new_attempt)
            return

        # Calculate backoff
        backoff_idx = min(attempt.attempt_count, len(self.backoff_secs) - 1)
        backoff = self.backoff_secs[backoff_idx]

        # Check if enough time has passed since last attempt
        if attempt.last_attempt_at:
            time_since = (datetime.utcnow() - attempt.last_attempt_at).total_seconds()
            if time_since < backoff:
                logger.debug(
                    f"Service {container_name} waiting for backoff "
                    f"({time_since:.0f}s < {backoff}s)"
                )
                return

        # Attempt restart
        logger.warning(
            f"Attempting restart of {container_name} "
            f"(attempt {attempt.attempt_count + 1}/{self.max_restart_attempts})"
        )

        success = await self._restart_container(container_name)

        # Update attempt state
        new_attempt = RestartAttempt(
            container_name=container_name,
            attempt_count=attempt.attempt_count + 1,
            last_attempt_at=datetime.utcnow(),
            last_failure_reason=reason if not success else None,
            cooldown_until=None,
        )
        await self._save_restart_attempt(new_attempt)

        if success:
            logger.info(f"Successfully restarted {container_name}")
            # Reset miss count
            self._miss_counts[container_name] = 0
        else:
            logger.error(f"Failed to restart {container_name}")
            await self._publish_alert(container_name, reason, "restart_failed")

    async def _restart_container(self, container_name: str) -> bool:
        """Attempt to restart a container via Docker API."""
        if not self._docker:
            logger.warning("Docker client not available - cannot restart")
            return False

        try:
            container = self._docker.containers.get(container_name)
            container.restart(timeout=30)
            return True
        except docker.errors.NotFound:
            logger.error(f"Container {container_name} not found")
            return False
        except docker.errors.APIError as e:
            logger.error(f"Docker API error restarting {container_name}: {e}")
            return False
        except Exception as e:
            logger.error(f"Unexpected error restarting {container_name}: {e}")
            return False

    async def _get_restart_attempt(self, container_name: str) -> RestartAttempt:
        """Get restart attempt state from Redis."""
        if not self._redis:
            return RestartAttempt(container_name=container_name)

        key = f"{self._restart_key_prefix}{container_name}"
        data = await self._redis.get(key)

        if not data:
            return RestartAttempt(container_name=container_name)

        try:
            payload = json.loads(data)
            return RestartAttempt(
                container_name=container_name,
                attempt_count=payload.get("attempt_count", 0),
                last_attempt_at=datetime.fromisoformat(payload["last_attempt_at"])
                if payload.get("last_attempt_at") else None,
                last_failure_reason=payload.get("last_failure_reason"),
                cooldown_until=datetime.fromisoformat(payload["cooldown_until"])
                if payload.get("cooldown_until") else None,
            )
        except Exception as e:
            logger.warning(f"Failed to parse restart attempt for {container_name}: {e}")
            return RestartAttempt(container_name=container_name)

    async def _save_restart_attempt(self, attempt: RestartAttempt) -> None:
        """Save restart attempt state to Redis."""
        if not self._redis:
            return

        key = f"{self._restart_key_prefix}{attempt.container_name}"
        payload = {
            "attempt_count": attempt.attempt_count,
            "last_attempt_at": attempt.last_attempt_at.isoformat() if attempt.last_attempt_at else None,
            "last_failure_reason": attempt.last_failure_reason,
            "cooldown_until": attempt.cooldown_until.isoformat() if attempt.cooldown_until else None,
        }

        # TTL: cooldown + buffer
        ttl = self.cooldown_secs + 3600

        await self._redis.setex(key, ttl, json.dumps(payload))

    async def _publish_alert(self, container_name: str, reason: str, action: str) -> None:
        """Publish a service restart failure alert."""
        if not self._redis:
            return

        attempt = await self._get_restart_attempt(container_name)

        alert = {
            "type": "SERVICE_RESTART_FAILED",
            "container": container_name,
            "reason": reason,
            "action": action,
            "attempts": attempt.attempt_count,
            "last_attempt": attempt.last_attempt_at.isoformat() if attempt.last_attempt_at else None,
            "cooldown_until": attempt.cooldown_until.isoformat() if attempt.cooldown_until else None,
            "timestamp": datetime.utcnow().isoformat(),
        }

        await self._redis.publish(Channel.SYSTEM_ALERTS.value, json.dumps(alert))
        logger.error(f"Published alert: {alert}")
