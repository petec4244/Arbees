"""
Anomaly detection for trading data.

Detects issues like:
- Exponential position size growth
- Too many trades per game
- Unrealistic PnL numbers
- Suspicious win rates
"""

from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from typing import Optional
import logging
import statistics

logger = logging.getLogger(__name__)


@dataclass
class AnomalyReport:
    """Container for detected anomalies."""
    analysis_date: date
    anomalies: list["Anomaly"] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    @property
    def has_critical(self) -> bool:
        """Check if any critical anomalies exist."""
        return any(a.severity == "critical" for a in self.anomalies)

    @property
    def critical_count(self) -> int:
        """Count of critical anomalies."""
        return sum(1 for a in self.anomalies if a.severity == "critical")

    @property
    def warning_count(self) -> int:
        """Count of warning anomalies."""
        return sum(1 for a in self.anomalies if a.severity == "warning")


@dataclass
class Anomaly:
    """A detected anomaly in trading data."""
    anomaly_type: str
    severity: str  # "critical", "warning", "info"
    title: str
    description: str
    details: dict = field(default_factory=dict)


class AnomalyDetector:
    """
    Detects anomalies in trading data that indicate bugs or issues.

    Anomaly types:
    - position_size_explosion: Position sizes growing exponentially
    - game_trade_overflow: Too many trades on single game
    - unrealistic_pnl: PnL numbers that don't match reality
    - suspicious_win_rate: Win rates that are unrealistically high/low
    - bankroll_growth_anomaly: Bankroll growing faster than possible
    """

    # Thresholds
    MAX_TRADES_PER_GAME = 5  # More than this is suspicious
    MAX_POSITION_SIZE_GROWTH = 2.0  # Position shouldn't double within same game
    MAX_SINGLE_GAME_PNL_PCT = 50.0  # Single game shouldn't contribute >50% of daily PnL
    MAX_DAILY_BANKROLL_GROWTH_PCT = 100.0  # Bankroll shouldn't 2x in a day
    SUSPICIOUS_WIN_RATE_HIGH = 0.85  # Win rate above 85% over many trades is suspicious
    SUSPICIOUS_WIN_RATE_LOW = 0.15  # Win rate below 15% over many trades is suspicious
    MIN_TRADES_FOR_RATE_CHECK = 20  # Need at least this many trades to flag win rate

    def __init__(
        self,
        max_trades_per_game: int = 5,
        max_position_growth: float = 2.0,
        max_single_game_pnl_pct: float = 50.0,
        max_daily_growth_pct: float = 100.0,
    ):
        """Initialize detector with configurable thresholds."""
        self.max_trades_per_game = max_trades_per_game
        self.max_position_growth = max_position_growth
        self.max_single_game_pnl_pct = max_single_game_pnl_pct
        self.max_daily_growth_pct = max_daily_growth_pct

    def analyze(
        self,
        trades: list[dict],
        for_date: date,
        starting_bankroll: Optional[float] = None,
        ending_bankroll: Optional[float] = None,
    ) -> AnomalyReport:
        """
        Analyze trades for anomalies.

        Args:
            trades: List of trade dictionaries from database
            for_date: Date being analyzed
            starting_bankroll: Bankroll at start of day (optional)
            ending_bankroll: Bankroll at end of day (optional)

        Returns:
            AnomalyReport with detected issues
        """
        report = AnomalyReport(analysis_date=for_date)

        if not trades:
            return report

        # Run all anomaly checks
        self._check_game_trade_overflow(trades, report)
        self._check_position_size_explosion(trades, report)
        self._check_unrealistic_pnl(trades, report)
        self._check_suspicious_win_rate(trades, report)

        if starting_bankroll and ending_bankroll:
            self._check_bankroll_growth(
                trades, starting_bankroll, ending_bankroll, report
            )

        # Log summary
        if report.has_critical:
            logger.warning(
                f"Anomaly detection found {report.critical_count} CRITICAL and "
                f"{report.warning_count} warning issues for {for_date}"
            )
        elif report.warning_count > 0:
            logger.info(
                f"Anomaly detection found {report.warning_count} warnings for {for_date}"
            )

        return report

    def _check_game_trade_overflow(
        self, trades: list[dict], report: AnomalyReport
    ) -> None:
        """Check for games with too many trades."""
        # Group by game
        games: dict[str, list[dict]] = {}
        for t in trades:
            game_id = t.get("game_id")
            if game_id not in games:
                games[game_id] = []
            games[game_id].append(t)

        for game_id, game_trades in games.items():
            if len(game_trades) > self.max_trades_per_game:
                total_pnl = sum(float(t.get("pnl") or 0) for t in game_trades)
                total_volume = sum(float(t.get("size") or 0) for t in game_trades)

                market_title = game_trades[0].get("market_title", "Unknown")

                report.anomalies.append(Anomaly(
                    anomaly_type="game_trade_overflow",
                    severity="critical" if len(game_trades) > self.max_trades_per_game * 2 else "warning",
                    title=f"Too many trades on game {game_id[:8]}",
                    description=(
                        f"{len(game_trades)} trades on single game (max {self.max_trades_per_game}). "
                        f"Total volume: ${total_volume:,.2f}, PnL: ${total_pnl:,.2f}"
                    ),
                    details={
                        "game_id": game_id,
                        "trade_count": len(game_trades),
                        "total_volume": total_volume,
                        "total_pnl": total_pnl,
                        "market_title": market_title,
                    }
                ))

    def _check_position_size_explosion(
        self, trades: list[dict], report: AnomalyReport
    ) -> None:
        """Check for exponential position size growth within a game."""
        # Group by game and sort by time
        games: dict[str, list[dict]] = {}
        for t in trades:
            game_id = t.get("game_id")
            if game_id not in games:
                games[game_id] = []
            games[game_id].append(t)

        for game_id, game_trades in games.items():
            if len(game_trades) < 2:
                continue

            # Sort by entry time
            sorted_trades = sorted(
                game_trades,
                key=lambda x: x.get("entry_time") or x.get("opened_at") or datetime.min
            )

            sizes = [float(t.get("size") or 0) for t in sorted_trades]
            if min(sizes) <= 0:
                continue

            max_growth = max(sizes) / min(sizes)

            if max_growth > self.max_position_growth:
                report.anomalies.append(Anomaly(
                    anomaly_type="position_size_explosion",
                    severity="critical" if max_growth > self.max_position_growth * 2 else "warning",
                    title=f"Position size explosion on game {game_id[:8]}",
                    description=(
                        f"Position sizes grew {max_growth:.1f}x within same game "
                        f"(${min(sizes):.2f} -> ${max(sizes):.2f}). "
                        f"This indicates a bankroll compounding bug."
                    ),
                    details={
                        "game_id": game_id,
                        "min_size": min(sizes),
                        "max_size": max(sizes),
                        "growth_factor": max_growth,
                        "trade_count": len(sorted_trades),
                        "sizes": sizes,
                    }
                ))

    def _check_unrealistic_pnl(
        self, trades: list[dict], report: AnomalyReport
    ) -> None:
        """Check for PnL that doesn't match size and price movements."""
        for t in trades:
            entry = float(t.get("entry_price") or 0)
            exit_p = float(t.get("exit_price") or 0)
            size = float(t.get("size") or 0)
            pnl = float(t.get("pnl") or 0)
            side = t.get("side")

            if size <= 0 or not side:
                continue

            # Calculate expected PnL
            if side == "buy":
                expected_pnl = (exit_p - entry) * size
            else:
                expected_pnl = (entry - exit_p) * size

            # Allow small difference due to slippage/fees
            diff = abs(pnl - expected_pnl)
            diff_pct = (diff / size * 100) if size > 0 else 0

            if diff > 1.0 and diff_pct > 1.0:  # More than $1 and >1% of size
                report.anomalies.append(Anomaly(
                    anomaly_type="unrealistic_pnl",
                    severity="warning",
                    title=f"PnL mismatch on trade {t.get('trade_id', 'unknown')[:8]}",
                    description=(
                        f"Recorded PnL ${pnl:.2f} doesn't match expected ${expected_pnl:.2f} "
                        f"(diff: ${diff:.2f}, {diff_pct:.1f}%)"
                    ),
                    details={
                        "trade_id": t.get("trade_id"),
                        "entry_price": entry,
                        "exit_price": exit_p,
                        "size": size,
                        "side": side,
                        "recorded_pnl": pnl,
                        "expected_pnl": expected_pnl,
                        "difference": diff,
                    }
                ))

    def _check_suspicious_win_rate(
        self, trades: list[dict], report: AnomalyReport
    ) -> None:
        """Check for unrealistically high or low win rates."""
        closed = [t for t in trades if t.get("status") == "closed"]

        if len(closed) < self.MIN_TRADES_FOR_RATE_CHECK:
            return

        wins = sum(1 for t in closed if t.get("outcome") == "win")
        win_rate = wins / len(closed)

        if win_rate > self.SUSPICIOUS_WIN_RATE_HIGH:
            report.anomalies.append(Anomaly(
                anomaly_type="suspicious_win_rate",
                severity="warning",
                title="Suspiciously high win rate",
                description=(
                    f"Win rate of {win_rate:.1%} ({wins}/{len(closed)}) is unusually high. "
                    f"This may indicate data issues or a bug in outcome determination."
                ),
                details={
                    "win_rate": win_rate,
                    "wins": wins,
                    "total_trades": len(closed),
                }
            ))

        if win_rate < self.SUSPICIOUS_WIN_RATE_LOW:
            report.anomalies.append(Anomaly(
                anomaly_type="suspicious_win_rate",
                severity="warning",
                title="Suspiciously low win rate",
                description=(
                    f"Win rate of {win_rate:.1%} ({wins}/{len(closed)}) is unusually low. "
                    f"This may indicate a bug in trade execution or outcome determination."
                ),
                details={
                    "win_rate": win_rate,
                    "wins": wins,
                    "total_trades": len(closed),
                }
            ))

    def _check_bankroll_growth(
        self,
        trades: list[dict],
        starting_bankroll: float,
        ending_bankroll: float,
        report: AnomalyReport,
    ) -> None:
        """Check for unrealistic bankroll growth."""
        if starting_bankroll <= 0:
            return

        growth_pct = ((ending_bankroll - starting_bankroll) / starting_bankroll) * 100

        if growth_pct > self.max_daily_growth_pct:
            total_pnl = sum(float(t.get("pnl") or 0) for t in trades)

            report.anomalies.append(Anomaly(
                anomaly_type="bankroll_growth_anomaly",
                severity="critical",
                title="Unrealistic bankroll growth",
                description=(
                    f"Bankroll grew {growth_pct:.1f}% in one day "
                    f"(${starting_bankroll:,.2f} -> ${ending_bankroll:,.2f}). "
                    f"Total recorded PnL: ${total_pnl:,.2f}. "
                    f"This indicates a compounding bug or data corruption."
                ),
                details={
                    "starting_bankroll": starting_bankroll,
                    "ending_bankroll": ending_bankroll,
                    "growth_pct": growth_pct,
                    "total_pnl": total_pnl,
                }
            ))

    def format_report(self, report: AnomalyReport) -> str:
        """Format anomaly report as readable text."""
        lines = [
            f"# Anomaly Detection Report for {report.analysis_date}",
            "",
        ]

        if not report.anomalies:
            lines.append("No anomalies detected. All data looks normal.")
            return "\n".join(lines)

        lines.append(f"Found {len(report.anomalies)} anomalies:")
        lines.append(f"- Critical: {report.critical_count}")
        lines.append(f"- Warning: {report.warning_count}")
        lines.append("")

        # Group by severity
        for severity in ["critical", "warning", "info"]:
            anomalies = [a for a in report.anomalies if a.severity == severity]
            if not anomalies:
                continue

            lines.append(f"## {severity.upper()} Anomalies")
            lines.append("")

            for a in anomalies:
                lines.append(f"### {a.title}")
                lines.append(f"Type: {a.anomaly_type}")
                lines.append(f"Description: {a.description}")
                lines.append("")

        return "\n".join(lines)
