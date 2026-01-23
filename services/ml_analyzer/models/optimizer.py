"""
Parameter Optimizer for trading strategy tuning.

Analyzes historical performance to suggest optimal parameter values
for edge thresholds, position sizing, and risk management.
"""

import logging
from dataclasses import dataclass
from typing import Optional

from ..feature_extractor import TradeFeatures

logger = logging.getLogger(__name__)


@dataclass
class OptimizationResult:
    """Result of a parameter optimization."""
    parameter: str
    current_value: float
    optimal_value: float
    expected_improvement: float
    confidence: str  # "low", "medium", "high"
    rationale: str
    samples_analyzed: int


class ParameterOptimizer:
    """
    Optimizes trading parameters based on historical performance.

    Analyzes trade data to find optimal values for:
    - Minimum edge threshold
    - Kelly fraction / position sizing
    - Stop loss thresholds
    - Take profit thresholds

    Uses simple statistical analysis rather than complex optimization
    to provide interpretable recommendations.
    """

    # Edge thresholds to test
    EDGE_THRESHOLDS = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0]

    # Kelly fractions to test
    KELLY_FRACTIONS = [0.10, 0.15, 0.20, 0.25, 0.30, 0.35]

    def __init__(self):
        """Initialize the optimizer."""
        pass

    def optimize_edge_threshold(
        self,
        features: list[TradeFeatures],
        current_threshold: float = 2.0,
    ) -> OptimizationResult:
        """
        Find optimal minimum edge threshold.

        Tests different edge thresholds and finds the one that maximizes
        expected profit while maintaining acceptable win rate.

        Args:
            features: List of trade features with outcomes
            current_threshold: Current edge threshold setting

        Returns:
            OptimizationResult with recommendation
        """
        if len(features) < 20:
            return OptimizationResult(
                parameter="min_edge_pct",
                current_value=current_threshold,
                optimal_value=current_threshold,
                expected_improvement=0,
                confidence="low",
                rationale="Insufficient data for optimization",
                samples_analyzed=len(features),
            )

        # Analyze performance at each threshold
        threshold_performance = {}

        for threshold in self.EDGE_THRESHOLDS:
            # Filter trades above threshold
            filtered = [f for f in features if f.edge_at_entry >= threshold]

            if len(filtered) < 10:
                continue

            wins = sum(1 for f in filtered if f.outcome == 1)
            total = len(filtered)
            pnl = sum(f.pnl for f in filtered)
            win_rate = wins / total if total > 0 else 0

            # Calculate expected value per trade
            ev_per_trade = pnl / total if total > 0 else 0

            threshold_performance[threshold] = {
                "trades": total,
                "win_rate": win_rate,
                "pnl": pnl,
                "ev_per_trade": ev_per_trade,
            }

        if not threshold_performance:
            return OptimizationResult(
                parameter="min_edge_pct",
                current_value=current_threshold,
                optimal_value=current_threshold,
                expected_improvement=0,
                confidence="low",
                rationale="No thresholds had enough data",
                samples_analyzed=len(features),
            )

        # Find optimal threshold (maximize EV while keeping reasonable trade count)
        # We want at least 50% of trades to remain
        min_trades = len(features) * 0.3

        best_threshold = current_threshold
        best_ev = threshold_performance.get(current_threshold, {}).get("ev_per_trade", 0)

        for threshold, perf in threshold_performance.items():
            if perf["trades"] >= min_trades and perf["ev_per_trade"] > best_ev:
                best_threshold = threshold
                best_ev = perf["ev_per_trade"]

        # Calculate expected improvement
        current_perf = threshold_performance.get(current_threshold, {})
        optimal_perf = threshold_performance.get(best_threshold, {})

        current_ev = current_perf.get("ev_per_trade", 0)
        optimal_ev = optimal_perf.get("ev_per_trade", 0)

        improvement = (optimal_ev - current_ev) / abs(current_ev) if current_ev != 0 else 0

        # Determine confidence
        optimal_trades = optimal_perf.get("trades", 0)
        if optimal_trades >= 50:
            confidence = "high"
        elif optimal_trades >= 20:
            confidence = "medium"
        else:
            confidence = "low"

        return OptimizationResult(
            parameter="min_edge_pct",
            current_value=current_threshold,
            optimal_value=best_threshold,
            expected_improvement=improvement,
            confidence=confidence,
            rationale=f"Threshold {best_threshold}% has {optimal_perf.get('win_rate', 0):.0%} win rate "
                     f"vs {current_perf.get('win_rate', 0):.0%} at current {current_threshold}%",
            samples_analyzed=len(features),
        )

    def optimize_kelly_fraction(
        self,
        features: list[TradeFeatures],
        current_kelly: float = 0.25,
    ) -> OptimizationResult:
        """
        Find optimal Kelly fraction for position sizing.

        Uses the Kelly criterion formula adjusted for empirical win rates.

        Args:
            features: List of trade features with outcomes
            current_kelly: Current Kelly fraction setting

        Returns:
            OptimizationResult with recommendation
        """
        if len(features) < 30:
            return OptimizationResult(
                parameter="kelly_fraction",
                current_value=current_kelly,
                optimal_value=current_kelly,
                expected_improvement=0,
                confidence="low",
                rationale="Insufficient data for Kelly optimization",
                samples_analyzed=len(features),
            )

        # Calculate empirical win rate and average win/loss sizes
        wins = [f for f in features if f.outcome == 1]
        losses = [f for f in features if f.outcome == 0]

        if not wins or not losses:
            return OptimizationResult(
                parameter="kelly_fraction",
                current_value=current_kelly,
                optimal_value=current_kelly,
                expected_improvement=0,
                confidence="low",
                rationale="Need both wins and losses for Kelly calculation",
                samples_analyzed=len(features),
            )

        p = len(wins) / (len(wins) + len(losses))  # Win probability
        avg_win = sum(f.pnl for f in wins) / len(wins)
        avg_loss = abs(sum(f.pnl for f in losses) / len(losses))

        if avg_loss == 0:
            return OptimizationResult(
                parameter="kelly_fraction",
                current_value=current_kelly,
                optimal_value=current_kelly,
                expected_improvement=0,
                confidence="low",
                rationale="Average loss is zero",
                samples_analyzed=len(features),
            )

        # Kelly formula: f* = (p * b - q) / b
        # where b = avg_win / avg_loss, q = 1 - p
        b = avg_win / avg_loss
        q = 1 - p

        kelly_optimal = (p * b - q) / b if b > 0 else 0

        # Apply half-Kelly for safety
        kelly_recommended = kelly_optimal * 0.5

        # Clamp to reasonable range
        kelly_recommended = max(0.05, min(0.5, kelly_recommended))

        # Determine confidence based on sample size
        total_trades = len(wins) + len(losses)
        if total_trades >= 100:
            confidence = "high"
        elif total_trades >= 50:
            confidence = "medium"
        else:
            confidence = "low"

        improvement = (kelly_recommended - current_kelly) / current_kelly if current_kelly > 0 else 0

        return OptimizationResult(
            parameter="kelly_fraction",
            current_value=current_kelly,
            optimal_value=round(kelly_recommended, 2),
            expected_improvement=improvement,
            confidence=confidence,
            rationale=f"Win rate {p:.0%}, avg win ${avg_win:.2f}, avg loss ${avg_loss:.2f}. "
                     f"Full Kelly = {kelly_optimal:.2f}, using half-Kelly = {kelly_recommended:.2f}",
            samples_analyzed=total_trades,
        )

    def optimize_all(
        self,
        features: list[TradeFeatures],
        current_params: dict,
    ) -> list[OptimizationResult]:
        """
        Run all optimizations and return results.

        Args:
            features: List of trade features
            current_params: Current parameter values

        Returns:
            List of OptimizationResult for each parameter
        """
        results = []

        # Optimize edge threshold
        edge_result = self.optimize_edge_threshold(
            features,
            current_params.get("min_edge_pct", 2.0),
        )
        if edge_result.optimal_value != edge_result.current_value:
            results.append(edge_result)

        # Optimize Kelly fraction
        kelly_result = self.optimize_kelly_fraction(
            features,
            current_params.get("kelly_fraction", 0.25),
        )
        if abs(kelly_result.optimal_value - kelly_result.current_value) > 0.02:
            results.append(kelly_result)

        # Sort by expected improvement
        results.sort(key=lambda x: abs(x.expected_improvement), reverse=True)

        return results

    def analyze_sport_exposure(
        self,
        features: list[TradeFeatures],
    ) -> dict[str, dict]:
        """
        Analyze performance by sport to identify exposure adjustments.

        Args:
            features: List of trade features

        Returns:
            Dictionary mapping sport code to analysis results
        """
        by_sport: dict[int, list[TradeFeatures]] = {}

        for f in features:
            sport = f.sport_encoded
            if sport not in by_sport:
                by_sport[sport] = []
            by_sport[sport].append(f)

        results = {}
        for sport, trades in by_sport.items():
            wins = sum(1 for t in trades if t.outcome == 1)
            total = len(trades)
            pnl = sum(t.pnl for t in trades)

            win_rate = wins / total if total > 0 else 0

            # Determine recommendation
            if win_rate < 0.40 and total >= 10:
                recommendation = "reduce"
                rationale = f"Low win rate ({win_rate:.0%}) suggests reducing exposure"
            elif win_rate > 0.60 and total >= 10:
                recommendation = "increase"
                rationale = f"High win rate ({win_rate:.0%}) suggests increasing exposure"
            else:
                recommendation = "maintain"
                rationale = "Performance within expected range"

            results[sport] = {
                "trades": total,
                "wins": wins,
                "win_rate": win_rate,
                "pnl": pnl,
                "recommendation": recommendation,
                "rationale": rationale,
                "confidence": "high" if total >= 20 else "medium" if total >= 10 else "low",
            }

        return results
