"""
Rule Evaluator - Evaluates signals against active trading rules.

Consumes rules from the feedback:rules channel and caches them
for fast evaluation in the signal processing pipeline.
"""

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Optional

from arbees_shared.db.connection import get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.signal import TradingSignal

logger = logging.getLogger(__name__)


class RuleDecisionType(str, Enum):
    """Type of decision from rule evaluation."""
    ALLOWED = "allowed"
    REJECTED = "rejected"
    THRESHOLD_OVERRIDE = "threshold_override"


@dataclass
class RuleDecision:
    """Result of evaluating a signal against rules."""
    allowed: bool
    decision_type: RuleDecisionType
    rule_id: Optional[str] = None
    reason: Optional[str] = None
    override_min_edge: Optional[float] = None

    def to_dict(self) -> dict:
        return {
            "allowed": self.allowed,
            "decision_type": self.decision_type.value,
            "rule_id": self.rule_id,
            "reason": self.reason,
            "override_min_edge": self.override_min_edge,
        }


@dataclass
class CachedRule:
    """In-memory cached trading rule."""
    rule_id: str
    rule_type: str  # block_pattern, threshold_override
    conditions: dict
    action: dict
    expires_at: Optional[datetime]

    def is_expired(self) -> bool:
        if self.expires_at is None:
            return False
        return datetime.utcnow() > self.expires_at

    def matches(self, signal_dict: dict) -> bool:
        """Check if signal matches this rule's conditions."""
        for key, value in self.conditions.items():
            signal_value = signal_dict.get(key)

            # Handle comparison operators in key
            if key.endswith("_lt"):
                actual_key = key[:-3]
                signal_value = signal_dict.get(actual_key)
                if signal_value is None or signal_value >= value:
                    return False
            elif key.endswith("_lte"):
                actual_key = key[:-4]
                signal_value = signal_dict.get(actual_key)
                if signal_value is None or signal_value > value:
                    return False
            elif key.endswith("_gt"):
                actual_key = key[:-3]
                signal_value = signal_dict.get(actual_key)
                if signal_value is None or signal_value <= value:
                    return False
            elif key.endswith("_gte"):
                actual_key = key[:-4]
                signal_value = signal_dict.get(actual_key)
                if signal_value is None or signal_value < value:
                    return False
            else:
                # Exact match (case-insensitive for strings)
                if isinstance(value, str) and isinstance(signal_value, str):
                    if signal_value.lower() != value.lower():
                        return False
                elif signal_value != value:
                    return False

        return True


