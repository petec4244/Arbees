"""Trade and position Pydantic v2 models."""

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field, computed_field

from arbees_shared.models.game import Sport
from arbees_shared.models.market import Platform
from arbees_shared.models.signal import SignalType


class TradeStatus(str, Enum):
    """Status of a trade."""
    PENDING = "pending"
    OPEN = "open"
    CLOSED = "closed"
    CANCELLED = "cancelled"
    EXPIRED = "expired"


class TradeSide(str, Enum):
    """Side of a trade."""
    BUY = "buy"
    SELL = "sell"


class TradeOutcome(str, Enum):
    """Outcome of a closed trade."""
    WIN = "win"
    LOSS = "loss"
    PUSH = "push"
    PENDING = "pending"


class PaperTrade(BaseModel):
    """A simulated paper trade for backtesting."""
    model_config = ConfigDict(frozen=True)

    trade_id: str
    signal_id: str
    game_id: str
    sport: Sport
    platform: Platform
    market_id: str
    market_title: str

    # Trade details
    side: TradeSide
    signal_type: SignalType
    entry_price: float = Field(ge=0.0, le=1.0)
    exit_price: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    size: float = Field(gt=0.0)

    # Risk metrics at entry
    model_prob: float = Field(ge=0.0, le=1.0)
    edge_at_entry: float
    kelly_fraction: float = Field(ge=0.0, le=1.0)

    # Execution details
    entry_time: datetime = Field(default_factory=datetime.utcnow)
    exit_time: Optional[datetime] = None
    status: TradeStatus = TradeStatus.PENDING
    outcome: TradeOutcome = TradeOutcome.PENDING

    # Fee tracking
    entry_fees: float = 0.0
    exit_fees: float = 0.0

    @computed_field
    @property
    def notional(self) -> float:
        """Dollar value of the trade."""
        return self.size

    @computed_field
    @property
    def risk_amount(self) -> float:
        """Amount at risk (max loss)."""
        if self.side == TradeSide.BUY:
            return self.size * self.entry_price
        return self.size * (1.0 - self.entry_price)

    @computed_field
    @property
    def potential_profit(self) -> float:
        """Maximum potential profit."""
        if self.side == TradeSide.BUY:
            return self.size * (1.0 - self.entry_price)
        return self.size * self.entry_price

    @computed_field
    @property
    def total_fees(self) -> float:
        """Total fees paid."""
        return self.entry_fees + self.exit_fees

    @computed_field
    @property
    def pnl(self) -> Optional[float]:
        """Profit/loss if trade is closed."""
        if self.exit_price is None or self.status != TradeStatus.CLOSED:
            return None

        if self.side == TradeSide.BUY:
            gross_pnl = self.size * (self.exit_price - self.entry_price)
        else:
            gross_pnl = self.size * (self.entry_price - self.exit_price)

        return gross_pnl - self.total_fees

    @computed_field
    @property
    def pnl_pct(self) -> Optional[float]:
        """Profit/loss as percentage of risk amount."""
        pnl = self.pnl
        if pnl is None or self.risk_amount == 0:
            return None
        return (pnl / self.risk_amount) * 100

    @computed_field
    @property
    def holding_time_seconds(self) -> Optional[int]:
        """Time trade was held in seconds."""
        if self.exit_time is None:
            return None
        delta = self.exit_time - self.entry_time
        return int(delta.total_seconds())


