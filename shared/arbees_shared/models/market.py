"""Market-related Pydantic v2 models for prediction markets."""

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field, computed_field


class Platform(str, Enum):
    """Prediction market platforms."""
    KALSHI = "kalshi"
    POLYMARKET = "polymarket"
    SPORTSBOOK = "sportsbook"
    PAPER = "paper"


class MarketStatus(str, Enum):
    """Market status values."""
    OPEN = "open"
    CLOSED = "closed"
    SUSPENDED = "suspended"
    SETTLED = "settled"


class OrderBookLevel(BaseModel):
    """A single level in an order book."""
    model_config = ConfigDict(frozen=True)

    price: float = Field(ge=0.0, le=1.0)
    quantity: float = Field(ge=0.0)

    @computed_field
    @property
    def notional(self) -> float:
        """Dollar value at this level."""
        return self.price * self.quantity


class OrderBook(BaseModel):
    """Order book with bid/ask levels."""
    model_config = ConfigDict(frozen=True)

    market_id: str
    platform: Platform
    yes_bids: list[OrderBookLevel] = Field(default_factory=list)
    yes_asks: list[OrderBookLevel] = Field(default_factory=list)
    timestamp: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def best_yes_bid(self) -> Optional[float]:
        """Best (highest) yes bid price."""
        if not self.yes_bids:
            return None
        return max(level.price for level in self.yes_bids)

    @computed_field
    @property
    def best_yes_ask(self) -> Optional[float]:
        """Best (lowest) yes ask price."""
        if not self.yes_asks:
            return None
        return min(level.price for level in self.yes_asks)

    @computed_field
    @property
    def spread(self) -> Optional[float]:
        """Spread in percentage points."""
        bid = self.best_yes_bid
        ask = self.best_yes_ask
        if bid is None or ask is None:
            return None
        return (ask - bid) * 100

    @computed_field
    @property
    def mid_price(self) -> Optional[float]:
        """Mid price between best bid and ask."""
        bid = self.best_yes_bid
        ask = self.best_yes_ask
        if bid is None or ask is None:
            return None
        return (bid + ask) / 2

    @computed_field
    @property
    def total_bid_liquidity(self) -> float:
        """Total liquidity on bid side."""
        return sum(level.notional for level in self.yes_bids)

    @computed_field
    @property
    def total_ask_liquidity(self) -> float:
        """Total liquidity on ask side."""
        return sum(level.notional for level in self.yes_asks)


class MarketPrice(BaseModel):
    """Snapshot of market prices at a point in time."""
    model_config = ConfigDict(frozen=True)

    market_id: str
    platform: Platform
    game_id: Optional[str] = None
    market_title: str = ""
    
    # Contract team: which team this YES contract is for
    # For Polymarket moneyline: "Boston Celtics" means YES = Celtics win
    # For Kalshi: typically the home team for KXMLB-* tickers
    # None means unknown/not applicable (e.g., O/U markets)
    contract_team: Optional[str] = None

    # Prices (0.0 to 1.0)
    yes_bid: float = Field(ge=0.0, le=1.0)
    yes_ask: float = Field(ge=0.0, le=1.0)
    
    # Best-level depth (number of contracts at best bid/ask)
    # 0.0 means depth unknown/unavailable
    yes_bid_size: float = Field(ge=0.0, default=0.0)
    yes_ask_size: float = Field(ge=0.0, default=0.0)

    # Market metrics
    volume: float = Field(ge=0.0, default=0.0)
    open_interest: float = Field(ge=0.0, default=0.0)
    liquidity: float = Field(ge=0.0, default=0.0)

    # Metadata
    status: MarketStatus = MarketStatus.OPEN
    timestamp: datetime = Field(default_factory=datetime.utcnow)
    last_trade_price: Optional[float] = None

    @computed_field
    @property
    def no_bid(self) -> float:
        """NO bid price (1 - yes_ask)."""
        return 1.0 - self.yes_ask

    @computed_field
    @property
    def no_ask(self) -> float:
        """NO ask price (1 - yes_bid)."""
        return 1.0 - self.yes_bid

    @computed_field
    @property
    def mid_price(self) -> float:
        """Mid price between yes bid and ask."""
        return (self.yes_bid + self.yes_ask) / 2

    @computed_field
    @property
    def spread(self) -> float:
        """Spread in percentage points."""
        return (self.yes_ask - self.yes_bid) * 100

    @computed_field
    @property
    def implied_probability(self) -> float:
        """Implied probability from mid price."""
        return self.mid_price

    @computed_field
    @property
    def timestamp_ms(self) -> int:
        """Timestamp in milliseconds."""
        return int(self.timestamp.timestamp() * 1000)

    def has_arbitrage_with(self, other: "MarketPrice") -> bool:
        """Check if arbitrage exists between this and another market."""
        # Buy YES here, sell YES there
        if self.yes_ask < other.yes_bid:
            return True
        # Buy YES there, sell YES here
        if other.yes_ask < self.yes_bid:
            return True
        return False

    def arbitrage_edge_with(self, other: "MarketPrice") -> float:
        """Calculate arbitrage edge in percentage points."""
        edge1 = (other.yes_bid - self.yes_ask) * 100
        edge2 = (self.yes_bid - other.yes_ask) * 100
        return max(edge1, edge2, 0.0)


class MarketMapping(BaseModel):
    """Maps a game to its corresponding prediction market."""
    model_config = ConfigDict(frozen=True)

    game_id: str
    platform: Platform
    market_id: str
    market_title: str
    market_type: str = "moneyline"  # moneyline, spread, total, player_prop
    team: Optional[str] = None
    line: Optional[float] = None
    created_at: datetime = Field(default_factory=datetime.utcnow)

    @computed_field
    @property
    def mapping_key(self) -> str:
        """Unique key for this mapping."""
        return f"{self.game_id}:{self.platform.value}:{self.market_type}"


class LatencyMetrics(BaseModel):
    """Track latency between data source updates and market reactions."""
    model_config = ConfigDict(frozen=True)

    game_id: str
    play_id: str
    play_timestamp: datetime
    espn_detected_at: datetime
    market_reacted_at: Optional[datetime] = None
    signal_generated_at: Optional[datetime] = None

    @computed_field
    @property
    def espn_latency_ms(self) -> int:
        """Milliseconds from play to ESPN detection."""
        delta = self.espn_detected_at - self.play_timestamp
        return int(delta.total_seconds() * 1000)

    @computed_field
    @property
    def market_latency_ms(self) -> Optional[int]:
        """Milliseconds from play to market reaction."""
        if self.market_reacted_at is None:
            return None
        delta = self.market_reacted_at - self.play_timestamp
        return int(delta.total_seconds() * 1000)

    @computed_field
    @property
    def total_latency_ms(self) -> Optional[int]:
        """Total milliseconds from play to signal generation."""
        if self.signal_generated_at is None:
            return None
        delta = self.signal_generated_at - self.play_timestamp
        return int(delta.total_seconds() * 1000)
