"""Execution-related Pydantic models for the split position management pipeline.

Message contracts for:
- SignalProcessor -> ExecutionService: ExecutionRequest
- ExecutionService -> PositionTracker: ExecutionResult
- PositionTracker -> UI/Monitoring: PositionUpdate
"""

from datetime import datetime
from enum import Enum
from typing import Optional
from uuid import uuid4

from pydantic import BaseModel, ConfigDict, Field

from arbees_shared.models.game import Sport
from arbees_shared.models.market import Platform


class ExecutionSide(str, Enum):
    """Order side (YES or NO contract)."""
    YES = "yes"
    NO = "no"


class ExecutionStatus(str, Enum):
    """Status of an execution request."""
    PENDING = "pending"
    ACCEPTED = "accepted"
    REJECTED = "rejected"
    FILLED = "filled"
    PARTIAL = "partial"
    CANCELLED = "cancelled"
    FAILED = "failed"


class PositionState(str, Enum):
    """State of a tracked position."""
    OPEN = "open"
    CLOSING = "closing"
    CLOSED = "closed"
    SETTLED = "settled"


class ExecutionRequest(BaseModel):
    """
    Request to execute a trade, sent from SignalProcessor to ExecutionService.
    
    Includes all information needed for order placement and later reconciliation.
    """
    model_config = ConfigDict(frozen=True)

    # Idempotency
    request_id: str = Field(default_factory=lambda: str(uuid4()))
    idempotency_key: str  # Unique key for dedupe (e.g., f"{signal_id}_{game_id}_{team}")

    # Game context
    game_id: str
    sport: Sport
    
    # Market targeting
    platform: Platform
    market_id: str
    contract_team: Optional[str] = None  # Which team's YES contract

    # Order details
    side: ExecutionSide  # YES or NO
    limit_price: float = Field(ge=0.0, le=1.0)
    size: float = Field(gt=0.0)  # Dollar amount or contract count

    # Signal metadata (for tracking/reporting)
    signal_id: str
    signal_type: str
    edge_pct: float
    model_prob: float
    market_prob: Optional[float] = None
    reason: str

    # Timestamps
    created_at: datetime = Field(default_factory=datetime.utcnow)
    expires_at: Optional[datetime] = None


class ExecutionResult(BaseModel):
    """
    Result of an execution attempt, sent from ExecutionService to PositionTracker.
    """
    model_config = ConfigDict(frozen=True)

    # Linkage
    request_id: str
    idempotency_key: str

    # Status
    status: ExecutionStatus
    rejection_reason: Optional[str] = None

    # Fill details (if filled)
    order_id: Optional[str] = None
    filled_qty: float = 0.0
    avg_price: float = 0.0
    fees: float = 0.0

    # Platform response
    platform: Platform
    market_id: str
    contract_team: Optional[str] = None

    # Game context (pass through for tracking)
    game_id: str
    sport: Sport
    signal_id: str
    signal_type: str
    edge_pct: float
    
    # Side
    side: ExecutionSide

    # Timestamps
    requested_at: datetime
    executed_at: datetime = Field(default_factory=datetime.utcnow)
    latency_ms: float = 0.0


class PositionUpdate(BaseModel):
    """
    Position state update, sent from PositionTracker to UI/monitoring.
    """
    model_config = ConfigDict(frozen=True)

    # Position identity
    position_id: str
    trade_id: str

    # State
    state: PositionState
    
    # Game context
    game_id: str
    sport: Sport
    platform: Platform
    market_id: str
    contract_team: Optional[str] = None

    # Position details
    side: ExecutionSide
    entry_price: float
    current_price: Optional[float] = None
    size: float

    # P&L
    unrealized_pnl: float = 0.0
    realized_pnl: float = 0.0
    fees_paid: float = 0.0

    # Exit info (if closed)
    exit_price: Optional[float] = None
    exit_reason: Optional[str] = None

    # Risk state
    stop_loss_price: Optional[float] = None
    take_profit_price: Optional[float] = None

    # Timestamps
    opened_at: datetime
    updated_at: datetime = Field(default_factory=datetime.utcnow)
    closed_at: Optional[datetime] = None


class RiskCheckResult(BaseModel):
    """Result of a risk evaluation check."""
    model_config = ConfigDict(frozen=True)

    approved: bool
    rejection_reason: Optional[str] = None
    rejection_details: Optional[str] = None

    # Current exposures
    daily_pnl: float = 0.0
    game_exposure: float = 0.0
    sport_exposure: float = 0.0
    
    # Limits
    max_daily_loss: float = 0.0
    max_game_exposure: float = 0.0
    max_sport_exposure: float = 0.0
