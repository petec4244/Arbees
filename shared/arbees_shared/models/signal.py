"""Trading signal Pydantic v2 models."""

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field, computed_field

from arbees_shared.models.game import Sport
from arbees_shared.models.market import Platform


class SignalType(str, Enum):
    """Types of trading signals."""
    # Arbitrage signals
    CROSS_MARKET_ARB = "cross_market_arb"
    CROSS_MARKET_ARB_NO = "cross_market_arb_no"

    # Model edge signals
    MODEL_EDGE_YES = "model_edge_yes"
    MODEL_EDGE_NO = "model_edge_no"

    # Live game signals
    WIN_PROB_SHIFT = "win_prob_shift"
    SCORING_PLAY = "scoring_play"
    TURNOVER = "turnover"
    MOMENTUM_SHIFT = "momentum_shift"

    # Mean reversion signals
    MEAN_REVERSION = "mean_reversion"
    OVERREACTION = "overreaction"

    # Market signals
    LAGGING_MARKET = "lagging_market"
    LIQUIDITY_OPPORTUNITY = "liquidity_opportunity"


class SignalDirection(str, Enum):
    """Direction of trading signal."""
    BUY = "buy"
    SELL = "sell"
    HOLD = "hold"


class SignalConfidence(str, Enum):
    """Confidence level of signal."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    VERY_HIGH = "very_high"


class ImpactAnalysis(BaseModel):
    """Analysis of a play's impact on win probability and market prices."""
    model_config = ConfigDict(frozen=True)

    play_id: str
    game_id: str
    sport: Sport

    # Win probability changes
    home_win_prob_before: float = Field(ge=0.0, le=1.0)
    home_win_prob_after: float = Field(ge=0.0, le=1.0)

    # Market prices at time of play
    market_price_before: Optional[float] = None
    market_price_after: Optional[float] = None

    # Timing
    play_timestamp: datetime
    analysis_timestamp: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def prob_change(self) -> float:
        """Change in home win probability."""
        return self.home_win_prob_after - self.home_win_prob_before

    @computed_field
    @property
    def prob_change_pct(self) -> float:
        """Change in home win probability as percentage."""
        return self.prob_change * 100

    @computed_field
    @property
    def is_significant(self) -> bool:
        """Whether the change is significant (> 2%)."""
        return abs(self.prob_change) > 0.02

    @computed_field
    @property
    def market_edge(self) -> Optional[float]:
        """Edge between model and market (if market price available)."""
        if self.market_price_after is None:
            return None
        return (self.home_win_prob_after - self.market_price_after) * 100

    @computed_field
    @property
    def market_direction(self) -> Optional[SignalDirection]:
        """Recommended direction based on market edge."""
        edge = self.market_edge
        if edge is None:
            return None
        if edge > 2.0:
            return SignalDirection.BUY
        if edge < -2.0:
            return SignalDirection.SELL
        return SignalDirection.HOLD


