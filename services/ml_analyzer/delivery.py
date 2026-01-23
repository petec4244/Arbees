"""
Report Delivery Service.

Handles delivery of ML analysis reports via multiple channels:
- File system (markdown/HTML)
- Slack webhooks
- Email (SMTP)
"""

import asyncio
import logging
import smtplib
from dataclasses import dataclass, field
from datetime import datetime
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from pathlib import Path
from typing import Optional

from .insights import PerformanceInsights
from .report_generator import ReportGenerator

logger = logging.getLogger(__name__)


@dataclass
class DeliveryConfig:
    """Configuration for report delivery channels."""
    # File delivery
    enable_file_delivery: bool = True
    file_output_dir: str = "reports"

    # Slack delivery
    enable_slack: bool = False
    slack_webhook_url: Optional[str] = None
    slack_channel: Optional[str] = None  # Override default channel

    # Email delivery
    enable_email: bool = False
    smtp_host: str = "smtp.gmail.com"
    smtp_port: int = 587
    smtp_user: Optional[str] = None
    smtp_password: Optional[str] = None
    email_from: Optional[str] = None
    email_to: list[str] = field(default_factory=list)
    email_subject_prefix: str = "[Arbees] "

    @classmethod
    def from_env(cls) -> "DeliveryConfig":
        """Create config from environment variables."""
        import os

        email_to = os.environ.get("ML_EMAIL_TO", "")
        email_list = [e.strip() for e in email_to.split(",") if e.strip()]

        return cls(
            enable_file_delivery=os.environ.get("ML_ENABLE_FILE_DELIVERY", "true").lower() == "true",
            file_output_dir=os.environ.get("ML_REPORT_DIR", "reports"),
            enable_slack=os.environ.get("ML_SLACK_ENABLED", "false").lower() == "true",
            slack_webhook_url=os.environ.get("ML_SLACK_WEBHOOK"),
            slack_channel=os.environ.get("ML_SLACK_CHANNEL"),
            enable_email=os.environ.get("ML_EMAIL_ENABLED", "false").lower() == "true",
            smtp_host=os.environ.get("ML_SMTP_HOST", "smtp.gmail.com"),
            smtp_port=int(os.environ.get("ML_SMTP_PORT", "587")),
            smtp_user=os.environ.get("ML_SMTP_USER"),
            smtp_password=os.environ.get("ML_SMTP_PASSWORD"),
            email_from=os.environ.get("ML_EMAIL_FROM"),
            email_to=email_list,
            email_subject_prefix=os.environ.get("ML_EMAIL_SUBJECT_PREFIX", "[Arbees] "),
        )


@dataclass
class DeliveryResult:
    """Result of a delivery attempt."""
    channel: str
    success: bool
    message: str
    timestamp: datetime = field(default_factory=datetime.now)


