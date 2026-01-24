"""
Loss Analyzer - Root cause classification for individual losing trades.

Classifies each loss into categories:
- edge_too_thin: Fees/slippage consumed edge
- model_error: Model probability was wrong
- market_speed: Market moved faster than execution
- liquidity_issue: Bad exit price due to thin books
- timing_pattern: Loss correlates with game period
- sport_underperformance: Sport/signal combo losing
"""

import logging
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Optional

logger = logging.getLogger(__name__)


class RootCauseType(str, Enum):
    """Root cause categories for losing trades."""
    EDGE_TOO_THIN = "edge_too_thin"
    MODEL_ERROR = "model_error"
    MARKET_SPEED = "market_speed"
    LIQUIDITY_ISSUE = "liquidity_issue"
    TIMING_PATTERN = "timing_pattern"
    SPORT_UNDERPERFORMANCE = "sport_underperformance"
    UNKNOWN = "unknown"


@dataclass
class LossRootCause:
    """Classification result for a losing trade."""
    root_cause: RootCauseType
    sub_cause: Optional[str] = None
    confidence: float = 0.5
    details: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "root_cause": self.root_cause.value,
            "sub_cause": self.sub_cause,
            "confidence": self.confidence,
            "details": self.details,
        }


class LossAnalyzer:
    """
    Analyzes individual losing trades to determine root causes.

    Used to classify losses for pattern detection and rule generation.
    """

    def __init__(
        self,
        fee_edge_ratio_threshold: float = 0.5,
        high_confidence_threshold: float = 0.70,
        rapid_reversal_seconds: float = 60.0,
        thin_book_threshold: float = 100.0,
    ):
        self.fee_edge_ratio_threshold = fee_edge_ratio_threshold
        self.high_confidence_threshold = high_confidence_threshold
        self.rapid_reversal_seconds = rapid_reversal_seconds
        self.thin_book_threshold = thin_book_threshold

    def classify(self, trade: dict) -> LossRootCause:
        """
        Classify the root cause of a losing trade.

        Args:
            trade: Trade dict with keys like:
                - edge_at_entry: Edge % at entry
                - entry_fees, exit_fees: Fee amounts
                - size: Position size
                - model_prob: Model probability at entry
                - outcome: 'win' or 'loss'
                - holding_time_seconds: How long position was held
                - exit_liquidity: Liquidity at exit
                - sport, signal_type: Classification info
                - game_period: e.g. "Q4", "2H", etc
                - pnl: Profit/loss amount

        Returns:
            LossRootCause with classification and confidence
        """
        edge = trade.get("edge_at_entry") or trade.get("edge_pct") or 0
        size = trade.get("size", 1.0)
        entry_fees = trade.get("entry_fees", 0)
        exit_fees = trade.get("exit_fees", 0)
        total_fees = entry_fees + exit_fees
        model_prob = trade.get("model_prob", 0.5)
        outcome = trade.get("outcome", "loss")
        holding_time = trade.get("holding_time_seconds", 0)
        exit_liquidity = trade.get("exit_liquidity") or trade.get("liquidity", 0)
        game_period = trade.get("game_period")
        pnl = trade.get("pnl", 0)

        # Skip if not a loss
        if outcome == "win" or pnl >= 0:
            return LossRootCause(
                root_cause=RootCauseType.UNKNOWN,
                sub_cause="not_a_loss",
                confidence=1.0,
                details={"pnl": pnl, "outcome": outcome}
            )

        # Calculate fee impact
        fees_pct = (total_fees / size * 100) if size > 0 else 0

        # 1. Edge erosion - fees ate more than threshold of edge
        if edge > 0 and fees_pct > edge * self.fee_edge_ratio_threshold:
            return LossRootCause(
                root_cause=RootCauseType.EDGE_TOO_THIN,
                sub_cause="fees_exceeded_edge",
                confidence=0.9,
                details={
                    "edge_at_entry": edge,
                    "fees_pct": fees_pct,
                    "fee_edge_ratio": fees_pct / edge if edge > 0 else 0,
                }
            )

        # 2. Model error - high confidence but wrong
        if model_prob > self.high_confidence_threshold:
            return LossRootCause(
                root_cause=RootCauseType.MODEL_ERROR,
                sub_cause="high_confidence_miss",
                confidence=0.85,
                details={
                    "model_prob": model_prob,
                    "threshold": self.high_confidence_threshold,
                }
            )

        # 3. Market speed - rapid reversal after entry
        if holding_time < self.rapid_reversal_seconds:
            return LossRootCause(
                root_cause=RootCauseType.MARKET_SPEED,
                sub_cause="rapid_reversal",
                confidence=0.75,
                details={
                    "holding_time_seconds": holding_time,
                    "threshold_seconds": self.rapid_reversal_seconds,
                }
            )

        # 4. Liquidity issue - thin books at exit
        if exit_liquidity > 0 and exit_liquidity < self.thin_book_threshold:
            return LossRootCause(
                root_cause=RootCauseType.LIQUIDITY_ISSUE,
                sub_cause="thin_exit_book",
                confidence=0.70,
                details={
                    "exit_liquidity": exit_liquidity,
                    "threshold": self.thin_book_threshold,
                }
            )

        # 5. Timing pattern - late game losses
        late_periods = {"Q4", "4Q", "2H", "OT", "P3", "3P", "9th", "9"}
        if game_period and any(p in str(game_period).upper() for p in late_periods):
            return LossRootCause(
                root_cause=RootCauseType.TIMING_PATTERN,
                sub_cause=f"late_game_{game_period}",
                confidence=0.60,
                details={"game_period": game_period}
            )

        # 6. Default to sport/signal underperformance for pattern analysis
        return LossRootCause(
            root_cause=RootCauseType.SPORT_UNDERPERFORMANCE,
            sub_cause="pattern_check_needed",
            confidence=0.5,
            details={
                "sport": trade.get("sport"),
                "signal_type": trade.get("signal_type"),
                "edge_at_entry": edge,
            }
        )

    async def analyze_and_store(
        self,
        trade: dict,
        pool,  # asyncpg pool
    ) -> LossRootCause:
        """
        Analyze a trade and store the result in loss_analysis table.

        Args:
            trade: Trade dict
            pool: Database connection pool

        Returns:
            LossRootCause classification
        """
        classification = self.classify(trade)

        if classification.root_cause == RootCauseType.UNKNOWN:
            return classification

        trade_id = trade.get("trade_id") or trade.get("id", "unknown")

        try:
            await pool.execute(
                """
                INSERT INTO loss_analysis (
                    trade_id, root_cause, sub_cause, confidence,
                    sport, signal_type, edge_at_entry, details, analyzed_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                """,
                trade_id,
                classification.root_cause.value,
                classification.sub_cause,
                classification.confidence,
                trade.get("sport"),
                trade.get("signal_type"),
                trade.get("edge_at_entry") or trade.get("edge_pct"),
                classification.details,
                datetime.utcnow(),
            )
            logger.info(
                f"Loss analyzed: trade={trade_id} cause={classification.root_cause.value} "
                f"sub={classification.sub_cause} conf={classification.confidence:.2f}"
            )
        except Exception as e:
            logger.error(f"Failed to store loss analysis: {e}")

        return classification
