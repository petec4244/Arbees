"""
Trace logger for NDJSON debug logs.

Provides end-to-end trade visibility by emitting structured JSON lines
to a debug log file. Each entry includes a trace_id that ties together
signals, execution requests, and position updates.
"""

import json
import logging
import os
import time
from datetime import datetime, timezone
from typing import Any, Optional

logger = logging.getLogger(__name__)

# Debug log file path (mounted from host in Docker)
DEBUG_LOG_PATH = os.environ.get("DEBUG_LOG_PATH", "/app/.cursor/debug.log")

# Enable/disable NDJSON trace logging
TRACE_ENABLED = os.environ.get("TRACE_LOGGING_ENABLED", "true").lower() in ("1", "true", "yes")


def trace_log(
    service: str,
    event: str,
    trace_id: Optional[str] = None,
    signal_id: Optional[str] = None,
    trade_id: Optional[str] = None,
    game_id: Optional[str] = None,
    **data: Any,
) -> None:
    """
    Write a structured NDJSON log entry for debugging.

    Args:
        service: Service name (signal_processor, execution_service, position_tracker, etc.)
        event: Event type (signal_received, filter_applied, execution_requested, etc.)
        trace_id: Request/idempotency key for end-to-end tracing
        signal_id: Trading signal ID
        trade_id: Paper trade ID
        game_id: ESPN game ID
        **data: Additional event-specific data
    """
    if not TRACE_ENABLED:
        return

    try:
        entry = {
            "ts": datetime.now(timezone.utc).isoformat(),
            "ts_ms": int(time.time() * 1000),
            "service": service,
            "event": event,
        }

        # Add trace envelope fields if provided
        if trace_id:
            entry["trace_id"] = trace_id
        if signal_id:
            entry["signal_id"] = signal_id
        if trade_id:
            entry["trade_id"] = trade_id
        if game_id:
            entry["game_id"] = game_id

        # Add all additional data
        entry.update(data)

        # Write to file
        with open(DEBUG_LOG_PATH, "a", encoding="utf-8") as f:
            f.write(json.dumps(entry, default=str) + "\n")

    except Exception as e:
        # Don't let trace logging failures affect main service
        logger.debug(f"Trace log write failed: {e}")


class TraceContext:
    """
    Context manager for grouped trace logging.

    Captures common fields once and allows multiple events
    to be logged with the same trace envelope.
    """

    def __init__(
        self,
        service: str,
        trace_id: Optional[str] = None,
        signal_id: Optional[str] = None,
        trade_id: Optional[str] = None,
        game_id: Optional[str] = None,
        sport: Optional[str] = None,
        platform: Optional[str] = None,
        market_id: Optional[str] = None,
        contract_team: Optional[str] = None,
        side: Optional[str] = None,
    ):
        self.service = service
        self.envelope = {}

        if trace_id:
            self.envelope["trace_id"] = trace_id
        if signal_id:
            self.envelope["signal_id"] = signal_id
        if trade_id:
            self.envelope["trade_id"] = trade_id
        if game_id:
            self.envelope["game_id"] = game_id
        if sport:
            self.envelope["sport"] = sport
        if platform:
            self.envelope["platform"] = platform
        if market_id:
            self.envelope["market_id"] = market_id
        if contract_team:
            self.envelope["contract_team"] = contract_team
        if side:
            self.envelope["side"] = side

    def log(self, event: str, **data: Any) -> None:
        """Log an event with the context's envelope fields."""
        trace_log(
            service=self.service,
            event=event,
            **self.envelope,
            **data,
        )

    def update(self, **fields: Any) -> None:
        """Update envelope fields (e.g., after getting trade_id)."""
        for k, v in fields.items():
            if v is not None:
                self.envelope[k] = v