class Position(BaseModel):
    """Aggregated position across multiple trades."""
    model_config = ConfigDict(frozen=True)

    position_id: str
    game_id: str
    sport: Sport
    platform: Platform
    market_id: str
    market_title: str

    # Position details
    side: TradeSide
    avg_entry_price: float = Field(ge=0.0, le=1.0)
    total_size: float = Field(ge=0.0)
    realized_pnl: float = 0.0

    # Current market
    current_price: Optional[float] = None
    last_updated: datetime = Field(default_factory=datetime.utcnow)

    # Trade history
    trade_ids: list[str] = Field(default_factory=list)
    entry_count: int = 0

    @computed_field
    @property
    def notional(self) -> float:
        """Total position value."""
        return self.total_size

    @computed_field
    @property
    def unrealized_pnl(self) -> Optional[float]:
        """Current unrealized profit/loss."""
        if self.current_price is None or self.total_size == 0:
            return None

        if self.side == TradeSide.BUY:
            return self.total_size * (self.current_price - self.avg_entry_price)
        return self.total_size * (self.avg_entry_price - self.current_price)

    @computed_field
    @property
    def total_pnl(self) -> Optional[float]:
        """Total realized + unrealized PnL."""
        unrealized = self.unrealized_pnl
        if unrealized is None:
            return self.realized_pnl
        return self.realized_pnl + unrealized

    @computed_field
    @property
    def risk_amount(self) -> float:
        """Current risk in position."""
        if self.side == TradeSide.BUY:
            return self.total_size * self.avg_entry_price
        return self.total_size * (1.0 - self.avg_entry_price)

    def is_flat(self) -> bool:
        """Check if position is flat (no size)."""
        return self.total_size == 0


class PerformanceStats(BaseModel):
    """Aggregate performance statistics."""
    model_config = ConfigDict(frozen=True)

    # Period
    start_date: datetime
    end_date: datetime = Field(default_factory=datetime.utcnow)

    # Trade counts
    total_trades: int = 0
    winning_trades: int = 0
    losing_trades: int = 0
    push_trades: int = 0

    # PnL
    total_pnl: float = 0.0
    total_fees: float = 0.0
    net_pnl: float = 0.0

    # By type
    arb_trades: int = 0
    arb_pnl: float = 0.0
    model_edge_trades: int = 0
    model_edge_pnl: float = 0.0

    # Bankroll
    starting_bankroll: float
    current_bankroll: float

    @computed_field
    @property
    def win_rate(self) -> float:
        """Win rate percentage."""
        if self.total_trades == 0:
            return 0.0
        return (self.winning_trades / self.total_trades) * 100

    @computed_field
    @property
    def profit_factor(self) -> Optional[float]:
        """Ratio of gross profits to gross losses."""
        # Simplified - would need more detailed tracking
        if self.losing_trades == 0:
            return None
        avg_loss = abs(self.total_pnl) / self.losing_trades if self.total_pnl < 0 else 1
        if avg_loss == 0:
            return None
        return abs(self.total_pnl / avg_loss) if self.total_pnl > 0 else 0

    @computed_field
    @property
    def roi_pct(self) -> float:
        """Return on investment percentage."""
        if self.starting_bankroll == 0:
            return 0.0
        return ((self.current_bankroll - self.starting_bankroll) / self.starting_bankroll) * 100

    @computed_field
    @property
    def avg_trade_pnl(self) -> float:
        """Average PnL per trade."""
        if self.total_trades == 0:
            return 0.0
        return self.net_pnl / self.total_trades


class Bankroll(BaseModel):
    """Track bankroll state."""
    model_config = ConfigDict(frozen=True)

    initial_balance: float = Field(gt=0)
    current_balance: float = Field(ge=0)
    reserved_balance: float = Field(ge=0, default=0.0)  # Funds in open positions
    peak_balance: float = Field(ge=0)
    trough_balance: float = Field(ge=0)
    last_updated: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def available_balance(self) -> float:
        """Balance available for new trades."""
        return self.current_balance - self.reserved_balance

    @computed_field
    @property
    def total_pnl(self) -> float:
        """Total profit/loss from initial."""
        return self.current_balance - self.initial_balance

    @computed_field
    @property
    def total_pnl_pct(self) -> float:
        """Total PnL as percentage."""
        return (self.total_pnl / self.initial_balance) * 100

    @computed_field
    @property
    def max_drawdown(self) -> float:
        """Maximum drawdown from peak."""
        if self.peak_balance == 0:
            return 0.0
        return ((self.peak_balance - self.trough_balance) / self.peak_balance) * 100

    def can_trade(self, size: float) -> bool:
        """Check if we have enough balance for a trade."""
        return self.available_balance >= size