class TradingSignal(BaseModel):
    """A complete trading signal with execution details."""
    model_config = ConfigDict(frozen=True)

    signal_id: str = Field(default_factory=lambda: "")
    signal_type: SignalType
    game_id: str
    sport: Sport
    team: str
    direction: SignalDirection

    # Probabilities
    model_prob: float = Field(ge=0.0, le=1.0)
    market_prob: Optional[float] = Field(default=None, ge=0.0, le=1.0)

    # Edge calculation
    edge_pct: float
    confidence: float = Field(ge=0.0, le=1.0)

    # Execution details
    platform_buy: Optional[Platform] = None
    platform_sell: Optional[Platform] = None
    buy_price: Optional[float] = None
    sell_price: Optional[float] = None
    liquidity_available: float = 0.0

    # Metadata
    reason: str
    created_at: datetime = Field(default_factory=datetime.utcnow)
    expires_at: Optional[datetime] = None
    play_id: Optional[str] = None

    @computed_field
    @property
    def is_risk_free(self) -> bool:
        """Whether this is a risk-free arbitrage opportunity."""
        return self.signal_type in (SignalType.CROSS_MARKET_ARB, SignalType.CROSS_MARKET_ARB_NO)

    @computed_field
    @property
    def confidence_level(self) -> SignalConfidence:
        """Categorical confidence level."""
        if self.confidence >= 0.8:
            return SignalConfidence.VERY_HIGH
        if self.confidence >= 0.6:
            return SignalConfidence.HIGH
        if self.confidence >= 0.4:
            return SignalConfidence.MEDIUM
        return SignalConfidence.LOW

    @computed_field
    @property
    def kelly_fraction(self) -> float:
        """Kelly criterion optimal bet fraction."""
        if self.edge_pct <= 0 or self.market_prob is None:
            return 0.0
        # Kelly = (p * b - q) / b where p=prob, q=1-p, b=odds-1
        p = self.model_prob
        q = 1.0 - p
        b = (1.0 / self.market_prob) - 1.0 if self.market_prob > 0 else 0
        if b <= 0:
            return 0.0
        return max(0.0, (p * b - q) / b)

    @computed_field
    @property
    def recommended_size_pct(self) -> float:
        """Recommended position size as percentage of bankroll (fractional Kelly)."""
        # Use quarter Kelly for safety
        return self.kelly_fraction * 0.25 * 100

    def is_expired(self, now: Optional[datetime] = None) -> bool:
        """Check if signal has expired."""
        if self.expires_at is None:
            return False
        if now is None:
            now = datetime.utcnow()
        return now > self.expires_at


class MeanReversionSignal(BaseModel):
    """Signal for mean reversion trading opportunities."""
    model_config = ConfigDict(frozen=True)

    game_id: str
    sport: Sport
    platform: Platform
    market_id: str

    # Price levels
    current_price: float = Field(ge=0.0, le=1.0)
    fair_value: float = Field(ge=0.0, le=1.0)
    recent_high: float = Field(ge=0.0, le=1.0)
    recent_low: float = Field(ge=0.0, le=1.0)

    # Analysis
    deviation_pct: float
    z_score: float
    direction: SignalDirection
    reason: str

    created_at: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def is_overbought(self) -> bool:
        """Whether the market appears overbought."""
        return self.z_score > 2.0

    @computed_field
    @property
    def is_oversold(self) -> bool:
        """Whether the market appears oversold."""
        return self.z_score < -2.0

    @computed_field
    @property
    def expected_move(self) -> float:
        """Expected price move back to fair value."""
        return self.fair_value - self.current_price


class ArbitrageOpportunity(BaseModel):
    """Complete arbitrage opportunity with execution details."""
    model_config = ConfigDict(frozen=True)

    opportunity_id: str = ""
    opportunity_type: str
    event_id: str
    sport: Sport
    market_title: str

    # Platforms
    platform_buy: Platform
    platform_sell: Platform

    # Prices
    buy_price: float = Field(ge=0.0, le=1.0)
    sell_price: float = Field(ge=0.0, le=1.0)
    edge_pct: float
    implied_profit: float

    # Liquidity
    liquidity_buy: float = Field(ge=0.0)
    liquidity_sell: float = Field(ge=0.0)

    # Metadata
    is_risk_free: bool = True
    status: str = "active"
    description: str = ""
    model_probability: Optional[float] = None
    created_at: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def max_size(self) -> float:
        """Maximum tradeable size based on liquidity."""
        return min(self.liquidity_buy, self.liquidity_sell)

    @computed_field
    @property
    def expected_profit(self) -> float:
        """Expected profit at max size."""
        return self.max_size * (self.sell_price - self.buy_price)

    def profit_for_size(self, size: float) -> float:
        """Calculate expected profit for given position size."""
        actual_size = min(size, self.max_size)
        return actual_size * (self.sell_price - self.buy_price)
