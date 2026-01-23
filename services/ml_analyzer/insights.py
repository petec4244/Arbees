"""
Insight extraction for ML performance analysis.

Analyzes trading performance and generates actionable insights
for the nightly hot wash reports.
"""

from dataclasses import dataclass, field
from datetime import date
from typing import Any, Optional
import logging

from .feature_extractor import FeatureExtractor, TradeFeatures

logger = logging.getLogger(__name__)


@dataclass
class CategoryPerformance:
    """Performance metrics for a category (sport, signal type, etc.)."""
    name: str
    trades: int
    wins: int
    losses: int
    pnl: float
    win_rate: float
    avg_edge: float = 0.0

    @classmethod
    def from_trades(cls, name: str, trades: list[TradeFeatures]) -> "CategoryPerformance":
        """Create from a list of trade features."""
        total = len(trades)
        wins = sum(1 for t in trades if t.outcome == 1)
        losses = sum(1 for t in trades if t.outcome == 0)
        pnl = sum(t.pnl for t in trades)
        edges = [t.edge_at_entry for t in trades if t.edge_at_entry > 0]
        avg_edge = sum(edges) / len(edges) if edges else 0

        return cls(
            name=name,
            trades=total,
            wins=wins,
            losses=losses,
            pnl=pnl,
            win_rate=wins / total if total > 0 else 0,
            avg_edge=avg_edge,
        )


@dataclass
class TradeBreakdown:
    """Breakdown of a single trade for analysis."""
    trade_id: str
    game_id: str
    sport: str
    signal_type: str
    edge: float
    pnl: float
    outcome: str
    game_period: str
    reason: str = ""


@dataclass
class Recommendation:
    """A recommendation for parameter adjustment."""
    title: str
    parameter: str
    current: Any
    recommended: Any
    impact: str
    confidence: str  # "low", "medium", "high"
    rationale: str


@dataclass
class PerformanceInsights:
    """
    Container for all performance insights from analysis.

    Used to generate the nightly hot wash report.
    """
    # Analysis date
    analysis_date: date

    # Summary stats
    total_trades: int = 0
    winning_trades: int = 0
    losing_trades: int = 0
    push_trades: int = 0
    total_pnl: float = 0.0
    win_rate: float = 0.0
    avg_edge: float = 0.0

    # Category breakdowns
    by_sport: dict[str, CategoryPerformance] = field(default_factory=dict)
    by_signal_type: dict[str, CategoryPerformance] = field(default_factory=dict)
    by_market_type: dict[str, CategoryPerformance] = field(default_factory=dict)
    by_edge_range: dict[str, CategoryPerformance] = field(default_factory=dict)
    by_period: dict[str, CategoryPerformance] = field(default_factory=dict)

    # Signal analysis
    signals_generated: int = 0
    signals_executed: int = 0
    missed_reasons: dict[str, int] = field(default_factory=dict)

    # Best/worst trades
    best_trades: list[TradeBreakdown] = field(default_factory=list)
    worst_trades: list[TradeBreakdown] = field(default_factory=list)

    # Top/worst performers
    best_sport: Optional[CategoryPerformance] = None
    worst_sport: Optional[CategoryPerformance] = None
    best_signal_type: Optional[CategoryPerformance] = None
    worst_signal_type: Optional[CategoryPerformance] = None
    best_edge_range: Optional[CategoryPerformance] = None

    # Recommendations
    recommendations: list[Recommendation] = field(default_factory=list)

    # Model metrics (if ML is available)
    model_accuracy: Optional[float] = None
    feature_importance: dict[str, float] = field(default_factory=dict)

    # Anomaly detection results
    anomaly_count: int = 0
    critical_anomalies: int = 0
    anomaly_summary: str = ""


