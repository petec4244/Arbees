"""
Feature extraction for ML models.

Extracts features from trades and signals for training trade success
prediction models and performance analysis.
"""

from dataclasses import dataclass
from datetime import datetime
from typing import Any, Optional
import logging

logger = logging.getLogger(__name__)


@dataclass
class TradeFeatures:
    """Features extracted from a single trade."""
    # Identifiers (not used in ML, for reference)
    trade_id: str
    game_id: str

    # Categorical features (encoded as integers)
    sport_encoded: int
    market_type_encoded: int
    signal_type_encoded: int
    side_encoded: int  # 0 = sell, 1 = buy
    platform_encoded: int

    # Continuous features - Trade context
    edge_at_entry: float
    model_prob: float
    market_prob: float
    size: float
    entry_price: float

    # Game context features
    game_period: int  # 1, 2, 3, 4, OT=5
    time_remaining_seconds: int
    score_differential: int  # home_score - away_score
    is_home_favorite: int  # 1 if entry_price >= 0.5, else 0

    # Timing features
    hour_of_day: int
    day_of_week: int
    is_weekend: int

    # Derived features
    edge_bucket: int  # Bucketed edge: 0=<1%, 1=1-2%, 2=2-3%, 3=3-5%, 4=>5%
    prob_gap: float  # |model_prob - market_prob|
    position_risk: float  # size * entry_price for buy, size * (1 - entry_price) for sell

    # Target (for supervised learning)
    outcome: int  # 0 = loss, 1 = win, -1 = push/unknown
    pnl: float