class ReportDeliveryService:
    """
    Delivers ML analysis reports to configured channels.

    Supports multiple delivery methods:
    - File system: Saves markdown and HTML to reports directory
    - Slack: Posts to configured webhook with formatted message
    - Email: Sends HTML report via SMTP
    """

    def __init__(
        self,
        config: Optional[DeliveryConfig] = None,
        report_generator: Optional[ReportGenerator] = None,
    ):
        """Initialize the delivery service.

        Args:
            config: Delivery configuration
            report_generator: Report generator for formatting
        """
        self.config = config or DeliveryConfig.from_env()
        self.report_generator = report_generator or ReportGenerator()

    async def deliver_report(
        self,
        insights: PerformanceInsights,
        report_md: str,
        report_html: str,
    ) -> list[DeliveryResult]:
        """
        Deliver report to all configured channels.

        Args:
            insights: Performance insights
            report_md: Markdown report content
            report_html: HTML report content

        Returns:
            List of DeliveryResult for each channel
        """
        results = []

        # File delivery
        if self.config.enable_file_delivery:
            result = await self._deliver_to_file(insights.analysis_date, report_md, report_html)
            results.append(result)

        # Slack delivery
        if self.config.enable_slack and self.config.slack_webhook_url:
            result = await self._deliver_to_slack(insights)
            results.append(result)

        # Email delivery
        if self.config.enable_email and self.config.email_to:
            result = await self._deliver_to_email(insights, report_html)
            results.append(result)

        # Log summary
        success_count = sum(1 for r in results if r.success)
        logger.info(f"Report delivery: {success_count}/{len(results)} channels succeeded")

        return results

    async def _deliver_to_file(
        self,
        report_date,
        report_md: str,
        report_html: str,
    ) -> DeliveryResult:
        """Save report to file system."""
        try:
            date_str = report_date.strftime("%Y-%m-%d")
            base_path = Path(self.config.file_output_dir)
            base_path.mkdir(parents=True, exist_ok=True)

            # Save markdown
            md_path = base_path / f"hot_wash_{date_str}.md"
            md_path.write_text(report_md, encoding="utf-8")

            # Save HTML
            html_path = base_path / f"hot_wash_{date_str}.html"
            html_path.write_text(report_html, encoding="utf-8")

            logger.info(f"Saved reports to {base_path}")

            return DeliveryResult(
                channel="file",
                success=True,
                message=f"Saved to {md_path} and {html_path}",
            )

        except Exception as e:
            logger.error(f"File delivery failed: {e}")
            return DeliveryResult(
                channel="file",
                success=False,
                message=str(e),
            )

    async def _deliver_to_slack(self, insights: PerformanceInsights) -> DeliveryResult:
        """Post report to Slack webhook."""
        try:
            import aiohttp

            payload = self.report_generator.generate_slack_message(insights)

            # Add channel override if configured
            if self.config.slack_channel:
                payload["channel"] = self.config.slack_channel

            async with aiohttp.ClientSession() as session:
                async with session.post(
                    self.config.slack_webhook_url,
                    json=payload,
                    timeout=aiohttp.ClientTimeout(total=30),
                ) as response:
                    if response.status == 200:
                        logger.info("Delivered report to Slack")
                        return DeliveryResult(
                            channel="slack",
                            success=True,
                            message="Posted to Slack successfully",
                        )
                    else:
                        error_text = await response.text()
                        logger.error(f"Slack delivery failed: {response.status} - {error_text}")
                        return DeliveryResult(
                            channel="slack",
                            success=False,
                            message=f"HTTP {response.status}: {error_text}",
                        )

        except ImportError:
            logger.warning("aiohttp not installed, skipping Slack delivery")
            return DeliveryResult(
                channel="slack",
                success=False,
                message="aiohttp not installed",
            )
        except Exception as e:
            logger.error(f"Slack delivery error: {e}")
            return DeliveryResult(
                channel="slack",
                success=False,
                message=str(e),
            )

    async def _deliver_to_email(
        self,
        insights: PerformanceInsights,
        report_html: str,
    ) -> DeliveryResult:
        """Send report via email."""
        try:
            if not self.config.smtp_user or not self.config.smtp_password:
                return DeliveryResult(
                    channel="email",
                    success=False,
                    message="SMTP credentials not configured",
                )

            # Build email
            msg = MIMEMultipart("alternative")
            msg["Subject"] = (
                f"{self.config.email_subject_prefix}"
                f"Trading Report - {insights.analysis_date.strftime('%b %d, %Y')} "
                f"(${insights.total_pnl:,.2f})"
            )
            msg["From"] = self.config.email_from or self.config.smtp_user
            msg["To"] = ", ".join(self.config.email_to)

            # Plain text version (summary)
            text_content = self._generate_email_text(insights)
            msg.attach(MIMEText(text_content, "plain"))

            # HTML version (full report)
            msg.attach(MIMEText(report_html, "html"))

            # Send via SMTP (run in executor to not block)
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, self._send_smtp, msg)

            logger.info(f"Sent email report to {len(self.config.email_to)} recipients")
            return DeliveryResult(
                channel="email",
                success=True,
                message=f"Sent to {len(self.config.email_to)} recipients",
            )

        except Exception as e:
            logger.error(f"Email delivery error: {e}")
            return DeliveryResult(
                channel="email",
                success=False,
                message=str(e),
            )

    def _send_smtp(self, msg: MIMEMultipart) -> None:
        """Send email via SMTP (blocking)."""
        with smtplib.SMTP(self.config.smtp_host, self.config.smtp_port) as server:
            server.starttls()
            server.login(self.config.smtp_user, self.config.smtp_password)
            server.send_message(msg)

    def _generate_email_text(self, insights: PerformanceInsights) -> str:
        """Generate plain text email summary."""
        return f"""Arbees Trading Report - {insights.analysis_date.strftime('%B %d, %Y')}

SUMMARY
=======
Daily P&L: ${insights.total_pnl:,.2f}
Win Rate: {insights.win_rate:.1%} ({insights.winning_trades}W / {insights.losing_trades}L)
Avg Edge: {insights.avg_edge:.2%}
Signal Capture: {insights.signals_executed}/{insights.signals_generated}

TOP PERFORMERS
==============
Best Sport: {insights.best_sport.name if insights.best_sport else 'N/A'} ({insights.best_sport.win_rate:.0%} win rate if insights.best_sport else '')
Worst Sport: {insights.worst_sport.name if insights.worst_sport else 'N/A'} ({insights.worst_sport.win_rate:.0%} win rate if insights.worst_sport else '')

RECOMMENDATIONS
===============
{chr(10).join(f'- {r.title}' for r in insights.recommendations[:3]) if insights.recommendations else 'No changes recommended'}

View the full HTML report attached for detailed analysis.

---
Generated by Arbees ML Analyzer
"""


