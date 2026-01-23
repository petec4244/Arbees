"""
Hot Wash Report Generator.

Generates markdown and HTML reports from performance insights.
"""

from datetime import date
from typing import Optional
import logging

from .insights import PerformanceInsights, CategoryPerformance, TradeBreakdown

logger = logging.getLogger(__name__)


class ReportGenerator:
    """
    Generates nightly hot wash reports from performance insights.

    Produces both Markdown and HTML formats suitable for
    file storage, email, or Slack delivery.
    """

    def generate_markdown(self, insights: PerformanceInsights) -> str:
        """
        Generate a markdown hot wash report.

        Args:
            insights: PerformanceInsights from the analyzer

        Returns:
            Markdown formatted report string
        """
        pnl_emoji = "" if insights.total_pnl >= 1500 else ""
        trend_emoji = "" if insights.total_pnl >= 0 else ""

        report = f"""# Arbees Trading Report - {insights.analysis_date.strftime('%B %d, %Y')}

## Executive Summary {pnl_emoji}
- **Daily P&L:** ${insights.total_pnl:,.2f} {trend_emoji}
- **Win Rate:** {insights.win_rate:.1%} ({insights.winning_trades} wins / {insights.total_trades} trades)
- **Avg Edge:** {insights.avg_edge:.2%}
- **Signal Capture:** {insights.signals_executed}/{insights.signals_generated} ({self._capture_rate(insights):.0%})

---

## What Went Well
{self._format_successes(insights)}

## What Needs Improvement
{self._format_improvements(insights)}

---

## Losing Trades Analysis
{self._format_losing_trades(insights)}

---

## Recommended Changes
{self._format_recommendations(insights)}

---

## Performance by Category

### By Sport
{self._format_category_table(insights.by_sport)}

### By Signal Type
{self._format_category_table(insights.by_signal_type)}

### By Edge Range
{self._format_category_table(insights.by_edge_range)}

### By Game Period
{self._format_category_table(insights.by_period)}

---

## Missed Opportunities
{self._format_missed_opportunities(insights)}

---

*Generated automatically by Arbees ML Analyzer*
"""
        return report

    def generate_html(self, insights: PerformanceInsights) -> str:
        """
        Generate an HTML hot wash report.

        Args:
            insights: PerformanceInsights from the analyzer

        Returns:
            HTML formatted report string
        """
        md = self.generate_markdown(insights)

        try:
            import markdown
            html_content = markdown.markdown(md, extensions=['tables', 'fenced_code'])
        except ImportError:
            # Fallback: basic conversion
            html_content = f"<pre>{md}</pre>"

        html = f"""<!DOCTYPE html>
<html>
<head>
    <title>Arbees Report - {insights.analysis_date}</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 20px;
            background: #1a1a2e;
            color: #eee;
        }}
        h1, h2, h3 {{ color: #4ade80; }}
        table {{
            border-collapse: collapse;
            width: 100%;
            margin: 15px 0;
        }}
        th, td {{
            border: 1px solid #444;
            padding: 8px 12px;
            text-align: left;
        }}
        th {{ background: #2d2d44; }}
        tr:nth-child(even) {{ background: #242438; }}
        .positive {{ color: #4ade80; }}
        .negative {{ color: #f87171; }}
        code {{ background: #2d2d44; padding: 2px 6px; border-radius: 4px; }}
        hr {{ border: none; border-top: 1px solid #444; margin: 30px 0; }}
    </style>
</head>
<body>
{html_content}
</body>
</html>
"""
        return html

    def generate_slack_message(self, insights: PerformanceInsights) -> dict:
        """
        Generate a Slack message payload.

        Args:
            insights: PerformanceInsights from the analyzer

        Returns:
            Slack message payload dictionary
        """
        pnl_color = "#4ade80" if insights.total_pnl >= 0 else "#f87171"

        blocks = [
            {
                "type": "header",
                "text": {
                    "type": "plain_text",
                    "text": f"Arbees Daily Report - {insights.analysis_date.strftime('%b %d, %Y')}"
                }
            },
            {
                "type": "section",
                "fields": [
                    {"type": "mrkdwn", "text": f"*Daily P&L:*\n${insights.total_pnl:,.2f}"},
                    {"type": "mrkdwn", "text": f"*Win Rate:*\n{insights.win_rate:.1%}"},
                    {"type": "mrkdwn", "text": f"*Trades:*\n{insights.total_trades}"},
                    {"type": "mrkdwn", "text": f"*Avg Edge:*\n{insights.avg_edge:.1%}"},
                ]
            },
            {"type": "divider"},
        ]

        # Add best/worst sport
        if insights.best_sport or insights.worst_sport:
            fields = []
            if insights.best_sport:
                fields.append({
                    "type": "mrkdwn",
                    "text": f"*Best Sport:*\n{insights.best_sport.name} ({insights.best_sport.win_rate:.0%})"
                })
            if insights.worst_sport:
                fields.append({
                    "type": "mrkdwn",
                    "text": f"*Worst Sport:*\n{insights.worst_sport.name} ({insights.worst_sport.win_rate:.0%})"
                })
            blocks.append({"type": "section", "fields": fields})

        # Add recommendations
        if insights.recommendations:
            rec_text = "*Recommendations:*\n"
            for rec in insights.recommendations[:3]:
                rec_text += f"- {rec.title}\n"
            blocks.append({"type": "section", "text": {"type": "mrkdwn", "text": rec_text}})

        return {
            "attachments": [{
                "color": pnl_color,
                "blocks": blocks,
            }]
        }

    def _capture_rate(self, insights: PerformanceInsights) -> float:
        """Calculate signal capture rate."""
        if insights.signals_generated == 0:
            return 0
        return insights.signals_executed / insights.signals_generated

    def _format_successes(self, insights: PerformanceInsights) -> str:
        """Format the successes section."""
        lines = []

        if insights.best_sport and insights.best_sport.win_rate > 0.5:
            lines.append(f"1. **{insights.best_sport.name} performed well:** "
                        f"{insights.best_sport.win_rate:.0%} win rate "
                        f"({insights.best_sport.wins}/{insights.best_sport.trades} trades, "
                        f"${insights.best_sport.pnl:.2f} P&L)")

        if insights.best_edge_range and insights.best_edge_range.win_rate > 0.5:
            lines.append(f"2. **Edge range {insights.best_edge_range.name}** achieved "
                        f"{insights.best_edge_range.win_rate:.0%} win rate")

        if insights.best_signal_type and insights.best_signal_type.win_rate > 0.55:
            lines.append(f"3. **{insights.best_signal_type.name}** signals had "
                        f"{insights.best_signal_type.win_rate:.0%} success rate")

        if insights.win_rate > 0.55:
            lines.append(f"4. **Overall win rate above 55%** - strategy is profitable")

        if not lines:
            lines.append("- Consistent performance across all categories")

        return "\n".join(lines)

    def _format_improvements(self, insights: PerformanceInsights) -> str:
        """Format the improvements needed section."""
        lines = []

        if insights.worst_sport and insights.worst_sport.win_rate < 0.45:
            lines.append(f"1. **{insights.worst_sport.name} underperformed:** "
                        f"{insights.worst_sport.win_rate:.0%} win rate "
                        f"(${insights.worst_sport.pnl:.2f} loss)")

        capture_rate = self._capture_rate(insights)
        if capture_rate < 0.6 and insights.signals_generated >= 10:
            missed = insights.signals_generated - insights.signals_executed
            lines.append(f"2. **Missed {missed} signals** ({100-capture_rate*100:.0f}% not captured)")

        if insights.worst_signal_type and insights.worst_signal_type.win_rate < 0.4:
            lines.append(f"3. **{insights.worst_signal_type.name}** signals struggling "
                        f"({insights.worst_signal_type.win_rate:.0%} win rate)")

        if not lines:
            lines.append("- No major issues identified today")

        return "\n".join(lines)

    def _format_losing_trades(self, insights: PerformanceInsights) -> str:
        """Format the losing trades analysis."""
        total_losses = sum(t.pnl for t in insights.worst_trades if t.pnl < 0)

        lines = [
            f"**Total Losses:** ${abs(total_losses):,.2f} ({insights.losing_trades} trades)",
            "",
            "**Top Losses:**",
        ]

        for i, trade in enumerate(insights.worst_trades[:3], 1):
            if trade.pnl < 0:
                lines.append(f"{i}. {trade.sport} - {trade.game_period} "
                            f"(${trade.pnl:,.2f}) - Edge was {trade.edge:.1%}")

        if insights.losing_trades == 0:
            return "No losing trades today!"

        return "\n".join(lines)

    def _format_recommendations(self, insights: PerformanceInsights) -> str:
        """Format the recommendations section."""
        if not insights.recommendations:
            return "- Continue with current parameters"

        lines = []
        for i, rec in enumerate(insights.recommendations[:4], 1):
            lines.append(f"""{i}. **{rec.title}** [{rec.confidence} confidence]
   - Current: `{rec.current}`
   - Recommended: `{rec.recommended}`
   - Impact: {rec.impact}
   - Rationale: {rec.rationale}
""")

        return "\n".join(lines)

    def _format_category_table(self, data: dict[str, CategoryPerformance]) -> str:
        """Format a category breakdown as a markdown table."""
        if not data:
            return "_No data available_"

        lines = [
            "| Category | Trades | Wins | Win Rate | P&L |",
            "|----------|--------|------|----------|-----|",
        ]

        # Sort by P&L descending
        sorted_items = sorted(data.items(), key=lambda x: -x[1].pnl)

        for name, perf in sorted_items:
            pnl_str = f"${perf.pnl:,.2f}"
            if perf.pnl >= 0:
                pnl_str = f"+{pnl_str}"
            lines.append(
                f"| {name} | {perf.trades} | {perf.wins} | "
                f"{perf.win_rate:.0%} | {pnl_str} |"
            )

        return "\n".join(lines)

    def _format_missed_opportunities(self, insights: PerformanceInsights) -> str:
        """Format missed signal analysis."""
        if not insights.missed_reasons:
            return "All signals were captured!"

        lines = [
            f"**Signals Generated:** {insights.signals_generated}",
            f"**Signals Executed:** {insights.signals_executed}",
            f"**Capture Rate:** {self._capture_rate(insights):.0%}",
            "",
            "**Reasons for Missed Signals:**",
        ]

        for reason, count in sorted(insights.missed_reasons.items(), key=lambda x: -x[1]):
            reason_display = reason.replace("_", " ").title()
            lines.append(f"- {reason_display}: {count}")

        return "\n".join(lines)
