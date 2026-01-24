"""
Pattern Detector - Finds systemic issues across losing trades.

Groups trades by various dimensions and identifies underperforming combos:
- Sport + signal type combinations
- Edge threshold ranges
- Game timing patterns
- Platform-specific issues

Uses Wilson confidence intervals for reliable pattern detection.
"""

import logging
import math
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from enum import Enum
from typing import Optional

logger = logging.getLogger(__name__)


class PatternType(str, Enum):
    """Types of patterns that can be detected."""
    SPORT_SIGNAL = "sport_signal"
    EDGE_BUCKET = "edge_bucket"
    TIMING_PATTERN = "timing_pattern"
    PLATFORM_ISSUE = "platform_issue"
    ROOT_CAUSE_CLUSTER = "root_cause_cluster"


@dataclass
class DetectedPattern:
    """A detected underperformance pattern."""
    pattern_id: str
    pattern_type: PatternType
    pattern_key: str  # e.g., "NFL:model_edge_yes" or "edge:<3%"
    description: str
    sample_size: int
    loss_count: int
    win_rate: float
    total_pnl: float
    conditions: dict
    suggested_action: Optional[dict] = None
    confidence: float = 0.0

    def to_dict(self) -> dict:
        return {
            "pattern_id": self.pattern_id,
            "pattern_type": self.pattern_type.value,
            "pattern_key": self.pattern_key,
            "description": self.description,
            "sample_size": self.sample_size,
            "loss_count": self.loss_count,
            "win_rate": self.win_rate,
            "total_pnl": self.total_pnl,
            "conditions": self.conditions,
            "suggested_action": self.suggested_action,
            "confidence": self.confidence,
        }