class SlackNotifier:
    """
    Lightweight Slack notifier for real-time alerts.

    Use this for immediate notifications separate from full reports.
    """

    def __init__(self, webhook_url: str):
        """Initialize with Slack webhook URL."""
        self.webhook_url = webhook_url

    async def send_alert(
        self,
        title: str,
        message: str,
        color: str = "#36a64f",  # Green
        fields: Optional[list[dict]] = None,
    ) -> bool:
        """Send an alert to Slack.

        Args:
            title: Alert title
            message: Alert message
            color: Attachment color (hex)
            fields: Optional list of field dicts with 'title' and 'value'

        Returns:
            True if sent successfully
        """
        try:
            import aiohttp

            payload = {
                "attachments": [{
                    "color": color,
                    "title": title,
                    "text": message,
                    "ts": int(datetime.now().timestamp()),
                }]
            }

            if fields:
                payload["attachments"][0]["fields"] = [
                    {"title": f["title"], "value": f["value"], "short": f.get("short", True)}
                    for f in fields
                ]

            async with aiohttp.ClientSession() as session:
                async with session.post(self.webhook_url, json=payload) as response:
                    return response.status == 200

        except Exception as e:
            logger.error(f"Slack alert failed: {e}")
            return False

    async def send_trade_alert(
        self,
        trade_type: str,
        game_id: str,
        sport: str,
        edge: float,
        size: float,
        side: str,
    ) -> bool:
        """Send a trade execution alert."""
        color = "#4ade80" if side.lower() == "buy" else "#f87171"
        return await self.send_alert(
            title=f"{trade_type} Trade Executed",
            message=f"{sport} - {game_id}",
            color=color,
            fields=[
                {"title": "Side", "value": side.upper()},
                {"title": "Size", "value": f"${size:.2f}"},
                {"title": "Edge", "value": f"{edge:.1%}"},
            ],
        )

    async def send_error_alert(self, error_type: str, message: str) -> bool:
        """Send an error alert."""
        return await self.send_alert(
            title=f"Error: {error_type}",
            message=message,
            color="#f87171",  # Red
        )

    async def send_daily_summary(
        self,
        total_trades: int,
        total_pnl: float,
        win_rate: float,
    ) -> bool:
        """Send a brief daily summary."""
        pnl_emoji = "📈" if total_pnl >= 0 else "📉"
        color = "#4ade80" if total_pnl >= 0 else "#f87171"

        return await self.send_alert(
            title=f"Daily Summary {pnl_emoji}",
            message=f"${total_pnl:,.2f} P&L | {win_rate:.0%} Win Rate | {total_trades} Trades",
            color=color,
        )