class RuleEvaluator:
    """
    Evaluates trading signals against active rules from the feedback loop.

    Subscribes to feedback:rules channel to receive rule updates.
    Caches rules in memory for fast evaluation.
    """

    def __init__(
        self,
        redis: Optional[RedisBus] = None,
        load_from_db: bool = True,
    ):
        self.redis = redis
        self.load_from_db = load_from_db

        # Cached rules by rule_id
        self._rules: dict[str, CachedRule] = {}
        self._last_update: Optional[datetime] = None
        self._rules_lock = asyncio.Lock()

    async def start(self, redis: Optional[RedisBus] = None) -> None:
        """Start the rule evaluator."""
        if redis:
            self.redis = redis

        # Load initial rules from database
        if self.load_from_db:
            await self._load_rules_from_db()

        # Subscribe to rule updates
        if self.redis:
            await self.redis.subscribe(
                Channel.FEEDBACK_RULES.value,
                self._handle_rules_update,
            )

        logger.info(f"RuleEvaluator started with {len(self._rules)} rules")

    async def stop(self) -> None:
        """Stop the rule evaluator."""
        logger.info("RuleEvaluator stopped")

    async def _load_rules_from_db(self) -> None:
        """Load active rules from the database."""
        try:
            pool = await get_pool()
            rows = await pool.fetch(
                """
                SELECT rule_id, rule_type, conditions, action, expires_at
                FROM trading_rules
                WHERE status = 'active'
                  AND (expires_at IS NULL OR expires_at > NOW())
                """
            )

            async with self._rules_lock:
                self._rules.clear()
                for row in rows:
                    rule = CachedRule(
                        rule_id=row["rule_id"],
                        rule_type=row["rule_type"],
                        conditions=row["conditions"],
                        action=row["action"],
                        expires_at=row["expires_at"],
                    )
                    self._rules[rule.rule_id] = rule
                self._last_update = datetime.utcnow()

            logger.info(f"Loaded {len(self._rules)} rules from database")

        except Exception as e:
            logger.error(f"Failed to load rules from database: {e}")

    async def _handle_rules_update(self, data: dict) -> None:
        """Handle rules update from feedback service."""
        try:
            msg_type = data.get("type")
            if msg_type != "rules_update":
                return

            rules_data = data.get("rules", [])

            async with self._rules_lock:
                self._rules.clear()
                for rule_data in rules_data:
                    expires_str = rule_data.get("expires_at")
                    expires_at = None
                    if expires_str:
                        try:
                            expires_at = datetime.fromisoformat(
                                expires_str.replace("Z", "+00:00")
                            )
                        except:
                            pass

                    rule = CachedRule(
                        rule_id=rule_data["rule_id"],
                        rule_type=rule_data["rule_type"],
                        conditions=rule_data.get("conditions", {}),
                        action=rule_data.get("action", {}),
                        expires_at=expires_at,
                    )
                    self._rules[rule.rule_id] = rule

                self._last_update = datetime.utcnow()

            logger.info(
                f"Rules updated: {len(self._rules)} active rules "
                f"(from {data.get('published_at', 'unknown')})"
            )

        except Exception as e:
            logger.error(f"Error handling rules update: {e}")

    async def evaluate(self, signal: TradingSignal) -> RuleDecision:
        """
        Evaluate a signal against all active rules.

        Returns the decision (allowed, rejected, or override).
        """
        # Convert signal to dict for matching
        signal_dict = {
            "sport": signal.sport.value if hasattr(signal.sport, "value") else str(signal.sport),
            "signal_type": signal.signal_type.value if hasattr(signal.signal_type, "value") else str(signal.signal_type),
            "direction": signal.direction.value if hasattr(signal.direction, "value") else str(signal.direction),
            "edge_pct": signal.edge_pct,
            "model_prob": signal.model_prob,
            "market_prob": signal.market_prob,
            "team": signal.team,
            "game_id": signal.game_id,
        }

        # Add edge as alias for edge_pct
        signal_dict["edge"] = signal_dict["edge_pct"]

        # Track best threshold override
        best_override: Optional[float] = None
        override_rule_id: Optional[str] = None

        async with self._rules_lock:
            # Remove expired rules
            expired = [rid for rid, r in self._rules.items() if r.is_expired()]
            for rid in expired:
                del self._rules[rid]

            # Evaluate against each rule
            for rule_id, rule in self._rules.items():
                if not rule.matches(signal_dict):
                    continue

                action_type = rule.action.get("type")

                if action_type == "reject":
                    # Log match for analytics
                    asyncio.create_task(
                        self._log_rule_match(rule_id, signal)
                    )
                    return RuleDecision(
                        allowed=False,
                        decision_type=RuleDecisionType.REJECTED,
                        rule_id=rule_id,
                        reason=rule.action.get("reason", "Rule blocked"),
                    )

                elif action_type == "override":
                    # Threshold override - collect highest
                    min_edge = rule.action.get("min_edge_pct", 0)
                    if min_edge > (best_override or 0):
                        best_override = min_edge
                        override_rule_id = rule_id

        # If we have a threshold override, apply it
        if best_override is not None:
            if signal.edge_pct < best_override:
                asyncio.create_task(
                    self._log_rule_match(override_rule_id, signal)
                )
                return RuleDecision(
                    allowed=False,
                    decision_type=RuleDecisionType.THRESHOLD_OVERRIDE,
                    rule_id=override_rule_id,
                    reason=f"Edge {signal.edge_pct:.1f}% below override threshold {best_override:.1f}%",
                    override_min_edge=best_override,
                )

        return RuleDecision(
            allowed=True,
            decision_type=RuleDecisionType.ALLOWED,
        )

    async def _log_rule_match(
        self,
        rule_id: str,
        signal: TradingSignal,
    ) -> None:
        """Log a rule match for analytics."""
        try:
            pool = await get_pool()
            await pool.execute(
                """
                INSERT INTO rule_matches (
                    rule_id, signal_id, game_id, sport, signal_type, matched_at
                ) VALUES ($1, $2, $3, $4, $5, NOW())
                """,
                rule_id,
                signal.signal_id,
                signal.game_id,
                signal.sport.value if hasattr(signal.sport, "value") else str(signal.sport),
                signal.signal_type.value if hasattr(signal.signal_type, "value") else str(signal.signal_type),
            )

            # Increment match count on the rule
            await pool.execute(
                """
                UPDATE trading_rules
                SET match_count = match_count + 1
                WHERE rule_id = $1
                """,
                rule_id,
            )

        except Exception as e:
            logger.debug(f"Failed to log rule match: {e}")

    def get_rule_count(self) -> int:
        """Get the number of cached rules."""
        return len(self._rules)

    def get_last_update(self) -> Optional[datetime]:
        """Get the timestamp of the last rules update."""
        return self._last_update
