"""
Trade Success Prediction Model.

Uses a Random Forest classifier to predict whether a trade will be successful
based on features extracted from trade context.
"""

import logging
import pickle
from datetime import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)


class TradeSuccessModel:
    """
    Random Forest classifier for trade success prediction.

    Predicts binary outcome (win/loss) based on trade features.
    Uses scikit-learn's RandomForestClassifier.

    Features used:
    - Sport, market type, signal type (encoded)
    - Edge at entry, model/market probability
    - Game context (period, score diff, time remaining)
    - Timing (hour, day of week)
    """

    def __init__(
        self,
        max_depth: int = 5,
        n_estimators: int = 100,
        min_samples_split: int = 10,
        model_path: Optional[str] = None,
    ):
        """Initialize the model.

        Args:
            max_depth: Maximum tree depth (prevents overfitting)
            n_estimators: Number of trees in the forest
            min_samples_split: Minimum samples required to split
            model_path: Optional path to save/load model
        """
        self.max_depth = max_depth
        self.n_estimators = n_estimators
        self.min_samples_split = min_samples_split
        self.model_path = model_path

        self._model = None
        self._feature_names: list[str] = []
        self._feature_importance: dict[str, float] = {}
        self._trained_at: Optional[datetime] = None
        self._training_samples: int = 0
        self._accuracy: Optional[float] = None

    @property
    def is_trained(self) -> bool:
        """Check if model has been trained."""
        return self._model is not None

    @property
    def feature_importance(self) -> dict[str, float]:
        """Get feature importance from trained model."""
        return self._feature_importance

    @property
    def accuracy(self) -> Optional[float]:
        """Get model accuracy from training."""
        return self._accuracy

    def train(
        self,
        X: list[list[float]],
        y: list[int],
        feature_names: list[str],
        test_size: float = 0.2,
    ) -> dict:
        """
        Train the model on historical trade data.

        Args:
            X: Feature matrix (list of feature vectors)
            y: Target labels (0=loss, 1=win)
            feature_names: Names of features for importance tracking
            test_size: Fraction of data to use for testing

        Returns:
            Dictionary with training metrics
        """
        try:
            from sklearn.ensemble import RandomForestClassifier
            from sklearn.model_selection import train_test_split
            from sklearn.metrics import accuracy_score, precision_score, recall_score
        except ImportError:
            logger.error("scikit-learn not installed. Install with: pip install scikit-learn")
            return {"error": "scikit-learn not installed"}

        # Filter out push trades (outcome = -1)
        valid_indices = [i for i, label in enumerate(y) if label in (0, 1)]
        if len(valid_indices) < 50:
            logger.warning(f"Only {len(valid_indices)} valid samples, need at least 50")
            return {"error": "insufficient_data", "samples": len(valid_indices)}

        X_valid = [X[i] for i in valid_indices]
        y_valid = [y[i] for i in valid_indices]

        # Split data
        X_train, X_test, y_train, y_test = train_test_split(
            X_valid, y_valid, test_size=test_size, random_state=42, stratify=y_valid
        )

        # Train model
        self._model = RandomForestClassifier(
            n_estimators=self.n_estimators,
            max_depth=self.max_depth,
            min_samples_split=self.min_samples_split,
            random_state=42,
            n_jobs=-1,
        )
        self._model.fit(X_train, y_train)

        # Evaluate
        y_pred = self._model.predict(X_test)
        accuracy = accuracy_score(y_test, y_pred)
        precision = precision_score(y_test, y_pred, zero_division=0)
        recall = recall_score(y_test, y_pred, zero_division=0)

        # Store feature importance
        self._feature_names = feature_names
        importances = self._model.feature_importances_
        self._feature_importance = {
            name: float(imp)
            for name, imp in zip(feature_names, importances)
        }

        # Store metadata
        self._trained_at = datetime.now()
        self._training_samples = len(X_valid)
        self._accuracy = accuracy

        # Save model if path provided
        if self.model_path:
            self.save(self.model_path)

        metrics = {
            "accuracy": accuracy,
            "precision": precision,
            "recall": recall,
            "train_samples": len(X_train),
            "test_samples": len(X_test),
            "total_samples": len(X_valid),
            "trained_at": self._trained_at.isoformat(),
        }

        logger.info(f"Model trained: accuracy={accuracy:.3f}, precision={precision:.3f}, "
                   f"recall={recall:.3f}, samples={len(X_valid)}")

        return metrics

    def predict(self, X: list[list[float]]) -> list[int]:
        """
        Predict trade outcomes.

        Args:
            X: Feature matrix

        Returns:
            List of predictions (0=loss, 1=win)
        """
        if not self.is_trained:
            raise ValueError("Model not trained. Call train() first.")

        return self._model.predict(X).tolist()

    def predict_proba(self, X: list[list[float]]) -> list[tuple[float, float]]:
        """
        Predict trade outcome probabilities.

        Args:
            X: Feature matrix

        Returns:
            List of (loss_prob, win_prob) tuples
        """
        if not self.is_trained:
            raise ValueError("Model not trained. Call train() first.")

        probas = self._model.predict_proba(X)
        return [(float(p[0]), float(p[1])) for p in probas]

    def predict_single(self, features: list[float]) -> tuple[int, float]:
        """
        Predict outcome for a single trade.

        Args:
            features: Feature vector for one trade

        Returns:
            Tuple of (prediction, confidence)
        """
        if not self.is_trained:
            raise ValueError("Model not trained. Call train() first.")

        pred = self._model.predict([features])[0]
        proba = self._model.predict_proba([features])[0]
        confidence = max(proba)

        return int(pred), float(confidence)

    def get_top_features(self, n: int = 5) -> list[tuple[str, float]]:
        """
        Get the top N most important features.

        Args:
            n: Number of features to return

        Returns:
            List of (feature_name, importance) tuples
        """
        if not self._feature_importance:
            return []

        sorted_features = sorted(
            self._feature_importance.items(),
            key=lambda x: x[1],
            reverse=True
        )
        return sorted_features[:n]

    def save(self, path: str) -> None:
        """Save model to file."""
        if not self.is_trained:
            raise ValueError("No model to save")

        model_data = {
            "model": self._model,
            "feature_names": self._feature_names,
            "feature_importance": self._feature_importance,
            "trained_at": self._trained_at,
            "training_samples": self._training_samples,
            "accuracy": self._accuracy,
            "config": {
                "max_depth": self.max_depth,
                "n_estimators": self.n_estimators,
                "min_samples_split": self.min_samples_split,
            },
        }

        Path(path).parent.mkdir(parents=True, exist_ok=True)
        with open(path, "wb") as f:
            pickle.dump(model_data, f)

        logger.info(f"Model saved to {path}")

    def load(self, path: str) -> bool:
        """Load model from file.

        Args:
            path: Path to saved model

        Returns:
            True if loaded successfully
        """
        try:
            with open(path, "rb") as f:
                model_data = pickle.load(f)

            self._model = model_data["model"]
            self._feature_names = model_data["feature_names"]
            self._feature_importance = model_data["feature_importance"]
            self._trained_at = model_data["trained_at"]
            self._training_samples = model_data["training_samples"]
            self._accuracy = model_data.get("accuracy")

            logger.info(f"Model loaded from {path} (trained {self._trained_at})")
            return True

        except Exception as e:
            logger.error(f"Failed to load model: {e}")
            return False

    def get_model_info(self) -> dict:
        """Get model metadata."""
        return {
            "is_trained": self.is_trained,
            "trained_at": self._trained_at.isoformat() if self._trained_at else None,
            "training_samples": self._training_samples,
            "accuracy": self._accuracy,
            "n_features": len(self._feature_names),
            "top_features": self.get_top_features(5),
            "config": {
                "max_depth": self.max_depth,
                "n_estimators": self.n_estimators,
                "min_samples_split": self.min_samples_split,
            },
        }
