"""Configuration for GameArchiver service."""

import os
from dataclasses import dataclass


@dataclass
class ArchiverConfig:
    """Configuration for the game archiver service.

    Attributes:
        grace_period_minutes: Time to wait after game ends before archiving.
            This allows for score corrections and late position settlements.
        archive_batch_size: Maximum games to archive in a single batch.
        poll_interval_seconds: How often to check for games ready to archive.
        enable_data_cleanup: Whether to delete from live tables after archiving.
            If False, games are marked as archived but data remains.
    """
    grace_period_minutes: int = 60
    archive_batch_size: int = 10
    poll_interval_seconds: int = 300  # 5 minutes
    enable_data_cleanup: bool = False  # Conservative default: keep live data

    @classmethod
    def from_env(cls) -> "ArchiverConfig":
        """Create config from environment variables."""
        return cls(
            grace_period_minutes=int(os.environ.get("ARCHIVER_GRACE_PERIOD_MINUTES", "60")),
            archive_batch_size=int(os.environ.get("ARCHIVER_BATCH_SIZE", "10")),
            poll_interval_seconds=int(os.environ.get("ARCHIVER_POLL_INTERVAL", "300")),
            enable_data_cleanup=os.environ.get("ARCHIVER_CLEANUP_LIVE_DATA", "false").lower() == "true",
        )
