"""
Feedback Service - Main orchestrator for loss analysis feedback loop.

Responsibilities:
1. Subscribe to trades:closed channel
2. Analyze each losing trade immediately
3. Run pattern detection every 5 minutes
4. Generate and publish rules to feedback:rules channel
"""

import asyncio
import logging
import os
from datetime import datetime
from enum import Enum
from typing import Optional

from arbees_shared.db.connection import get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel

from .loss_analyzer import LossAnalyzer, LossRootCause, RootCauseType
from .pattern_detector import PatternDetector, DetectedPattern
from .rule_generator import RuleGenerator, TradingRule, RuleStatus

logger = logging.getLogger(__name__)


class OperatingMode(str, Enum):
    """Operating mode for the feedback service."""
    LEARNING = "learning"  # Suggest rules but require approval
    AUTO = "auto"  # Auto-apply solvable, circuit-break unsolvable


class FeedbackService:
    """
    Orchestrates the loss analysis feedback loop.

    Flow:
    1. Trade closes (loss) -> LossAnalyzer classifies root cause
    2. Periodic -> PatternDetector finds systemic issues
    3. Pattern detected -> RuleGenerator creates blocking rules
    4. Rules published -> SignalProcessor blocks future bad trades
    """

    def __init__(
        self,
        redis: Optional[RedisBus] = None,
        mode: OperatingMode = OperatingMode.LEARNING,
        pattern_check_interval: float = 300.0,  # 5 minutes
        lookback_hours: int = 24,
    ):
        self.redis = redis
        self.mode = mode
        self.pattern_check_interval = pattern_check_interval
        self.lookback_hours = lookback_hours

        # Components
        self.loss_analyzer = LossAnalyzer()
        self.pattern_detector = PatternDetector(
            min_samples_detect=3,
            min_samples_act=5,
            max_win_rate=0.40,
            lookback_hours=lookback_hours,
        )
        self.rule_generator = RuleGenerator(
            auto_approve=(mode == OperatingMode.AUTO),
            min_confidence_auto=0.7,
            min_samples_auto=5,
        )

        # State
        self._running = False
        self._pattern_task: Optional[asyncio.Task] = None
        self._trades_analyzed = 0
        self._patterns_detected = 0
        self._rules_generated = 0

    async def start(self) -> None:
        """Start the feedback service."""
        logger.info(f"Starting FeedbackService in {self.mode.value} mode")

        if not self.redis:
            self.redis = RedisBus()
            await self.redis.connect()

        self._running = True

        # Subscribe to closed trades
        await self.redis.subscribe(
            Channel.TRADES_CLOSED.value,
            self._handle_closed_trade,
        )

        # Start pattern detection loop
        self._pattern_task = asyncio.create_task(self._pattern_detection_loop())

        # Expire old rules on startup
        pool = await get_pool()
        await self.rule_generator.expire_old_rules(pool)

        # Publish current active rules on startup
        await self._publish_active_rules()

        logger.info(
            f"FeedbackService started: pattern_interval={self.pattern_check_interval}s, "
            f"lookback={self.lookback_hours}h"
        )

    async def stop(self) -> None:
        """Stop the feedback service."""
        logger.info("Stopping FeedbackService")
        self._running = False

        if self._pattern_task:
            self._pattern_task.cancel()
            try:
                await self._pattern_task
            except asyncio.CancelledError:
                pass

        logger.info(
            f"FeedbackService stopped: analyzed={self._trades_analyzed}, "
            f"patterns={self._patterns_detected}, rules={self._rules_generated}"
        )

    async def _handle_closed_trade(self, data: dict) -> None:
        """Handle a closed trade event."""
        try:
            outcome = data.get("outcome")
            pnl = data.get("pnl", 0)

            # Only analyze losses
            if outcome != "loss" and pnl >= 0:
                return

            trade_id = data.get("trade_id") or data.get("id", "unknown")
            logger.debug(f"Analyzing closed trade: {trade_id} (pnl={pnl})")

            pool = await get_pool()

            # Analyze the loss
            classification = await self.loss_analyzer.analyze_and_store(data, pool)
            self._trades_analyzed += 1

            # Publish analysis result
            if self.redis:
                await self.redis.publish(
                    Channel.FEEDBACK_LOSS_ANALYZED.value,
                    {
                        "trade_id": trade_id,
                        "classification": classification.to_dict(),
                        "analyzed_at": datetime.utcnow().isoformat(),
                    },
                )

            # If this is a critical root cause, trigger immediate pattern check
            if classification.root_cause in (
                RootCauseType.MODEL_ERROR,
                RootCauseType.EDGE_TOO_THIN,
            ):
                logger.info(
                    f"Critical root cause detected ({classification.root_cause.value}), "
                    "triggering pattern check"
                )
                asyncio.create_task(self._run_pattern_detection())

        except Exception as e:
            logger.error(f"Error handling closed trade: {e}", exc_info=True)

    async def _pattern_detection_loop(self) -> None:
        """Periodically run pattern detection."""
        while self._running:
            try:
                await asyncio.sleep(self.pattern_check_interval)
                await self._run_pattern_detection()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"Pattern detection error: {e}", exc_info=True)
                await asyncio.sleep(60)  # Back off on error

    async def _run_pattern_detection(self) -> None:
        """Run pattern detection and generate rules."""
        pool = await get_pool()

        # Detect patterns
        patterns = await self.pattern_detector.detect_all_patterns(
            pool,
            lookback_hours=self.lookback_hours,
        )

        if not patterns:
            logger.debug("No patterns detected")
            return

        logger.info(f"Detected {len(patterns)} patterns")
        self._patterns_detected += len(patterns)

        new_rules = []
        for pattern in patterns:
            # Store pattern
            await self.pattern_detector.store_pattern(pattern, pool)

            # Publish pattern detection
            if self.redis:
                await self.redis.publish(
                    Channel.FEEDBACK_PATTERN_DETECTED.value,
                    pattern.to_dict(),
                )

            # Generate rule if actionable
            rule = self.rule_generator.generate_from_pattern(pattern)
            if rule:
                await self.rule_generator.store_rule(rule, pool)
                await self.rule_generator.link_pattern_to_rule(pattern, rule, pool)
                new_rules.append(rule)
                self._rules_generated += 1

                logger.info(
                    f"Generated rule: {rule.rule_id} type={rule.rule_type.value} "
                    f"status={rule.status.value} conditions={rule.conditions}"
                )

        # Publish active rules to signal processor
        if new_rules:
            await self._publish_active_rules()

    async def _publish_active_rules(self) -> None:
        """Publish all active rules to the feedback:rules channel."""
        pool = await get_pool()
        rules = await self.rule_generator.get_active_rules(pool)

        if not rules:
            return

        # Only publish active rules (not pending approval in learning mode)
        active_rules = [r for r in rules if r.status == RuleStatus.ACTIVE]

        if self.redis and active_rules:
            await self.redis.publish(
                Channel.FEEDBACK_RULES.value,
                {
                    "type": "rules_update",
                    "rules": [r.to_dict() for r in active_rules],
                    "count": len(active_rules),
                    "published_at": datetime.utcnow().isoformat(),
                },
            )
            logger.info(f"Published {len(active_rules)} active rules")

    async def approve_rule(self, rule_id: str) -> bool:
        """Manually approve a pending rule."""
        pool = await get_pool()
        result = await pool.execute(
            """
            UPDATE trading_rules
            SET status = 'active'
            WHERE rule_id = $1 AND status = 'pending_approval'
            """,
            rule_id,
        )
        count = int(result.split()[-1]) if result else 0

        if count > 0:
            logger.info(f"Rule approved: {rule_id}")
            await self._publish_active_rules()
            return True
        return False

    async def deactivate_rule(self, rule_id: str) -> bool:
        """Deactivate an active rule."""
        pool = await get_pool()
        result = await pool.execute(
            """
            UPDATE trading_rules
            SET status = 'inactive'
            WHERE rule_id = $1 AND status = 'active'
            """,
            rule_id,
        )
        count = int(result.split()[-1]) if result else 0

        if count > 0:
            logger.info(f"Rule deactivated: {rule_id}")
            await self._publish_active_rules()
            return True
        return False

    def get_stats(self) -> dict:
        """Get feedback service statistics."""
        return {
            "mode": self.mode.value,
            "trades_analyzed": self._trades_analyzed,
            "patterns_detected": self._patterns_detected,
            "rules_generated": self._rules_generated,
            "pattern_check_interval": self.pattern_check_interval,
            "lookback_hours": self.lookback_hours,
        }
