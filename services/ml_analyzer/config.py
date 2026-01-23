"""Configuration for ML Analyzer service."""

import os
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class AnalyzerConfig:
    """Configuration for the ML analyzer service.

    Attributes:
        run_time: Time to run nightly analysis (HH:MM format, 24-hour).
        min_trades_for_ml: Minimum trades required to train ML models.
        lookback_days: Days of historical data to analyze.
        model_retrain_interval_days: How often to retrain ML models.
        report_output_dir: Directory to save reports.
        enable_slack_delivery: Whether to send reports to Slack.
        slack_webhook_url: Slack webhook URL for report delivery.
    """
    run_time: str = "23:00"
    min_trades_for_ml: int = 100
    lookback_days: int = 30
    model_retrain_interval_days: int = 7
    report_output_dir: str = "reports"
    enable_slack_delivery: bool = False
    slack_webhook_url: Optional[str] = None

    # Feature extraction settings
    edge_buckets: list[float] = field(default_factory=lambda: [1.0, 2.0, 3.0, 5.0, 10.0])

    # Model settings
    model_max_depth: int = 5
    model_n_estimators: int = 100
    model_min_samples_split: int = 10

    @classmethod
    def from_env(cls) -> "AnalyzerConfig":
        """Create config from environment variables."""
        return cls(
            run_time=os.environ.get("ML_ANALYZER_RUN_TIME", "23:00"),
            min_trades_for_ml=int(os.environ.get("ML_MIN_TRADES", "100")),
            lookback_days=int(os.environ.get("ML_LOOKBACK_DAYS", "30")),
            model_retrain_interval_days=int(os.environ.get("ML_RETRAIN_INTERVAL", "7")),
            report_output_dir=os.environ.get("ML_REPORT_DIR", "reports"),
            enable_slack_delivery=os.environ.get("ML_SLACK_ENABLED", "false").lower() == "true",
            slack_webhook_url=os.environ.get("ML_SLACK_WEBHOOK"),
        )
