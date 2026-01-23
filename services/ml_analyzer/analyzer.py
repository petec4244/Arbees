"""
ML Analyzer Service - Main entry point.

Orchestrates nightly analysis, report generation, and ML model training.
Runs on a schedule (default 11pm) after markets close.
"""

import asyncio
import json
import logging
import os
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Optional

from arbees_shared.db.connection import get_pool, close_pool

from .config import AnalyzerConfig
from .data_loader import DataLoader
from .feature_extractor import FeatureExtractor, TradeFeatures
from .insights import InsightExtractor, PerformanceInsights, Recommendation
from .report_generator import ReportGenerator
from .delivery import ReportDeliveryService, DeliveryConfig
from .models import TradeSuccessModel, ParameterOptimizer
from .validation import ModelValidator, DataDriftDetector
from .anomaly_detector import AnomalyDetector, AnomalyReport

logger = logging.getLogger(__name__)


class MLAnalyzer:
    """
    ML Analyzer service for nightly performance analysis.

    Features:
    - Scheduled nightly runs (11pm by default)
    - Loads historical trades and signals
    - Extracts ML features
    - Generates performance insights
    - Creates hot wash reports (Markdown/HTML)
    - Saves reports to database and files
    - Optional Slack delivery

    Future:
    - ML model training for trade success prediction
    - Parameter optimization recommendations
    """

    def __init__(self, config: Optional[AnalyzerConfig] = None):
        """Initialize the ML Analyzer.

        Args:
            config: Configuration options. Uses defaults if None.
        """
        self.config = config or AnalyzerConfig.from_env()
        self.data_loader = DataLoader()
        self.feature_extractor = FeatureExtractor()
        self.insight_extractor = InsightExtractor()
        self.report_generator = ReportGenerator()

        # ML models
        model_dir = Path(self.config.report_output_dir) / "models"
        self.model_path = model_dir / "trade_success_model.pkl"
        self.trade_model = TradeSuccessModel(model_path=str(self.model_path))
        self.optimizer = ParameterOptimizer()

        # Validation
        self.validator = ModelValidator()
        self.drift_detector = DataDriftDetector()

        # Anomaly detection
        self.anomaly_detector = AnomalyDetector()

        # Report delivery
        self.delivery_service = ReportDeliveryService(
            config=DeliveryConfig.from_env(),
            report_generator=self.report_generator,
        )

        self._running = False
        self._last_run: Optional[date] = None
        self._model_loaded = False
        self._redis = None

    async def start(self) -> None:
        """Start the ML analyzer with scheduled runs."""
        logger.info(f"ML Analyzer starting, scheduled for {self.config.run_time} daily")

        self._running = True

        # Ensure report directory exists
        Path(self.config.report_output_dir).mkdir(parents=True, exist_ok=True)

        # Connect to Redis for on-demand requests
        try:
            from arbees_shared.messaging.redis_bus import RedisBus
            self._redis = RedisBus()
            await self._redis.connect()
            await self._redis.subscribe("ml:analysis:request", self._handle_analysis_request)
            asyncio.create_task(self._redis.start_listening())
            logger.info("Subscribed to ml:analysis:request channel")
        except Exception as e:
            logger.warning(f"Could not connect to Redis for on-demand requests: {e}")
            self._redis = None

        # Run scheduler loop
        while self._running:
            try:
                await self._check_and_run()
            except Exception as e:
                logger.error(f"Error in scheduler loop: {e}", exc_info=True)

            # Check every minute
            await asyncio.sleep(60)

    async def stop(self) -> None:
        """Stop the ML analyzer."""
        logger.info("Stopping ML Analyzer")
        self._running = False

        # Disconnect from Redis
        if self._redis:
            try:
                await self._redis.disconnect()
            except Exception as e:
                logger.warning(f"Error disconnecting from Redis: {e}")

    async def _handle_analysis_request(self, message: dict) -> None:
        """Handle on-demand analysis request from Redis.

        Args:
            message: Request message with 'date' field
        """
        try:
            request_date = message.get("date")
            if not request_date:
                logger.warning("Received analysis request without date")
                return

            # Parse date
            if isinstance(request_date, str):
                analysis_date = datetime.strptime(request_date, "%Y-%m-%d").date()
            else:
                analysis_date = request_date

            logger.info(f"Processing on-demand analysis request for {analysis_date}")
            await self.run_nightly_analysis(analysis_date)

            # Publish completion notification
            if self._redis:
                await self._redis.publish("ml:analysis:complete", {
                    "date": analysis_date.isoformat(),
                    "completed_at": datetime.utcnow().isoformat(),
                    "status": "success",
                })

        except Exception as e:
            logger.error(f"Error processing analysis request: {e}", exc_info=True)

            # Publish failure notification
            if self._redis:
                await self._redis.publish("ml:analysis:complete", {
                    "date": message.get("date"),
                    "completed_at": datetime.utcnow().isoformat(),
                    "status": "error",
                    "error": str(e),
                })

    async def _check_and_run(self) -> None:
        """Check if it's time to run and execute if so."""
        now = datetime.now()
        today = now.date()

        # Parse run time
        hour, minute = map(int, self.config.run_time.split(":"))
        run_time = now.replace(hour=hour, minute=minute, second=0, microsecond=0)

        # Check if we should run
        if now >= run_time and self._last_run != today:
            logger.info(f"Starting scheduled analysis for {today}")
            await self.run_nightly_analysis(today)
            self._last_run = today

    async def run_nightly_analysis(self, for_date: Optional[date] = None) -> PerformanceInsights:
        """
        Run the full nightly analysis pipeline.

        Steps:
        1. Load trades and signals for the date
        2. Extract features for ML
        3. Generate insights
        4. Train/retrain ML model (if conditions met)
        5. Run parameter optimization
        6. Add ML insights to report
        7. Create reports
        8. Save to database and files
        9. Deliver via Slack (if enabled)

        Args:
            for_date: Date to analyze. Defaults to yesterday.

        Returns:
            PerformanceInsights from the analysis
        """
        analysis_date = for_date or (date.today() - timedelta(days=1))
        logger.info(f"Running nightly analysis for {analysis_date}")

        try:
            # 1. Load data for the specific date
            trades = await self.data_loader.load_trades_for_date(analysis_date)
            signals = await self.data_loader.load_signals_for_date(analysis_date)

            if not trades:
                logger.info(f"No trades for {analysis_date}, generating empty report")

            # 2. Get current parameters for recommendations
            current_params = await self.data_loader.get_current_parameters()

            # 3. Generate base insights
            insights = self.insight_extractor.analyze(
                trades=trades,
                signals=signals,
                for_date=analysis_date,
                current_params=current_params,
            )

            # 3.5 Run anomaly detection
            anomaly_report = self.anomaly_detector.analyze(
                trades=trades,
                for_date=analysis_date,
            )

            # Add anomaly results to insights
            insights.anomaly_count = len(anomaly_report.anomalies)
            insights.critical_anomalies = anomaly_report.critical_count
            if anomaly_report.anomalies:
                insights.anomaly_summary = self.anomaly_detector.format_report(anomaly_report)
                logger.warning(
                    f"Anomaly detection found {insights.critical_anomalies} critical, "
                    f"{anomaly_report.warning_count} warnings"
                )
            else:
                insights.anomaly_summary = "No anomalies detected."

            # 4. Load historical data for ML training (lookback period)
            historical_trades = await self.data_loader.load_historical_trades(
                days=self.config.lookback_days
            )

            # 5. Extract ML features from historical trades
            ml_features = self._extract_ml_features(historical_trades)

            # 6. Train/retrain ML model if conditions are met
            model_metrics = await self._maybe_train_model(ml_features)
            if model_metrics:
                insights.model_accuracy = model_metrics.get("accuracy")
                insights.feature_importance = self._get_feature_importance_dict()
                logger.info(f"Model trained with accuracy: {insights.model_accuracy:.3f}")

            # 7. Run parameter optimization
            optimization_results = self._run_optimization(ml_features, current_params)
            self._add_optimization_recommendations(insights, optimization_results)

            # 8. Generate reports
            report_md = self.report_generator.generate_markdown(insights)
            report_html = self.report_generator.generate_html(insights)

            # 9. Save to database
            await self._save_report_to_db(analysis_date, insights, report_md, report_html)

            # 10. Deliver via all configured channels (file, Slack, email)
            delivery_results = await self.delivery_service.deliver_report(
                insights, report_md, report_html
            )

            # Log delivery results
            for result in delivery_results:
                if result.success:
                    logger.info(f"Delivery to {result.channel}: {result.message}")
                else:
                    logger.warning(f"Delivery to {result.channel} failed: {result.message}")

            logger.info(f"Nightly analysis complete for {analysis_date}: "
                       f"{insights.total_trades} trades, ${insights.total_pnl:.2f} P&L")

            return insights

        except Exception as e:
            logger.error(f"Nightly analysis failed for {analysis_date}: {e}", exc_info=True)
            raise

    def _extract_ml_features(self, trades: list[dict]) -> list[TradeFeatures]:
        """Extract ML features from historical trade data.

        Args:
            trades: List of trade dictionaries from database

        Returns:
            List of TradeFeatures for ML processing
        """
        if not trades:
            return []

        features = []
        for trade in trades:
            try:
                feature = self.feature_extractor.extract_trade_features(trade)
                if feature:
                    features.append(feature)
            except Exception as e:
                logger.warning(f"Failed to extract features for trade {trade.get('id')}: {e}")

        logger.info(f"Extracted {len(features)} feature sets from {len(trades)} trades")
        return features

    async def _maybe_train_model(self, features: list[TradeFeatures]) -> Optional[dict]:
        """Train the ML model if conditions are met.

        Conditions for training:
        - At least min_trades (default 100) samples
        - Either no existing model, or retrain_interval_days have passed
        - Data passes validation checks

        Args:
            features: List of extracted features

        Returns:
            Training metrics if trained, None otherwise
        """
        if len(features) < self.config.min_trades_for_ml:
            logger.info(f"Insufficient data for training: {len(features)} < {self.config.min_trades_for_ml}")
            return None

        # Validate training data before proceeding
        validation_report = self.validator.validate_training_data(features)
        if not validation_report.overall_passed:
            logger.warning(f"Training data validation failed: {validation_report.warnings}")
            for rec in validation_report.recommendations:
                logger.info(f"Recommendation: {rec}")
            return None

        # Log validation results
        for result in validation_report.results:
            logger.debug(f"Validation: {result.message}")

        # Check if model exists and when it was last trained
        should_retrain = True
        if self.model_path.exists():
            try:
                if not self._model_loaded:
                    self.trade_model.load(str(self.model_path))
                    self._model_loaded = True

                if self.trade_model._trained_at:
                    days_since_training = (datetime.now() - self.trade_model._trained_at).days
                    if days_since_training < self.config.model_retrain_interval_days:
                        logger.info(f"Model trained {days_since_training} days ago, "
                                   f"next retrain in {self.config.model_retrain_interval_days - days_since_training} days")
                        should_retrain = False
            except Exception as e:
                logger.warning(f"Could not load existing model: {e}")
                should_retrain = True

        if not should_retrain:
            return None

        # Prepare training data
        X, feature_names, y = self.feature_extractor.to_feature_matrix(features)

        if len(X) < 50:
            logger.warning(f"Too few valid samples for training: {len(X)}")
            return None

        # Train model
        logger.info(f"Training trade success model on {len(X)} samples")
        metrics = self.trade_model.train(X, y, feature_names)

        if "error" in metrics:
            logger.error(f"Model training failed: {metrics['error']}")
            return None

        # Validate model performance
        perf_validation = self.validator.validate_model_performance(
            accuracy=metrics.get("accuracy", 0),
            precision=metrics.get("precision", 0),
            recall=metrics.get("recall", 0),
        )

        if not perf_validation.overall_passed:
            logger.warning(f"Model performance validation warnings: {perf_validation.warnings}")
            for rec in perf_validation.recommendations:
                logger.info(f"Recommendation: {rec}")
            # Still save the model but log warnings

        # Save model
        model_dir = self.model_path.parent
        model_dir.mkdir(parents=True, exist_ok=True)
        self.trade_model.save(str(self.model_path))
        self._model_loaded = True

        # Add validation info to metrics
        metrics["validation_passed"] = perf_validation.overall_passed
        metrics["validation_warnings"] = perf_validation.warnings

        return metrics

    def _get_feature_importance_dict(self) -> dict[str, float]:
        """Get feature importance from trained model."""
        if not self.trade_model.is_trained:
            return {}

        return dict(self.trade_model.get_top_features(10))

    def _run_optimization(
        self,
        features: list[TradeFeatures],
        current_params: dict,
    ) -> list:
        """Run parameter optimization on historical features.

        Args:
            features: List of trade features
            current_params: Current parameter values

        Returns:
            List of OptimizationResult objects
        """
        if len(features) < 20:
            logger.info("Insufficient data for optimization")
            return []

        try:
            results = self.optimizer.optimize_all(features, current_params)
            logger.info(f"Generated {len(results)} optimization recommendations")
            return results
        except Exception as e:
            logger.error(f"Optimization failed: {e}")
            return []

    def _add_optimization_recommendations(
        self,
        insights: PerformanceInsights,
        optimization_results: list,
    ) -> None:
        """Add optimization results as recommendations to insights.

        Args:
            insights: PerformanceInsights to update
            optimization_results: List of OptimizationResult from optimizer
        """
        for opt_result in optimization_results:
            # Only add recommendations with meaningful improvement
            if abs(opt_result.expected_improvement) < 0.05:
                continue

            impact = "high" if abs(opt_result.expected_improvement) > 0.20 else "medium"

            rec = Recommendation(
                title=f"Adjust {opt_result.parameter}",
                parameter=opt_result.parameter,
                current=opt_result.current_value,
                recommended=opt_result.optimal_value,
                impact=impact,
                confidence=opt_result.confidence,
                rationale=opt_result.rationale,
            )
            insights.recommendations.append(rec)

    async def run_on_demand(self, for_date: date) -> str:
        """
        Run analysis on-demand and return the markdown report.

        Args:
            for_date: Date to analyze

        Returns:
            Markdown report string
        """
        insights = await self.run_nightly_analysis(for_date)
        return self.report_generator.generate_markdown(insights)

    async def get_historical_insights(self, days: int = 30) -> dict:
        """
        Get aggregated insights over a time period.

        Args:
            days: Number of days to analyze

        Returns:
            Dictionary with aggregated performance metrics
        """
        by_sport = await self.data_loader.get_performance_by_sport(days)
        by_signal_type = await self.data_loader.get_performance_by_signal_type(days)
        by_edge = await self.data_loader.get_performance_by_edge_bucket(days)

        # Calculate totals
        total_trades = sum(s["trades"] for s in by_sport.values())
        total_wins = sum(s["wins"] for s in by_sport.values())
        total_pnl = sum(s["pnl"] for s in by_sport.values())

        return {
            "period_days": days,
            "total_trades": total_trades,
            "total_wins": total_wins,
            "total_pnl": total_pnl,
            "win_rate": total_wins / total_trades if total_trades > 0 else 0,
            "by_sport": by_sport,
            "by_signal_type": by_signal_type,
            "by_edge_range": by_edge,
        }

    async def _save_report_to_db(
        self,
        report_date: date,
        insights: PerformanceInsights,
        report_md: str,
        report_html: str,
    ) -> None:
        """Save report to ml_analysis_reports table."""
        pool = await get_pool()

        # Prepare recommendations as JSON
        recommendations_json = [
            {
                "title": r.title,
                "parameter": r.parameter,
                "current": str(r.current),
                "recommended": str(r.recommended),
                "impact": r.impact,
                "confidence": r.confidence,
                "rationale": r.rationale,
            }
            for r in insights.recommendations
        ]

        await pool.execute("""
            INSERT INTO ml_analysis_reports (
                report_date, generated_at,
                total_games, total_trades, total_pnl, win_rate,
                best_sport, best_sport_win_rate,
                worst_sport, worst_sport_win_rate,
                signals_generated, signals_executed,
                missed_opportunity_reasons, recommendations,
                model_accuracy, feature_importance,
                report_markdown, report_html
            ) VALUES (
                $1, NOW(),
                $2, $3, $4, $5,
                $6, $7,
                $8, $9,
                $10, $11,
                $12, $13,
                $14, $15,
                $16, $17
            )
            ON CONFLICT (report_date) DO UPDATE SET
                generated_at = NOW(),
                total_trades = EXCLUDED.total_trades,
                total_pnl = EXCLUDED.total_pnl,
                win_rate = EXCLUDED.win_rate,
                best_sport = EXCLUDED.best_sport,
                best_sport_win_rate = EXCLUDED.best_sport_win_rate,
                worst_sport = EXCLUDED.worst_sport,
                worst_sport_win_rate = EXCLUDED.worst_sport_win_rate,
                signals_generated = EXCLUDED.signals_generated,
                signals_executed = EXCLUDED.signals_executed,
                missed_opportunity_reasons = EXCLUDED.missed_opportunity_reasons,
                recommendations = EXCLUDED.recommendations,
                report_markdown = EXCLUDED.report_markdown,
                report_html = EXCLUDED.report_html
        """,
            report_date,
            0,  # total_games - would need to count from archived_games
            insights.total_trades,
            insights.total_pnl,
            insights.win_rate,
            insights.best_sport.name if insights.best_sport else None,
            insights.best_sport.win_rate if insights.best_sport else None,
            insights.worst_sport.name if insights.worst_sport else None,
            insights.worst_sport.win_rate if insights.worst_sport else None,
            insights.signals_generated,
            insights.signals_executed,
            insights.missed_reasons,  # JSONB
            recommendations_json,  # JSONB
            insights.model_accuracy,
            insights.feature_importance,  # JSONB
            report_md,
            report_html,
        )

        logger.info(f"Saved report to database for {report_date}")


# Entry point for running as standalone service
async def main():
    """Run MLAnalyzer as standalone service."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
    )

    analyzer = MLAnalyzer()

    try:
        await analyzer.start()
    except KeyboardInterrupt:
        pass
    finally:
        await analyzer.stop()
        await close_pool()


if __name__ == "__main__":
    asyncio.run(main())
