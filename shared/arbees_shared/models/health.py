"""Health monitoring Pydantic models for heartbeat and service status."""

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field


class ServiceStatus(str, Enum):
    """Status of a service instance."""
    STARTING = "starting"      # Service is initializing
    HEALTHY = "healthy"        # All critical checks pass
    DEGRADED = "degraded"      # Running but with issues (e.g., WS down but REST working)
    UNHEALTHY = "unhealthy"    # Cannot perform primary function
    STOPPING = "stopping"      # Graceful shutdown in progress


class Heartbeat(BaseModel):
    """
    Heartbeat message published by each service instance.
    
    Used for:
    - Liveness detection (via Redis TTL key)
    - Real-time observability (via Redis pubsub)
    - Service health dashboard
    """
    model_config = ConfigDict(frozen=True)

    # Identity
    service: str  # e.g., "game_shard", "polymarket_monitor"
    instance_id: str  # e.g., container name or shard_id

    # Status
    status: ServiceStatus = ServiceStatus.HEALTHY
    
    # Timestamps
    started_at: datetime  # When the service started
    timestamp: datetime = Field(default_factory=datetime.utcnow)

    # Health checks (service-specific booleans)
    # Examples: redis_ok, db_ok, ws_ok, vpn_ok
    checks: dict[str, bool] = Field(default_factory=dict)

    # Runtime metrics (service-specific)
    # Examples: games_monitored, signals_generated, last_price_age_s
    metrics: dict[str, float] = Field(default_factory=dict)

    # Optional metadata
    version: Optional[str] = None  # Git SHA or build tag
    hostname: Optional[str] = None  # Container hostname

    def is_healthy(self) -> bool:
        """Check if all critical checks pass."""
        if not self.checks:
            return self.status == ServiceStatus.HEALTHY
        return all(self.checks.values()) and self.status in (
            ServiceStatus.HEALTHY,
            ServiceStatus.STARTING,
        )


class RestartAttempt(BaseModel):
    """
    Tracks restart attempts for a container.
    
    Stored in Redis with TTL to prevent infinite restart loops.
    """
    model_config = ConfigDict(frozen=True)

    container_name: str
    attempt_count: int = 0
    last_attempt_at: Optional[datetime] = None
    last_failure_reason: Optional[str] = None
    cooldown_until: Optional[datetime] = None

    def can_restart(self, max_attempts: int, now: Optional[datetime] = None) -> bool:
        """Check if another restart attempt is allowed."""
        if now is None:
            now = datetime.utcnow()
        
        # Check cooldown
        if self.cooldown_until and now < self.cooldown_until:
            return False
        
        # Check attempt count
        return self.attempt_count < max_attempts


class ServiceHealthSummary(BaseModel):
    """Summary of all service health states (for logging/alerts)."""
    # NOTE: Not frozen because supervisor needs to update counts incrementally
    model_config = ConfigDict(frozen=False)

    timestamp: datetime = Field(default_factory=datetime.utcnow)
    total_services: int = 0
    healthy_count: int = 0
    degraded_count: int = 0
    unhealthy_count: int = 0
    missing_count: int = 0  # Expected but no heartbeat

    services: dict[str, ServiceStatus] = Field(default_factory=dict)

    @property
    def all_healthy(self) -> bool:
        return self.unhealthy_count == 0 and self.missing_count == 0
