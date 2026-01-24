"""
Scheduler for analytics jobs.

Provides a simple cron-like scheduler for running archiver and ML analyzer jobs.
"""

import asyncio
import logging
from datetime import datetime, time
from typing import Callable, Coroutine, Optional

logger = logging.getLogger(__name__)


class Job:
    """A scheduled job."""

    def __init__(
        self,
        name: str,
        run_time: time,
        func: Callable[[], Coroutine],
        enabled: bool = True,
    ):
        self.name = name
        self.run_time = run_time
        self.func = func
        self.enabled = enabled
        self.last_run: Optional[datetime] = None
        self.run_count = 0
        self.error_count = 0

    def should_run(self, now: datetime) -> bool:
        """Check if job should run now."""
        if not self.enabled:
            return False

        today = now.date()
        current_time = now.time()

        # Already ran today?
        if self.last_run and self.last_run.date() == today:
            return False

        # Past scheduled time?
        return current_time >= self.run_time


class Scheduler:
    """Simple scheduler for analytics jobs."""

    def __init__(self):
        self.jobs: list[Job] = []
        self._running = False
        self._check_interval = 60  # seconds

    def add_job(
        self,
        name: str,
        run_time: time,
        func: Callable[[], Coroutine],
        enabled: bool = True,
    ) -> None:
        """Add a job to the scheduler."""
        job = Job(name=name, run_time=run_time, func=func, enabled=enabled)
        self.jobs.append(job)
        logger.info(f"Scheduled job '{name}' at {run_time}")

    async def start(self) -> None:
        """Start the scheduler loop."""
        self._running = True
        logger.info(f"Scheduler started with {len(self.jobs)} jobs")

        while self._running:
            now = datetime.now()

            for job in self.jobs:
                if job.should_run(now):
                    await self._run_job(job)

            await asyncio.sleep(self._check_interval)

    async def stop(self) -> None:
        """Stop the scheduler."""
        self._running = False
        logger.info("Scheduler stopped")

    async def _run_job(self, job: Job) -> None:
        """Run a job and update its state."""
        logger.info(f"Running job: {job.name}")
        start_time = datetime.now()

        try:
            await job.func()
            job.run_count += 1
            job.last_run = start_time
            elapsed = (datetime.now() - start_time).total_seconds()
            logger.info(f"Job '{job.name}' completed in {elapsed:.1f}s")
        except Exception as e:
            job.error_count += 1
            logger.error(f"Job '{job.name}' failed: {e}", exc_info=True)

    def get_status(self) -> dict:
        """Get scheduler status."""
        return {
            "running": self._running,
            "jobs": [
                {
                    "name": job.name,
                    "run_time": job.run_time.isoformat(),
                    "enabled": job.enabled,
                    "last_run": job.last_run.isoformat() if job.last_run else None,
                    "run_count": job.run_count,
                    "error_count": job.error_count,
                }
                for job in self.jobs
            ],
        }
