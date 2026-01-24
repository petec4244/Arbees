"""
Analytics Service - Phase 2 merged archiver + ml_analyzer.

This service combines:
1. GameArchiver - Archives completed games (event-driven + polling)
2. MLAnalyzer - Nightly performance analysis and ML training

Single container for all "cold path" batch jobs.
"""

import asyncio
import logging
import os
from datetime import time, datetime
from typing import Optional

from arbees_shared.db.connection import get_pool, close_pool
from arbees_shared.messaging.redis_bus import RedisBus
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus

# Import existing implementations
from services.archiver.archiver import GameArchiver
from services.archiver.config import ArchiverConfig
from services.ml_analyzer.analyzer import MLAnalyzer
from services.ml_analyzer.config import AnalyzerConfig

from .scheduler import Scheduler
from .feedback.feedback_service import FeedbackService, OperatingMode

logger = logging.getLogger(__name__)


class AnalyticsService:
    """
    Unified analytics service combining archiver and ML analyzer.

    Features:
    - Event-driven archiving (Redis games:ended)
    - Scheduled ML analysis (default 11pm)
    - Unified health monitoring
    - Single container deployment
    """

    def __init__(
        self,
        archiver_config: Optional[ArchiverConfig] = None,
        ml_config: Optional[AnalyzerConfig] = None,
    ):
        self.archiver_config = archiver_config or ArchiverConfig.from_env()
        self.ml_config = ml_config or AnalyzerConfig.from_env()

        # Components
        self.archiver: Optional[GameArchiver] = None
        self.ml_analyzer: Optional[MLAnalyzer] = None
        self.scheduler: Optional[Scheduler] = None
        self.redis: Optional[RedisBus] = None
        self.feedback_service: Optional[FeedbackService] = None

        # State
        self._running = False

        # Heartbeat publisher
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

    async def start(self) -> None:
        """Start the analytics service."""
        logger.info("Starting Analytics Service")

        # Connect to database
        pool = await get_pool()

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Initialize archiver (event-driven)
        self.archiver = GameArchiver(
            db=None,  # Will create its own
            redis=self.redis,
            config=self.archiver_config,
        )
        await self.archiver.start()
        logger.info("Archiver started (event-driven + polling)")

        # Initialize ML analyzer
        self.ml_analyzer = MLAnalyzer(config=self.ml_config)

        # Initialize feedback service for loss analysis loop
        feedback_mode = OperatingMode(
            os.environ.get("FEEDBACK_MODE", "learning").lower()
        )
        self.feedback_service = FeedbackService(
            redis=self.redis,
            mode=feedback_mode,
            pattern_check_interval=float(os.environ.get("PATTERN_CHECK_INTERVAL", "300")),
            lookback_hours=int(os.environ.get("FEEDBACK_LOOKBACK_HOURS", "24")),
        )
        await self.feedback_service.start()
        logger.info(f"Feedback service started (mode={feedback_mode.value})")

        # Setup scheduler for ML jobs
        self.scheduler = Scheduler()

        # Parse ML run time
        ml_hour, ml_minute = map(int, self.ml_config.run_time.split(":"))
        ml_run_time = time(hour=ml_hour, minute=ml_minute)

        # Add ML analysis job
        self.scheduler.add_job(
            name="ml_analysis",
            run_time=ml_run_time,
            func=self._run_ml_analysis,
            enabled=True,
        )

        # Add report cleanup job (midnight)
        self.scheduler.add_job(
            name="report_cleanup",
            run_time=time(hour=0, minute=30),
            func=self._cleanup_old_reports,
            enabled=True,
        )

        self._running = True

        # Start scheduler in background
        asyncio.create_task(self.scheduler.start())

        # Start heartbeat
        asyncio.create_task(self._heartbeat_loop())

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="analytics_service",
            instance_id=os.environ.get("HOSTNAME", "analytics-service-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
            "archiver_ok": self.archiver is not None,
            "ml_analyzer_ok": self.ml_analyzer is not None,
            "feedback_ok": self.feedback_service is not None,
        })

        # Subscribe to on-demand analysis requests
        await self.redis.subscribe("ml:analysis:request", self._handle_analysis_request)
        asyncio.create_task(self.redis.start_listening())

        logger.info(
            f"Analytics Service started (ML scheduled at {self.ml_config.run_time})"
        )

    async def stop(self) -> None:
        """Stop the analytics service."""
        logger.info("Stopping Analytics Service")
        self._running = False

        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()

        if self.scheduler:
            await self.scheduler.stop()

        if self.archiver:
            await self.archiver.stop()

        if self.feedback_service:
            await self.feedback_service.stop()

        if self.redis:
            await self.redis.disconnect()

        await close_pool()
        logger.info("Analytics Service stopped")

    async def _run_ml_analysis(self) -> None:
        """Run the nightly ML analysis."""
        if not self.ml_analyzer:
            return

        try:
            logger.info("Starting scheduled ML analysis")
            insights = await self.ml_analyzer.run_nightly_analysis()
            logger.info(
                f"ML analysis complete: {insights.total_trades} trades, "
                f"${insights.total_pnl:.2f} P&L"
            )
        except Exception as e:
            logger.error(f"ML analysis failed: {e}", exc_info=True)

    async def _cleanup_old_reports(self) -> None:
        """Clean up old report files."""
        try:
            from pathlib import Path
            from datetime import timedelta

            report_dir = Path(self.ml_config.report_output_dir)
            if not report_dir.exists():
                return

            cutoff = datetime.now() - timedelta(days=30)
            cleaned = 0

            for report_file in report_dir.glob("*.md"):
                if report_file.stat().st_mtime < cutoff.timestamp():
                    report_file.unlink()
                    cleaned += 1

            for report_file in report_dir.glob("*.html"):
                if report_file.stat().st_mtime < cutoff.timestamp():
                    report_file.unlink()
                    cleaned += 1

            if cleaned > 0:
                logger.info(f"Cleaned up {cleaned} old report files")

        except Exception as e:
            logger.error(f"Report cleanup failed: {e}")

    async def _handle_analysis_request(self, message: dict) -> None:
        """Handle on-demand analysis request."""
        if not self.ml_analyzer:
            return

        try:
            request_date = message.get("date")
            if not request_date:
                logger.warning("Analysis request missing date")
                return

            from datetime import date as date_type

            if isinstance(request_date, str):
                analysis_date = datetime.strptime(request_date, "%Y-%m-%d").date()
            else:
                analysis_date = request_date

            logger.info(f"Processing on-demand analysis for {analysis_date}")
            await self.ml_analyzer.run_nightly_analysis(analysis_date)

            # Notify completion
            if self.redis:
                await self.redis.publish("ml:analysis:complete", {
                    "date": analysis_date.isoformat(),
                    "completed_at": datetime.utcnow().isoformat(),
                    "status": "success",
                })

        except Exception as e:
            logger.error(f"On-demand analysis failed: {e}", exc_info=True)
            if self.redis:
                await self.redis.publish("ml:analysis:complete", {
                    "date": message.get("date"),
                    "completed_at": datetime.utcnow().isoformat(),
                    "status": "error",
                    "error": str(e),
                })

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                archiver_pending = len(self.archiver._pending_archives) if self.archiver else 0
                scheduler_status = self.scheduler.get_status() if self.scheduler else {}
                feedback_stats = self.feedback_service.get_stats() if self.feedback_service else {}

                status = {
                    "type": "analytics_service",
                    "archiver_pending": archiver_pending,
                    "scheduler": scheduler_status,
                    "feedback": feedback_stats,
                    "timestamp": datetime.utcnow().isoformat(),
                }

                logger.info(
                    f"Analytics Service: {archiver_pending} pending archives, "
                    f"{len(scheduler_status.get('jobs', []))} scheduled jobs, "
                    f"{feedback_stats.get('rules_generated', 0)} rules generated"
                )

                # Update health monitoring heartbeat
                if self._heartbeat_publisher:
                    self._heartbeat_publisher.update_metrics({
                        "archiver_pending": float(archiver_pending),
                        "scheduled_jobs": float(len(scheduler_status.get("jobs", []))),
                        "feedback_trades_analyzed": float(feedback_stats.get("trades_analyzed", 0)),
                        "feedback_rules_generated": float(feedback_stats.get("rules_generated", 0)),
                    })

            except Exception as e:
                logger.warning(f"Heartbeat error: {e}")

            await asyncio.sleep(60)


async def main():
    """Main entry point."""
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    service = AnalyticsService()

    try:
        await service.start()

        # Keep running
        while True:
            await asyncio.sleep(1)

    except KeyboardInterrupt:
        pass
    finally:
        await service.stop()


if __name__ == "__main__":
    asyncio.run(main())