class PatternDetector:
    """
    Detects systemic underperformance patterns in trading data.

    Aggressive mode: reacts quickly with lower sample thresholds.
    """

    def __init__(
        self,
        min_samples_detect: int = 3,
        min_samples_act: int = 5,
        max_win_rate: float = 0.40,
        lookback_hours: int = 24,
        confidence_level: float = 0.95,
    ):
        self.min_samples_detect = min_samples_detect
        self.min_samples_act = min_samples_act
        self.max_win_rate = max_win_rate
        self.lookback_hours = lookback_hours
        self.confidence_level = confidence_level

    def _wilson_lower_bound(
        self,
        wins: int,
        total: int,
        z: float = 1.96,
    ) -> float:
        """
        Calculate Wilson score lower bound for win rate.

        This gives a conservative estimate of true win rate that accounts
        for sample size uncertainty.
        """
        if total == 0:
            return 0.0

        p = wins / total
        denominator = 1 + z**2 / total
        center = p + z**2 / (2 * total)
        spread = z * math.sqrt((p * (1 - p) + z**2 / (4 * total)) / total)

        return (center - spread) / denominator

    async def detect_sport_signal_patterns(
        self,
        pool,
        lookback_hours: Optional[int] = None,
    ) -> list[DetectedPattern]:
        """
        Find underperforming sport + signal type combinations.

        Query groups by (sport, signal_type) and finds combos with
        win rate below threshold.
        """
        hours = lookback_hours or self.lookback_hours
        cutoff = datetime.utcnow() - timedelta(hours=hours)

        rows = await pool.fetch(
            """
            SELECT
                sport,
                signal_type,
                COUNT(*) as trades,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                AVG(CASE WHEN outcome = 'win' THEN 1.0 ELSE 0.0 END) as win_rate,
                SUM(pnl) as total_pnl
            FROM paper_trades
            WHERE status = 'closed'
              AND time > $1
              AND outcome IN ('win', 'loss')
            GROUP BY sport, signal_type
            HAVING COUNT(*) >= $2
            """,
            cutoff,
            self.min_samples_detect,
        )

        patterns = []
        for row in rows:
            sport = row["sport"]
            signal_type = row["signal_type"]
            trades = row["trades"]
            wins = row["wins"]
            win_rate = float(row["win_rate"])
            total_pnl = float(row["total_pnl"])

            # Calculate Wilson lower bound for conservative estimate
            wilson_lower = self._wilson_lower_bound(wins, trades)

            # Only flag if upper bound of win rate is still below threshold
            if wilson_lower < self.max_win_rate and trades >= self.min_samples_detect:
                pattern_key = f"{sport}:{signal_type}"
                pattern_id = f"sport_signal_{pattern_key.replace(':', '_')}"

                confidence = min(1.0, trades / 20)  # Scales with sample size

                pattern = DetectedPattern(
                    pattern_id=pattern_id,
                    pattern_type=PatternType.SPORT_SIGNAL,
                    pattern_key=pattern_key,
                    description=f"{sport} {signal_type} underperforming: {win_rate*100:.0f}% win rate over {trades} trades",
                    sample_size=trades,
                    loss_count=trades - wins,
                    win_rate=win_rate,
                    total_pnl=total_pnl,
                    conditions={
                        "sport": sport,
                        "signal_type": signal_type,
                    },
                    suggested_action={
                        "type": "block_pattern" if trades >= self.min_samples_act else "monitor",
                        "reason": f"Low win rate ({win_rate*100:.0f}%)",
                    },
                    confidence=confidence,
                )
                patterns.append(pattern)

        return patterns

    async def detect_edge_threshold_patterns(
        self,
        pool,
        lookback_hours: Optional[int] = None,
    ) -> list[DetectedPattern]:
        """
        Find edge ranges that consistently lose.

        Groups trades by edge bucket and identifies thresholds that
        don't produce profitable results.
        """
        hours = lookback_hours or self.lookback_hours
        cutoff = datetime.utcnow() - timedelta(hours=hours)

        rows = await pool.fetch(
            """
            SELECT
                CASE
                    WHEN edge_at_entry < 2 THEN '<2%'
                    WHEN edge_at_entry < 3 THEN '2-3%'
                    WHEN edge_at_entry < 4 THEN '3-4%'
                    WHEN edge_at_entry < 5 THEN '4-5%'
                    ELSE '5%+'
                END as edge_bucket,
                COUNT(*) as trades,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                AVG(CASE WHEN outcome = 'win' THEN 1.0 ELSE 0.0 END) as win_rate,
                SUM(pnl) as total_pnl,
                MIN(edge_at_entry) as min_edge,
                MAX(edge_at_entry) as max_edge
            FROM paper_trades
            WHERE status = 'closed'
              AND time > $1
              AND outcome IN ('win', 'loss')
              AND edge_at_entry IS NOT NULL
            GROUP BY edge_bucket
            HAVING COUNT(*) >= $2
            ORDER BY min_edge
            """,
            cutoff,
            self.min_samples_detect,
        )

        patterns = []
        for row in rows:
            bucket = row["edge_bucket"]
            trades = row["trades"]
            wins = row["wins"]
            win_rate = float(row["win_rate"])
            total_pnl = float(row["total_pnl"])
            min_edge = float(row["min_edge"])
            max_edge = float(row["max_edge"])

            wilson_lower = self._wilson_lower_bound(wins, trades)

            if wilson_lower < self.max_win_rate and trades >= self.min_samples_detect:
                pattern_key = f"edge:{bucket}"
                pattern_id = f"edge_bucket_{bucket.replace('%', 'pct').replace('<', 'lt').replace('-', '_')}"

                confidence = min(1.0, trades / 20)

                # Suggest raising minimum edge if low buckets are losing
                suggested_min_edge = None
                if bucket in ("<2%", "2-3%"):
                    suggested_min_edge = 4.0
                elif bucket == "3-4%":
                    suggested_min_edge = 5.0

                pattern = DetectedPattern(
                    pattern_id=pattern_id,
                    pattern_type=PatternType.EDGE_BUCKET,
                    pattern_key=pattern_key,
                    description=f"Edge {bucket} underperforming: {win_rate*100:.0f}% win rate, ${total_pnl:.2f} P&L",
                    sample_size=trades,
                    loss_count=trades - wins,
                    win_rate=win_rate,
                    total_pnl=total_pnl,
                    conditions={
                        "edge_lt": max_edge,
                        "edge_gte": min_edge,
                    },
                    suggested_action={
                        "type": "threshold_override" if suggested_min_edge else "monitor",
                        "new_min_edge": suggested_min_edge,
                        "reason": f"Edge bucket {bucket} losing",
                    },
                    confidence=confidence,
                )
                patterns.append(pattern)

        return patterns

    async def detect_timing_patterns(
        self,
        pool,
        lookback_hours: Optional[int] = None,
    ) -> list[DetectedPattern]:
        """
        Find game periods that correlate with losses.

        E.g., Q4 trades might be more volatile and unprofitable.
        """
        hours = lookback_hours or self.lookback_hours
        cutoff = datetime.utcnow() - timedelta(hours=hours)

        rows = await pool.fetch(
            """
            SELECT
                game_period,
                COUNT(*) as trades,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                AVG(CASE WHEN outcome = 'win' THEN 1.0 ELSE 0.0 END) as win_rate,
                SUM(pnl) as total_pnl
            FROM paper_trades
            WHERE status = 'closed'
              AND time > $1
              AND outcome IN ('win', 'loss')
              AND game_period IS NOT NULL
            GROUP BY game_period
            HAVING COUNT(*) >= $2
            """,
            cutoff,
            self.min_samples_detect,
        )

        patterns = []
        for row in rows:
            period = row["game_period"]
            trades = row["trades"]
            wins = row["wins"]
            win_rate = float(row["win_rate"])
            total_pnl = float(row["total_pnl"])

            wilson_lower = self._wilson_lower_bound(wins, trades)

            if wilson_lower < self.max_win_rate and trades >= self.min_samples_detect:
                pattern_key = f"period:{period}"
                pattern_id = f"timing_{period.replace(' ', '_').lower()}"

                confidence = min(1.0, trades / 20)

                pattern = DetectedPattern(
                    pattern_id=pattern_id,
                    pattern_type=PatternType.TIMING_PATTERN,
                    pattern_key=pattern_key,
                    description=f"Period {period} underperforming: {win_rate*100:.0f}% win rate",
                    sample_size=trades,
                    loss_count=trades - wins,
                    win_rate=win_rate,
                    total_pnl=total_pnl,
                    conditions={
                        "game_period": period,
                    },
                    suggested_action={
                        "type": "block_pattern" if trades >= self.min_samples_act else "monitor",
                        "reason": f"Period {period} losses",
                    },
                    confidence=confidence,
                )
                patterns.append(pattern)

        return patterns

    async def detect_root_cause_clusters(
        self,
        pool,
        lookback_hours: Optional[int] = None,
    ) -> list[DetectedPattern]:
        """
        Find clusters of losses with the same root cause.

        Uses the loss_analysis table to find recurring issues.
        """
        hours = lookback_hours or self.lookback_hours
        cutoff = datetime.utcnow() - timedelta(hours=hours)

        rows = await pool.fetch(
            """
            SELECT
                root_cause,
                sub_cause,
                sport,
                signal_type,
                COUNT(*) as occurrences,
                AVG(edge_at_entry) as avg_edge
            FROM loss_analysis
            WHERE analyzed_at > $1
            GROUP BY root_cause, sub_cause, sport, signal_type
            HAVING COUNT(*) >= $2
            ORDER BY occurrences DESC
            """,
            cutoff,
            self.min_samples_detect,
        )

        patterns = []
        for row in rows:
            cause = row["root_cause"]
            sub = row["sub_cause"]
            sport = row["sport"]
            signal_type = row["signal_type"]
            count = row["occurrences"]
            avg_edge = float(row["avg_edge"]) if row["avg_edge"] else 0

            pattern_key = f"cause:{cause}:{sport}:{signal_type}"
            pattern_id = f"cause_{cause}_{sport}_{signal_type}".replace(":", "_")

            confidence = min(1.0, count / 10)

            pattern = DetectedPattern(
                pattern_id=pattern_id,
                pattern_type=PatternType.ROOT_CAUSE_CLUSTER,
                pattern_key=pattern_key,
                description=f"{cause} ({sub}) recurring in {sport} {signal_type}: {count} losses",
                sample_size=count,
                loss_count=count,
                win_rate=0.0,  # All losses by definition
                total_pnl=0.0,
                conditions={
                    "root_cause": cause,
                    "sport": sport,
                    "signal_type": signal_type,
                },
                suggested_action={
                    "type": "investigate",
                    "reason": f"Root cause {cause} clustering",
                    "avg_edge": avg_edge,
                },
                confidence=confidence,
            )
            patterns.append(pattern)

        return patterns

    async def detect_all_patterns(
        self,
        pool,
        lookback_hours: Optional[int] = None,
    ) -> list[DetectedPattern]:
        """
        Run all pattern detection methods and return combined results.
        """
        all_patterns = []

        sport_patterns = await self.detect_sport_signal_patterns(pool, lookback_hours)
        all_patterns.extend(sport_patterns)

        edge_patterns = await self.detect_edge_threshold_patterns(pool, lookback_hours)
        all_patterns.extend(edge_patterns)

        timing_patterns = await self.detect_timing_patterns(pool, lookback_hours)
        all_patterns.extend(timing_patterns)

        cause_patterns = await self.detect_root_cause_clusters(pool, lookback_hours)
        all_patterns.extend(cause_patterns)

        # Sort by loss count (most impactful first)
        all_patterns.sort(key=lambda p: p.loss_count, reverse=True)

        return all_patterns

    async def store_pattern(
        self,
        pattern: DetectedPattern,
        pool,
    ) -> None:
        """Store or update a detected pattern in the database."""
        try:
            await pool.execute(
                """
                INSERT INTO detected_patterns (
                    pattern_id, pattern_type, pattern_key, description,
                    sample_size, loss_count, win_rate, total_pnl,
                    conditions, suggested_action, status,
                    first_detected_at, last_updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'active', NOW(), NOW())
                ON CONFLICT (pattern_key) DO UPDATE SET
                    sample_size = EXCLUDED.sample_size,
                    loss_count = EXCLUDED.loss_count,
                    win_rate = EXCLUDED.win_rate,
                    total_pnl = EXCLUDED.total_pnl,
                    suggested_action = EXCLUDED.suggested_action,
                    last_updated_at = NOW()
                """,
                pattern.pattern_id,
                pattern.pattern_type.value,
                pattern.pattern_key,
                pattern.description,
                pattern.sample_size,
                pattern.loss_count,
                pattern.win_rate,
                pattern.total_pnl,
                pattern.conditions,
                pattern.suggested_action,
            )
            logger.info(f"Pattern stored/updated: {pattern.pattern_key}")
        except Exception as e:
            logger.error(f"Failed to store pattern: {e}")
