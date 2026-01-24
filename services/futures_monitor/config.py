"""Configuration for FuturesMonitor service."""

import os
from dataclasses import dataclass


@dataclass
class FuturesConfig:
    """Configuration for the futures monitor service.

    Attributes:
        lookahead_hours: How far ahead to look for games (default 48h).
        min_hours_before_start: Minimum hours before game starts to begin monitoring.
        handoff_minutes: When to hand off to Orchestrator (minutes before start).
        game_discovery_interval_seconds: How often to poll ESPN for upcoming games.
        price_poll_interval_seconds: How often to poll market prices.
        min_edge_pct: Minimum edge to generate a signal.
        line_movement_alert_pct: Threshold for line movement alerts.
        max_concurrent_games: Maximum games to monitor concurrently.
    """
    lookahead_hours: int = 48
    min_hours_before_start: float = 0.5  # At least 30 min before
    handoff_minutes: int = 15
    game_discovery_interval_seconds: int = 1800  # 30 minutes
    price_poll_interval_seconds: int = 60
    min_edge_pct: float = 5.0
    line_movement_alert_pct: float = 3.0
    max_concurrent_games: int = 50

    @classmethod
    def from_env(cls) -> "FuturesConfig":
        """Create config from environment variables."""
        return cls(
            lookahead_hours=int(os.environ.get("FUTURES_LOOKAHEAD_HOURS", "48")),
            min_hours_before_start=float(os.environ.get("FUTURES_MIN_HOURS_BEFORE_START", "0.5")),
            handoff_minutes=int(os.environ.get("FUTURES_HANDOFF_MINUTES", "15")),
            game_discovery_interval_seconds=int(os.environ.get("FUTURES_DISCOVERY_INTERVAL", "1800")),
            price_poll_interval_seconds=int(os.environ.get("FUTURES_PRICE_POLL_INTERVAL", "60")),
            min_edge_pct=float(os.environ.get("FUTURES_MIN_EDGE", "5.0")),
            line_movement_alert_pct=float(os.environ.get("FUTURES_LINE_MOVEMENT_ALERT", "3.0")),
            max_concurrent_games=int(os.environ.get("FUTURES_MAX_GAMES", "50")),
        )