class InsightExtractor:
    """
    Extracts insights from trading performance data.

    Analyzes trades, signals, and features to generate actionable
    insights and recommendations for the hot wash report.
    """

    # Sport name mapping for display
    SPORT_NAMES = {
        0: "NBA", 1: "NFL", 2: "NHL", 3: "MLB",
        4: "NCAAF", 5: "NCAAB", 6: "Soccer", 7: "MLS",
        8: "Tennis", 9: "MMA",
    }

    # Signal type names for display
    SIGNAL_TYPE_NAMES = {
        0: "Model Edge YES",
        1: "Model Edge NO",
        2: "Cross-Market Arb",
        3: "Win Prob Shift",
        4: "Mean Reversion",
        5: "Momentum",
    }

    # Period names for display
    PERIOD_NAMES = {
        1: "1st Period", 2: "2nd Period", 3: "3rd Period",
        4: "4th Period", 5: "Overtime",
    }

    # Edge range names
    EDGE_RANGE_NAMES = {
        0: "0-1%", 1: "1-2%", 2: "2-3%", 3: "3-5%", 4: "5%+",
    }

    def __init__(self):
        """Initialize the insight extractor."""
        self.feature_extractor = FeatureExtractor()

    def analyze(
        self,
        trades: list[dict],
        signals: list[dict],
        for_date: date,
        current_params: Optional[dict] = None,
    ) -> PerformanceInsights:
        """
        Analyze trading performance and generate insights.

        Args:
            trades: List of trade records
            signals: List of signal records
            for_date: Date being analyzed
            current_params: Current trading parameters for recommendations

        Returns:
            PerformanceInsights with all analysis results
        """
        insights = PerformanceInsights(analysis_date=for_date)

        if not trades:
            logger.info("No trades to analyze")
            return insights

        # Extract features
        features = self.feature_extractor.extract_batch(trades, signals)

        # Calculate summary stats
        insights.total_trades = len(features)
        insights.winning_trades = sum(1 for f in features if f.outcome == 1)
        insights.losing_trades = sum(1 for f in features if f.outcome == 0)
        insights.push_trades = sum(1 for f in features if f.outcome == -1)
        insights.total_pnl = sum(f.pnl for f in features)
        insights.win_rate = (
            insights.winning_trades / insights.total_trades
            if insights.total_trades > 0 else 0
        )

        edges = [f.edge_at_entry for f in features if f.edge_at_entry > 0]
        insights.avg_edge = sum(edges) / len(edges) if edges else 0

        # Signal analysis
        insights.signals_generated = len(signals)
        executed_signal_ids = {t.get("signal_id") for t in trades if t.get("signal_id")}
        insights.signals_executed = len(executed_signal_ids)

        # Analyze missed signals
        insights.missed_reasons = self._analyze_missed_signals(signals, executed_signal_ids)

        # Category breakdowns
        insights.by_sport = self._breakdown_by_sport(features)
        insights.by_signal_type = self._breakdown_by_signal_type(features)
        insights.by_market_type = self._breakdown_by_market_type(features)
        insights.by_edge_range = self._breakdown_by_edge_range(features)
        insights.by_period = self._breakdown_by_period(features)

        # Find best/worst performers
        self._find_top_performers(insights)

        # Find best/worst individual trades
        insights.best_trades = self._get_best_trades(features, trades, 5)
        insights.worst_trades = self._get_worst_trades(features, trades, 5)

        # Generate recommendations
        if current_params:
            insights.recommendations = self._generate_recommendations(
                insights, features, current_params
            )

        return insights

    def _breakdown_by_sport(
        self, features: list[TradeFeatures]
    ) -> dict[str, CategoryPerformance]:
        """Break down performance by sport."""
        by_sport: dict[int, list[TradeFeatures]] = {}

        for f in features:
            sport = f.sport_encoded
            if sport not in by_sport:
                by_sport[sport] = []
            by_sport[sport].append(f)

        return {
            self.SPORT_NAMES.get(sport, f"Unknown ({sport})"): CategoryPerformance.from_trades(
                self.SPORT_NAMES.get(sport, f"Unknown ({sport})"), trades
            )
            for sport, trades in by_sport.items()
        }

    def _breakdown_by_signal_type(
        self, features: list[TradeFeatures]
    ) -> dict[str, CategoryPerformance]:
        """Break down performance by signal type."""
        by_type: dict[int, list[TradeFeatures]] = {}

        for f in features:
            sig_type = f.signal_type_encoded
            if sig_type not in by_type:
                by_type[sig_type] = []
            by_type[sig_type].append(f)

        return {
            self.SIGNAL_TYPE_NAMES.get(sig_type, f"Unknown ({sig_type})"): CategoryPerformance.from_trades(
                self.SIGNAL_TYPE_NAMES.get(sig_type, f"Unknown ({sig_type})"), trades
            )
            for sig_type, trades in by_type.items()
        }

    def _breakdown_by_market_type(
        self, features: list[TradeFeatures]
    ) -> dict[str, CategoryPerformance]:
        """Break down performance by market type."""
        market_names = {0: "Moneyline", 1: "Spread", 2: "Total"}
        by_type: dict[int, list[TradeFeatures]] = {}

        for f in features:
            mkt_type = f.market_type_encoded
            if mkt_type not in by_type:
                by_type[mkt_type] = []
            by_type[mkt_type].append(f)

        return {
            market_names.get(mkt_type, f"Unknown ({mkt_type})"): CategoryPerformance.from_trades(
                market_names.get(mkt_type, f"Unknown ({mkt_type})"), trades
            )
            for mkt_type, trades in by_type.items()
        }

    def _breakdown_by_edge_range(
        self, features: list[TradeFeatures]
    ) -> dict[str, CategoryPerformance]:
        """Break down performance by edge bucket."""
        by_edge: dict[int, list[TradeFeatures]] = {}

        for f in features:
            bucket = f.edge_bucket
            if bucket not in by_edge:
                by_edge[bucket] = []
            by_edge[bucket].append(f)

        return {
            self.EDGE_RANGE_NAMES.get(bucket, f"{bucket}"): CategoryPerformance.from_trades(
                self.EDGE_RANGE_NAMES.get(bucket, f"{bucket}"), trades
            )
            for bucket, trades in by_edge.items()
        }

    def _breakdown_by_period(
        self, features: list[TradeFeatures]
    ) -> dict[str, CategoryPerformance]:
        """Break down performance by game period."""
        by_period: dict[int, list[TradeFeatures]] = {}

        for f in features:
            period = f.game_period
            if period not in by_period:
                by_period[period] = []
            by_period[period].append(f)

        return {
            self.PERIOD_NAMES.get(period, f"Period {period}"): CategoryPerformance.from_trades(
                self.PERIOD_NAMES.get(period, f"Period {period}"), trades
            )
            for period, trades in by_period.items()
        }

    def _find_top_performers(self, insights: PerformanceInsights) -> None:
        """Find best and worst performing categories."""
        # Best/worst sport (min 3 trades)
        sports_with_data = [
            s for s in insights.by_sport.values() if s.trades >= 3
        ]
        if sports_with_data:
            insights.best_sport = max(sports_with_data, key=lambda x: x.win_rate)
            insights.worst_sport = min(sports_with_data, key=lambda x: x.win_rate)

        # Best/worst signal type (min 3 trades)
        signals_with_data = [
            s for s in insights.by_signal_type.values() if s.trades >= 3
        ]
        if signals_with_data:
            insights.best_signal_type = max(signals_with_data, key=lambda x: x.win_rate)
            insights.worst_signal_type = min(signals_with_data, key=lambda x: x.win_rate)

        # Best edge range (min 3 trades)
        edges_with_data = [
            e for e in insights.by_edge_range.values() if e.trades >= 3
        ]
        if edges_with_data:
            insights.best_edge_range = max(edges_with_data, key=lambda x: x.win_rate)

    def _get_best_trades(
        self, features: list[TradeFeatures], trades: list[dict], limit: int
    ) -> list[TradeBreakdown]:
        """Get the best performing trades by P&L."""
        sorted_features = sorted(features, key=lambda f: f.pnl, reverse=True)

        # Build lookup
        trade_lookup = {t.get("trade_id"): t for t in trades}

        result = []
        for f in sorted_features[:limit]:
            trade = trade_lookup.get(f.trade_id, {})
            result.append(TradeBreakdown(
                trade_id=f.trade_id,
                game_id=f.game_id,
                sport=self.SPORT_NAMES.get(f.sport_encoded, "Unknown"),
                signal_type=self.SIGNAL_TYPE_NAMES.get(f.signal_type_encoded, "Unknown"),
                edge=f.edge_at_entry,
                pnl=f.pnl,
                outcome="WIN" if f.outcome == 1 else "LOSS" if f.outcome == 0 else "PUSH",
                game_period=self.PERIOD_NAMES.get(f.game_period, f"Period {f.game_period}"),
                reason=trade.get("reason", ""),
            ))

        return result

    def _get_worst_trades(
        self, features: list[TradeFeatures], trades: list[dict], limit: int
    ) -> list[TradeBreakdown]:
        """Get the worst performing trades by P&L."""
        sorted_features = sorted(features, key=lambda f: f.pnl)

        trade_lookup = {t.get("trade_id"): t for t in trades}

        result = []
        for f in sorted_features[:limit]:
            trade = trade_lookup.get(f.trade_id, {})
            result.append(TradeBreakdown(
                trade_id=f.trade_id,
                game_id=f.game_id,
                sport=self.SPORT_NAMES.get(f.sport_encoded, "Unknown"),
                signal_type=self.SIGNAL_TYPE_NAMES.get(f.signal_type_encoded, "Unknown"),
                edge=f.edge_at_entry,
                pnl=f.pnl,
                outcome="WIN" if f.outcome == 1 else "LOSS" if f.outcome == 0 else "PUSH",
                game_period=self.PERIOD_NAMES.get(f.game_period, f"Period {f.game_period}"),
                reason=trade.get("reason", ""),
            ))

        return result

    def _analyze_missed_signals(
        self, signals: list[dict], executed_ids: set[str]
    ) -> dict[str, int]:
        """Analyze why signals weren't executed."""
        reasons: dict[str, int] = {}

        for signal in signals:
            sig_id = signal.get("signal_id")
            if sig_id and sig_id not in executed_ids:
                # Determine reason (this would need more context in practice)
                reason = "unknown"

                edge = signal.get("edge_pct", 0)
                if edge < 2.0:
                    reason = "edge_below_threshold"
                elif signal.get("expires_at"):
                    reason = "expired_before_execution"
                else:
                    reason = "risk_limits"

                reasons[reason] = reasons.get(reason, 0) + 1

        return reasons

    def _generate_recommendations(
        self,
        insights: PerformanceInsights,
        features: list[TradeFeatures],
        current_params: dict,
    ) -> list[Recommendation]:
        """Generate parameter recommendations based on analysis."""
        recommendations = []

        # Check if edge threshold should change
        if insights.best_edge_range:
            best_edge = insights.best_edge_range.name
            current_min_edge = current_params.get("min_edge_pct", 2.0)

            # If low edge trades are losing, recommend raising threshold
            low_edge_perf = insights.by_edge_range.get("0-1%")
            if low_edge_perf and low_edge_perf.trades >= 5 and low_edge_perf.win_rate < 0.45:
                recommendations.append(Recommendation(
                    title="Raise Minimum Edge Threshold",
                    parameter="min_edge_pct",
                    current=current_min_edge,
                    recommended=max(current_min_edge + 0.5, 2.0),
                    impact=f"Avoid low-edge trades with {low_edge_perf.win_rate:.0%} win rate",
                    confidence="medium",
                    rationale=f"Trades with edge <1% have {low_edge_perf.win_rate:.0%} win rate vs overall {insights.win_rate:.0%}",
                ))

        # Check if a sport should be avoided
        if insights.worst_sport and insights.worst_sport.win_rate < 0.35:
            recommendations.append(Recommendation(
                title=f"Reduce {insights.worst_sport.name} Exposure",
                parameter="sport_limits",
                current="unrestricted",
                recommended=f"max 50% of {insights.worst_sport.name} positions",
                impact=f"Reduce exposure to sport with {insights.worst_sport.win_rate:.0%} win rate",
                confidence="medium" if insights.worst_sport.trades >= 10 else "low",
                rationale=f"{insights.worst_sport.name} has {insights.worst_sport.win_rate:.0%} win rate over {insights.worst_sport.trades} trades",
            ))

        # Check signal capture rate
        if insights.signals_generated > 0:
            capture_rate = insights.signals_executed / insights.signals_generated
            if capture_rate < 0.5:
                top_reason = max(insights.missed_reasons.items(), key=lambda x: x[1]) if insights.missed_reasons else ("unknown", 0)
                recommendations.append(Recommendation(
                    title="Improve Signal Capture Rate",
                    parameter="execution_speed",
                    current=f"{capture_rate:.0%} captured",
                    recommended="target 70%+ capture rate",
                    impact=f"Currently missing {100-capture_rate*100:.0f}% of signals",
                    confidence="high" if insights.signals_generated >= 20 else "medium",
                    rationale=f"Top reason for missed signals: {top_reason[0]} ({top_reason[1]} signals)",
                ))

        # Check position sizing
        avg_position = sum(f.size for f in features) / len(features) if features else 0
        if avg_position > 0:
            current_kelly = current_params.get("kelly_fraction", 0.25)
            # If win rate is high and we're being conservative, suggest increasing
            if insights.win_rate > 0.55 and current_kelly < 0.3:
                recommendations.append(Recommendation(
                    title="Consider Increasing Kelly Fraction",
                    parameter="kelly_fraction",
                    current=current_kelly,
                    recommended=min(current_kelly + 0.05, 0.35),
                    impact=f"Capitalize on {insights.win_rate:.0%} win rate",
                    confidence="low",
                    rationale="Win rate above 55% suggests conservative sizing may leave money on table",
                ))
            # If win rate is low, suggest reducing
            elif insights.win_rate < 0.45 and current_kelly > 0.2:
                recommendations.append(Recommendation(
                    title="Reduce Kelly Fraction",
                    parameter="kelly_fraction",
                    current=current_kelly,
                    recommended=max(current_kelly - 0.05, 0.15),
                    impact=f"Reduce drawdown risk with {insights.win_rate:.0%} win rate",
                    confidence="medium",
                    rationale="Win rate below 45% suggests reducing position sizing",
                ))

        return recommendations

    def format_summary(self, insights: PerformanceInsights) -> str:
        """Format insights as a brief text summary."""
        lines = [
            f"Performance Summary for {insights.analysis_date}",
            f"=" * 40,
            f"",
            f"Total Trades: {insights.total_trades}",
            f"Win Rate: {insights.win_rate:.1%} ({insights.winning_trades}W/{insights.losing_trades}L)",
            f"Total P&L: ${insights.total_pnl:.2f}",
            f"Avg Edge: {insights.avg_edge:.1%}",
            f"",
            f"Signal Capture: {insights.signals_executed}/{insights.signals_generated} "
            f"({insights.signals_executed/max(insights.signals_generated,1):.0%})",
        ]

        if insights.best_sport:
            lines.append(f"")
            lines.append(f"Best Sport: {insights.best_sport.name} ({insights.best_sport.win_rate:.0%})")

        if insights.worst_sport:
            lines.append(f"Worst Sport: {insights.worst_sport.name} ({insights.worst_sport.win_rate:.0%})")

        if insights.recommendations:
            lines.append(f"")
            lines.append(f"Recommendations ({len(insights.recommendations)}):")
            for rec in insights.recommendations[:3]:
                lines.append(f"  - {rec.title}")

        return "\n".join(lines)
