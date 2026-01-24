"""
Rule Generator - Converts detected patterns into actionable trading rules.

Rule types:
- block_pattern: Reject signals matching conditions
- threshold_override: Increase min_edge for specific conditions

Rules auto-expire based on confidence (24h-7d).
Broad rules (blocking entire sport) require approval.
"""

import logging
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from enum import Enum
from typing import Optional
from uuid import uuid4

from .pattern_detector import DetectedPattern, PatternType

logger = logging.getLogger(__name__)


class RuleType(str, Enum):
    """Types of trading rules."""
    BLOCK_PATTERN = "block_pattern"
    THRESHOLD_OVERRIDE = "threshold_override"


class RuleStatus(str, Enum):
    """Rule lifecycle status."""
    ACTIVE = "active"
    PENDING_APPROVAL = "pending_approval"
    INACTIVE = "inactive"
    EXPIRED = "expired"


@dataclass
class TradingRule:
    """A trading rule that blocks or modifies signal processing."""
    rule_id: str
    rule_type: RuleType
    conditions: dict  # Matching conditions
    action: dict  # What to do when matched
    source: str = "automated"
    confidence: float = 0.5
    sample_size: int = 0
    reason: Optional[str] = None
    status: RuleStatus = RuleStatus.ACTIVE
    created_at: datetime = field(default_factory=datetime.utcnow)
    expires_at: Optional[datetime] = None
    match_count: int = 0

    def to_dict(self) -> dict:
        return {
            "rule_id": self.rule_id,
            "rule_type": self.rule_type.value,
            "conditions": self.conditions,
            "action": self.action,
            "source": self.source,
            "confidence": self.confidence,
            "sample_size": self.sample_size,
            "reason": self.reason,
            "status": self.status.value,
            "created_at": self.created_at.isoformat() if self.created_at else None,
            "expires_at": self.expires_at.isoformat() if self.expires_at else None,
            "match_count": self.match_count,
        }

    def matches(self, signal: dict) -> bool:
        """Check if a signal matches this rule's conditions."""
        for key, value in self.conditions.items():
            signal_value = signal.get(key)

            # Handle comparison operators in key
            if key.endswith("_lt"):
                actual_key = key[:-3]
                signal_value = signal.get(actual_key)
                if signal_value is None or signal_value >= value:
                    return False
            elif key.endswith("_lte"):
                actual_key = key[:-4]
                signal_value = signal.get(actual_key)
                if signal_value is None or signal_value > value:
                    return False
            elif key.endswith("_gt"):
                actual_key = key[:-3]
                signal_value = signal.get(actual_key)
                if signal_value is None or signal_value <= value:
                    return False
            elif key.endswith("_gte"):
                actual_key = key[:-4]
                signal_value = signal.get(actual_key)
                if signal_value is None or signal_value < value:
                    return False
            else:
                # Exact match
                if signal_value != value:
                    return False

        return True