class FeatureExtractor:
    """
    Extracts ML features from trades and signals.

    Features are designed to capture:
    1. Trade characteristics (edge, size, side)
    2. Game context (sport, period, score)
    3. Timing patterns (hour, day of week)
    4. Model vs market disagreement
    """

    # Encoding maps
    SPORT_MAP = {
        "nba": 0, "nfl": 1, "nhl": 2, "mlb": 3,
        "ncaaf": 4, "ncaab": 5, "soccer": 6, "mls": 7,
        "tennis": 8, "mma": 9,
    }

    MARKET_TYPE_MAP = {
        "moneyline": 0, "spread": 1, "total": 2,
    }

    SIGNAL_TYPE_MAP = {
        "model_edge_yes": 0,
        "model_edge_no": 1,
        "cross_market_arb": 2,
        "win_prob_shift": 3,
        "mean_reversion": 4,
        "momentum": 5,
    }

    PLATFORM_MAP = {
        "kalshi": 0, "polymarket": 1, "paper": 2,
    }

    PERIOD_MAP = {
        "1": 1, "2": 2, "3": 3, "4": 4,
        "q1": 1, "q2": 2, "q3": 3, "q4": 4,
        "1st": 1, "2nd": 2, "3rd": 3, "4th": 4,
        "ot": 5, "overtime": 5, "so": 5,
        "h1": 1, "h2": 2,  # Soccer halves
    }

    OUTCOME_MAP = {
        "win": 1, "loss": 0, "push": -1,
    }

    # Edge buckets: [0, 1), [1, 2), [2, 3), [3, 5), [5, inf)
    EDGE_BUCKETS = [1.0, 2.0, 3.0, 5.0]

    def __init__(self):
        """Initialize the feature extractor."""
        pass

    def extract_trade_features(
        self,
        trade: dict[str, Any],
        signal: Optional[dict[str, Any]] = None,
    ) -> TradeFeatures:
        """
        Extract features from a single trade.

        Args:
            trade: Trade record from database (archived_trades or paper_trades)
            signal: Optional signal record that triggered the trade

        Returns:
            TradeFeatures dataclass with all extracted features
        """
        # Get timing from trade
        opened_at = self._parse_datetime(trade.get("opened_at") or trade.get("time"))

        # Encode categorical features
        sport = (trade.get("sport") or "").lower()
        sport_encoded = self.SPORT_MAP.get(sport, -1)

        market_type = (trade.get("market_type") or "moneyline").lower()
        market_type_encoded = self.MARKET_TYPE_MAP.get(market_type, 0)

        signal_type = (trade.get("signal_type") or "").lower()
        signal_type_encoded = self.SIGNAL_TYPE_MAP.get(signal_type, -1)

        side = (trade.get("side") or "").lower()
        side_encoded = 1 if side == "buy" else 0

        platform = (trade.get("platform") or "").lower()
        platform_encoded = self.PLATFORM_MAP.get(platform, 2)

        # Continuous features
        edge_at_entry = float(trade.get("edge_at_entry") or 0)
        model_prob = float(trade.get("model_prob") or trade.get("model_prob_at_entry") or 0)
        market_prob = float(trade.get("market_prob_at_entry") or 0)
        size = float(trade.get("size") or 0)
        entry_price = float(trade.get("entry_price") or 0)

        # If we have a signal, use its probabilities as fallback
        if signal:
            if model_prob == 0:
                model_prob = float(signal.get("model_prob") or 0)
            if market_prob == 0:
                market_prob = float(signal.get("market_prob") or 0)
            if edge_at_entry == 0:
                edge_at_entry = float(signal.get("edge_pct") or 0)

        # Game context
        game_period_str = (trade.get("game_period_at_entry") or trade.get("game_period") or "1").lower()
        game_period = self.PERIOD_MAP.get(game_period_str, 1)

        time_remaining = int(trade.get("time_remaining_at_entry") or 0)
        score_diff = int(trade.get("score_diff_at_entry") or 0)

        # Timing features
        hour = opened_at.hour if opened_at else 12
        dow = opened_at.weekday() if opened_at else 0
        is_weekend = 1 if dow >= 5 else 0

        # Derived features
        edge_bucket = self._bucket_edge(edge_at_entry)
        prob_gap = abs(model_prob - market_prob) if model_prob and market_prob else 0

        if side_encoded == 1:  # buy
            position_risk = size * entry_price
            is_home_favorite = 1 if entry_price >= 0.5 else 0
        else:  # sell
            position_risk = size * (1 - entry_price)
            is_home_favorite = 1 if entry_price < 0.5 else 0

        # Target
        outcome_str = (trade.get("outcome") or "").lower()
        outcome = self.OUTCOME_MAP.get(outcome_str, -1)
        pnl = float(trade.get("pnl") or 0)

        return TradeFeatures(
            trade_id=trade.get("trade_id", ""),
            game_id=trade.get("game_id", ""),
            sport_encoded=sport_encoded,
            market_type_encoded=market_type_encoded,
            signal_type_encoded=signal_type_encoded,
            side_encoded=side_encoded,
            platform_encoded=platform_encoded,
            edge_at_entry=edge_at_entry,
            model_prob=model_prob,
            market_prob=market_prob,
            size=size,
            entry_price=entry_price,
            game_period=game_period,
            time_remaining_seconds=time_remaining,
            score_differential=score_diff,
            is_home_favorite=is_home_favorite,
            hour_of_day=hour,
            day_of_week=dow,
            is_weekend=is_weekend,
            edge_bucket=edge_bucket,
            prob_gap=prob_gap,
            position_risk=position_risk,
            outcome=outcome,
            pnl=pnl,
        )

    def extract_batch(
        self,
        trades: list[dict[str, Any]],
        signals: Optional[list[dict[str, Any]]] = None,
    ) -> list[TradeFeatures]:
        """
        Extract features from a batch of trades.

        Args:
            trades: List of trade records
            signals: Optional list of signal records (matched by signal_id)

        Returns:
            List of TradeFeatures
        """
        # Build signal lookup
        signal_map: dict[str, dict] = {}
        if signals:
            for sig in signals:
                sig_id = sig.get("signal_id")
                if sig_id:
                    signal_map[sig_id] = sig

        features = []
        for trade in trades:
            signal_id = trade.get("signal_id")
            signal = signal_map.get(signal_id) if signal_id else None

            try:
                feat = self.extract_trade_features(trade, signal)
                features.append(feat)
            except Exception as e:
                logger.warning(f"Failed to extract features for trade {trade.get('trade_id')}: {e}")

        return features

    def to_feature_matrix(
        self,
        features: list[TradeFeatures],
        include_target: bool = True,
    ) -> tuple[list[list[float]], list[str], Optional[list[int]]]:
        """
        Convert TradeFeatures to a feature matrix suitable for ML.

        Args:
            features: List of TradeFeatures
            include_target: Whether to include outcome as target

        Returns:
            Tuple of (feature_matrix, feature_names, targets)
            - feature_matrix: List of feature vectors (one per trade)
            - feature_names: List of feature column names
            - targets: List of outcomes (1=win, 0=loss) or None if not included
        """
        feature_names = [
            "sport_encoded",
            "market_type_encoded",
            "signal_type_encoded",
            "side_encoded",
            "platform_encoded",
            "edge_at_entry",
            "model_prob",
            "market_prob",
            "size",
            "entry_price",
            "game_period",
            "time_remaining_seconds",
            "score_differential",
            "is_home_favorite",
            "hour_of_day",
            "day_of_week",
            "is_weekend",
            "edge_bucket",
            "prob_gap",
            "position_risk",
        ]

        matrix = []
        targets = [] if include_target else None

        for feat in features:
            row = [
                feat.sport_encoded,
                feat.market_type_encoded,
                feat.signal_type_encoded,
                feat.side_encoded,
                feat.platform_encoded,
                feat.edge_at_entry,
                feat.model_prob,
                feat.market_prob,
                feat.size,
                feat.entry_price,
                feat.game_period,
                feat.time_remaining_seconds,
                feat.score_differential,
                feat.is_home_favorite,
                feat.hour_of_day,
                feat.day_of_week,
                feat.is_weekend,
                feat.edge_bucket,
                feat.prob_gap,
                feat.position_risk,
            ]
            matrix.append(row)

            if include_target:
                # Only include win/loss, skip push (-1)
                targets.append(feat.outcome)

        return matrix, feature_names, targets

    def to_dataframe(self, features: list[TradeFeatures]):
        """
        Convert TradeFeatures to a pandas DataFrame.

        Requires pandas to be installed.

        Args:
            features: List of TradeFeatures

        Returns:
            pandas DataFrame with all features
        """
        try:
            import pandas as pd
        except ImportError:
            raise ImportError("pandas is required for to_dataframe()")

        data = []
        for feat in features:
            data.append({
                "trade_id": feat.trade_id,
                "game_id": feat.game_id,
                "sport_encoded": feat.sport_encoded,
                "market_type_encoded": feat.market_type_encoded,
                "signal_type_encoded": feat.signal_type_encoded,
                "side_encoded": feat.side_encoded,
                "platform_encoded": feat.platform_encoded,
                "edge_at_entry": feat.edge_at_entry,
                "model_prob": feat.model_prob,
                "market_prob": feat.market_prob,
                "size": feat.size,
                "entry_price": feat.entry_price,
                "game_period": feat.game_period,
                "time_remaining_seconds": feat.time_remaining_seconds,
                "score_differential": feat.score_differential,
                "is_home_favorite": feat.is_home_favorite,
                "hour_of_day": feat.hour_of_day,
                "day_of_week": feat.day_of_week,
                "is_weekend": feat.is_weekend,
                "edge_bucket": feat.edge_bucket,
                "prob_gap": feat.prob_gap,
                "position_risk": feat.position_risk,
                "outcome": feat.outcome,
                "pnl": feat.pnl,
            })

        return pd.DataFrame(data)

    def _bucket_edge(self, edge: float) -> int:
        """
        Bucket edge percentage into categories.

        Buckets: [0, 1), [1, 2), [2, 3), [3, 5), [5, inf)
        Returns: 0, 1, 2, 3, 4
        """
        for i, threshold in enumerate(self.EDGE_BUCKETS):
            if edge < threshold:
                return i
        return len(self.EDGE_BUCKETS)

    def _parse_datetime(self, value) -> Optional[datetime]:
        """Parse datetime from various formats."""
        if value is None:
            return None
        if isinstance(value, datetime):
            return value
        if isinstance(value, str):
            try:
                # Try ISO format
                return datetime.fromisoformat(value.replace("Z", "+00:00"))
            except ValueError:
                pass
        return None

    @staticmethod
    def get_feature_names() -> list[str]:
        """Get feature names for the feature matrix columns."""
        return [
            "sport_encoded",
            "market_type_encoded",
            "signal_type_encoded",
            "side_encoded",
            "platform_encoded",
            "edge_at_entry",
            "model_prob",
            "market_prob",
            "size",
            "entry_price",
            "game_period",
            "time_remaining_seconds",
            "score_differential",
            "is_home_favorite",
            "hour_of_day",
            "day_of_week",
            "is_weekend",
            "edge_bucket",
            "prob_gap",
            "position_risk",
        ]

    @staticmethod
    def get_feature_importance_names() -> list[str]:
        """Get human-readable feature names for importance plots."""
        return [
            "Sport",
            "Market Type",
            "Signal Type",
            "Side (Buy/Sell)",
            "Platform",
            "Edge at Entry",
            "Model Probability",
            "Market Probability",
            "Position Size",
            "Entry Price",
            "Game Period",
            "Time Remaining",
            "Score Differential",
            "Home Favorite",
            "Hour of Day",
            "Day of Week",
            "Weekend",
            "Edge Bucket",
            "Model-Market Gap",
            "Position Risk",
        ]