class RuleGenerator:
    """
    Generates trading rules from detected patterns.

    Modes:
    - Learning: Suggest rules but require approval
    - Auto: Auto-apply rules with clear solutions
    """

    # How long rules last based on confidence
    EXPIRY_MAP = {
        0.9: timedelta(days=7),
        0.7: timedelta(days=3),
        0.5: timedelta(days=1),
    }

    # Patterns that require manual approval (too broad)
    BROAD_PATTERNS = {
        "NFL", "NBA", "NHL", "MLB", "NCAAB", "NCAAF",
        "MLS", "SOCCER", "TENNIS", "MMA",
    }

    def __init__(
        self,
        auto_approve: bool = False,
        min_confidence_auto: float = 0.7,
        min_samples_auto: int = 5,
    ):
        self.auto_approve = auto_approve
        self.min_confidence_auto = min_confidence_auto
        self.min_samples_auto = min_samples_auto

    def _calculate_expiry(self, confidence: float) -> datetime:
        """Calculate rule expiry based on confidence."""
        for threshold, delta in sorted(self.EXPIRY_MAP.items(), reverse=True):
            if confidence >= threshold:
                return datetime.utcnow() + delta
        return datetime.utcnow() + timedelta(hours=24)

    def _is_broad_rule(self, conditions: dict) -> bool:
        """Check if rule is too broad and needs approval."""
        # Blocking an entire sport is broad
        if len(conditions) == 1:
            key = list(conditions.keys())[0]
            value = conditions[key]
            if key == "sport" and value in self.BROAD_PATTERNS:
                return True

        return False

    def generate_from_pattern(
        self,
        pattern: DetectedPattern,
    ) -> Optional[TradingRule]:
        """
        Generate a trading rule from a detected pattern.

        Returns None if no actionable rule can be generated.
        """
        suggested = pattern.suggested_action
        if not suggested:
            return None

        action_type = suggested.get("type")
        if action_type == "monitor":
            # Not actionable yet
            return None

        rule_id = f"rule_{pattern.pattern_id}_{uuid4().hex[:8]}"
        conditions = dict(pattern.conditions)
        confidence = pattern.confidence

        # Determine if this needs approval
        needs_approval = self._is_broad_rule(conditions)
        if not self.auto_approve and pattern.sample_size < self.min_samples_auto:
            needs_approval = True
        if not self.auto_approve and confidence < self.min_confidence_auto:
            needs_approval = True

        status = RuleStatus.PENDING_APPROVAL if needs_approval else RuleStatus.ACTIVE

        if action_type == "block_pattern":
            rule = TradingRule(
                rule_id=rule_id,
                rule_type=RuleType.BLOCK_PATTERN,
                conditions=conditions,
                action={
                    "type": "reject",
                    "reason": suggested.get("reason", "Pattern block"),
                },
                source="automated",
                confidence=confidence,
                sample_size=pattern.sample_size,
                reason=pattern.description,
                status=status,
                expires_at=self._calculate_expiry(confidence),
            )
            return rule

        elif action_type == "threshold_override":
            new_min_edge = suggested.get("new_min_edge")
            if not new_min_edge:
                return None

            rule = TradingRule(
                rule_id=rule_id,
                rule_type=RuleType.THRESHOLD_OVERRIDE,
                conditions=conditions,
                action={
                    "type": "override",
                    "min_edge_pct": new_min_edge,
                    "reason": suggested.get("reason", "Edge threshold increase"),
                },
                source="automated",
                confidence=confidence,
                sample_size=pattern.sample_size,
                reason=pattern.description,
                status=status,
                expires_at=self._calculate_expiry(confidence),
            )
            return rule

        elif action_type == "investigate":
            # Create a monitoring rule that alerts but doesn't block
            rule = TradingRule(
                rule_id=rule_id,
                rule_type=RuleType.BLOCK_PATTERN,
                conditions=conditions,
                action={
                    "type": "alert",
                    "reason": suggested.get("reason", "Requires investigation"),
                },
                source="automated",
                confidence=confidence,
                sample_size=pattern.sample_size,
                reason=pattern.description,
                status=RuleStatus.PENDING_APPROVAL,  # Always needs approval
                expires_at=self._calculate_expiry(0.5),
            )
            return rule

        return None

    async def store_rule(
        self,
        rule: TradingRule,
        pool,
    ) -> None:
        """Store a trading rule in the database."""
        try:
            await pool.execute(
                """
                INSERT INTO trading_rules (
                    rule_id, rule_type, conditions, action, source,
                    confidence, sample_size, reason, status,
                    created_at, expires_at, match_count
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                ON CONFLICT (rule_id) DO UPDATE SET
                    conditions = EXCLUDED.conditions,
                    action = EXCLUDED.action,
                    confidence = EXCLUDED.confidence,
                    sample_size = EXCLUDED.sample_size,
                    reason = EXCLUDED.reason,
                    status = EXCLUDED.status,
                    expires_at = EXCLUDED.expires_at
                """,
                rule.rule_id,
                rule.rule_type.value,
                rule.conditions,
                rule.action,
                rule.source,
                rule.confidence,
                rule.sample_size,
                rule.reason,
                rule.status.value,
                rule.created_at,
                rule.expires_at,
                rule.match_count,
            )
            logger.info(
                f"Rule stored: {rule.rule_id} type={rule.rule_type.value} "
                f"status={rule.status.value}"
            )
        except Exception as e:
            logger.error(f"Failed to store rule: {e}")

    async def link_pattern_to_rule(
        self,
        pattern: DetectedPattern,
        rule: TradingRule,
        pool,
    ) -> None:
        """Link a pattern to the rule generated from it."""
        try:
            await pool.execute(
                """
                UPDATE detected_patterns
                SET rule_id = $1
                WHERE pattern_id = $2
                """,
                rule.rule_id,
                pattern.pattern_id,
            )
        except Exception as e:
            logger.error(f"Failed to link pattern to rule: {e}")

    async def get_active_rules(self, pool) -> list[TradingRule]:
        """Get all active trading rules from the database."""
        rows = await pool.fetch(
            """
            SELECT rule_id, rule_type, conditions, action, source,
                   confidence, sample_size, reason, status,
                   created_at, expires_at, match_count
            FROM trading_rules
            WHERE status = 'active'
              AND (expires_at IS NULL OR expires_at > NOW())
            ORDER BY created_at DESC
            """
        )

        rules = []
        for row in rows:
            rule = TradingRule(
                rule_id=row["rule_id"],
                rule_type=RuleType(row["rule_type"]),
                conditions=row["conditions"],
                action=row["action"],
                source=row["source"],
                confidence=float(row["confidence"]) if row["confidence"] else 0.5,
                sample_size=row["sample_size"] or 0,
                reason=row["reason"],
                status=RuleStatus(row["status"]),
                created_at=row["created_at"],
                expires_at=row["expires_at"],
                match_count=row["match_count"] or 0,
            )
            rules.append(rule)

        return rules

    async def expire_old_rules(self, pool) -> int:
        """Mark expired rules as expired and return count."""
        result = await pool.execute(
            """
            UPDATE trading_rules
            SET status = 'expired'
            WHERE status = 'active'
              AND expires_at IS NOT NULL
              AND expires_at <= NOW()
            """
        )
        # Extract count from result string like "UPDATE 5"
        count = int(result.split()[-1]) if result else 0
        if count > 0:
            logger.info(f"Expired {count} trading rules")
        return count

    async def increment_match_count(self, rule_id: str, pool) -> None:
        """Increment the match count for a rule."""
        await pool.execute(
            """
            UPDATE trading_rules
            SET match_count = match_count + 1
            WHERE rule_id = $1
            """,
            rule_id,
        )
