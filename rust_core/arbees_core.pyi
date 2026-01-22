# Type stubs for arbees_core - Rust-based arbitrage detection and trading
# Auto-generated for PyO3 bindings

from typing import Optional, List, Tuple, Dict, Any

# ============================================================================
# Enums
# ============================================================================

class Sport:
    NFL: Sport
    NBA: Sport
    NHL: Sport
    MLB: Sport
    NCAAF: Sport
    NCAAB: Sport
    MLS: Sport
    EPL: Sport
    LaLiga: Sport
    Bundesliga: Sport
    SerieA: Sport
    Ligue1: Sport
    UFC: Sport

class Platform:
    Kalshi: Platform
    Polymarket: Platform
    Paper: Platform
    Sportsbook: Platform

# ============================================================================
# Original Types
# ============================================================================

class GameState:
    sport: Sport
    home_score: int
    away_score: int
    period: int
    time_remaining_secs: int
    possession_home: bool

    def __init__(
        self,
        sport: Sport,
        home_score: int,
        away_score: int,
        period: int,
        time_remaining_secs: int,
        possession_home: bool,
    ) -> None: ...

class MarketPrice:
    platform: Platform
    market_id: str
    yes_bid: float
    yes_ask: float
    liquidity: float
    volume_24h: float
    timestamp_ms: int

    def __init__(
        self,
        platform: Platform,
        market_id: str,
        yes_bid: float,
        yes_ask: float,
        liquidity: float,
        volume_24h: float,
        timestamp_ms: int,
    ) -> None: ...

    def mid_price(self) -> float: ...
    def spread(self) -> float: ...
    def no_ask(self) -> float: ...

class ArbitrageOpportunity:
    opportunity_type: str
    platform_buy: Platform
    platform_sell: Platform
    event_id: str
    sport: Sport
    market_title: str
    edge_pct: float
    buy_price: float
    sell_price: float
    buy_liquidity: float
    sell_liquidity: float
    is_true_arb: bool
    description: str
    model_probability: Optional[float]

class TradingSignal:
    signal_type: str
    game_id: str
    sport: Sport
    team: str
    direction: str
    model_probability: float
    market_probability: float
    confidence: float
    reason: str
    timestamp_ms: int

# ============================================================================
# AtomicOrderbook (terauss integration)
# ============================================================================

class AtomicOrderbook:
    """Lock-free atomic orderbook using packed u64."""

    def __init__(self) -> None: ...
    def load(self) -> Tuple[int, int, int, int]:
        """Returns (yes_ask, no_ask, yes_size, no_size)."""
        ...
    def store(self, yes_ask: int, no_ask: int, yes_size: int, no_size: int) -> None: ...
    def update_yes(self, yes_ask: int, yes_size: int) -> None: ...
    def update_no(self, no_ask: int, no_size: int) -> None: ...

class GlobalState:
    """Global state container for up to 1024 market pairs."""

    def __init__(self) -> None: ...
    def add_pair(
        self,
        ticker: str,
        kalshi_ticker: str,
        poly_slug: str,
        league: str,
    ) -> Optional[int]:
        """Add a market pair, returns market_id or None if full."""
        ...
    def get_by_id(self, market_id: int) -> Optional[Dict[str, Any]]: ...
    def get_by_ticker(self, ticker: str) -> Optional[int]: ...
    def update_kalshi(
        self,
        market_id: int,
        yes_ask: int,
        no_ask: int,
        yes_size: int,
        no_size: int,
    ) -> None: ...
    def update_poly(
        self,
        market_id: int,
        yes_ask: int,
        no_ask: int,
        yes_size: int,
        no_size: int,
    ) -> None: ...
    def market_count(self) -> int: ...

def py_kalshi_fee_cents(price: int) -> int:
    """Calculate Kalshi fee in cents for a given price (0-100)."""
    ...

# ============================================================================
# SIMD Arbitrage Detection (terauss integration)
# ============================================================================

def simd_check_arbs(
    kalshi_yes: int,
    kalshi_no: int,
    poly_yes: int,
    poly_no: int,
    threshold_cents: int,
) -> int:
    """
    Check for arbitrage opportunities using SIMD.

    Returns a bitmask:
    - bit 0: PolyYes + KalshiNo arb
    - bit 1: KalshiYes + PolyNo arb
    - bit 2: PolyOnly arb
    - bit 3: KalshiOnly arb
    """
    ...

def simd_batch_scan(
    markets: List[Tuple[int, int, int, int]],
    threshold_cents: int,
) -> List[Tuple[int, int]]:
    """
    Batch scan markets for arbitrage.

    Args:
        markets: List of (kalshi_yes, kalshi_no, poly_yes, poly_no) tuples
        threshold_cents: Maximum total cost in cents (usually 100)

    Returns:
        List of (market_index, arb_mask) for markets with opportunities
    """
    ...

def simd_calculate_profit(
    kalshi_yes: int,
    kalshi_no: int,
    poly_yes: int,
    poly_no: int,
    arb_type: int,
) -> int:
    """Calculate profit in cents for a given arb type (0-3)."""
    ...

def simd_decode_mask(mask: int) -> List[str]:
    """Decode an arb bitmask into human-readable arb types."""
    ...

# ============================================================================
# Circuit Breaker (terauss integration)
# ============================================================================

class CircuitBreakerConfig:
    """Configuration for the circuit breaker."""

    max_position_per_market: int
    max_total_position: int
    max_daily_loss: float
    max_consecutive_errors: int
    cooldown_secs: int
    enabled: bool

    def __init__(
        self,
        max_position_per_market: int = 50000,
        max_total_position: int = 100000,
        max_daily_loss: float = 500.0,
        max_consecutive_errors: int = 5,
        cooldown_secs: int = 300,
        enabled: bool = True,
    ) -> None: ...

class CircuitBreaker:
    """Risk management circuit breaker."""

    def __init__(self, config: CircuitBreakerConfig) -> None: ...
    def is_trading_allowed(self) -> bool: ...
    def can_execute(self, market_id: str, contracts: int) -> None:
        """Raises RuntimeError if execution would violate limits."""
        ...
    def record_success(
        self,
        market_id: str,
        kalshi_contracts: int,
        poly_contracts: int,
        pnl: float,
    ) -> None: ...
    def record_error(self) -> None: ...
    def record_pnl(self, pnl: float) -> None: ...
    def trip(self, reason: str) -> None: ...
    def reset(self) -> None: ...
    def reset_daily_pnl(self) -> None: ...
    def status(self) -> Dict[str, Any]: ...

# ============================================================================
# Execution Tracker (terauss integration)
# ============================================================================

class ExecutionTracker:
    """Lock-free execution tracker for deduplication."""

    def __init__(self) -> None: ...
    def try_acquire(self, market_id: int) -> bool:
        """Try to acquire execution slot. Returns True if acquired."""
        ...
    def release(self, market_id: int) -> None: ...
    def is_in_flight(self, market_id: int) -> bool: ...
    def in_flight_count(self) -> int: ...
    def now_ns(self) -> int:
        """Get nanoseconds since tracker creation."""
        ...
    def reset(self) -> None: ...

class FastExecutionRequest:
    """Fast execution request with profit calculation."""

    market_id: int
    yes_price: int
    no_price: int
    yes_size: int
    no_size: int
    arb_type: int
    detected_ns: int

    def __init__(
        self,
        market_id: int,
        yes_price: int,
        no_price: int,
        yes_size: int,
        no_size: int,
        arb_type: int,
        detected_ns: int,
    ) -> None: ...

    def profit_cents(self) -> int: ...
    def estimated_fee_cents(self) -> int: ...
    def max_contracts(self) -> int: ...

# ============================================================================
# Position Tracker (terauss integration)
# ============================================================================

class ArbPosition:
    """Arbitrage position across platforms."""

    market_id: str
    description: str
    status: str
    total_contracts: float
    total_cost: float
    guaranteed_profit: float
    matched_contracts: float
    unmatched_exposure: float
    realized_pnl: Optional[float]
    total_fees: float

    def __init__(self, market_id: str, description: str) -> None: ...
    def to_dict(self) -> Dict[str, Any]: ...

class PositionTracker:
    """Track positions and P&L across all markets."""

    def __init__(self) -> None: ...

    @staticmethod
    def load(path: Optional[str] = None) -> "PositionTracker": ...

    def save(self, path: Optional[str] = None) -> None: ...
    def record_fill(
        self,
        market_id: str,
        description: str,
        platform: str,
        side: str,
        contracts: float,
        price: float,
        fees: float,
    ) -> None: ...
    def get_position(self, market_id: str) -> Optional[ArbPosition]: ...
    def resolve_position(self, market_id: str, yes_won: bool) -> Optional[float]: ...
    def summary(self) -> Dict[str, Any]: ...
    def daily_pnl(self) -> float: ...
    def all_time_pnl(self) -> float: ...
    def open_positions(self) -> List[ArbPosition]: ...
    def position_count(self) -> int: ...

# ============================================================================
# Team Cache (terauss integration)
# ============================================================================

class TeamCache:
    """Bidirectional team code mapping (Polymarket <-> Kalshi)."""

    def __init__(self) -> None: ...

    @staticmethod
    def load(path: Optional[str] = None) -> "TeamCache": ...

    def save(self, path: Optional[str] = None) -> None: ...
    def poly_to_kalshi(self, league: str, poly_code: str) -> Optional[str]: ...
    def kalshi_to_poly(self, league: str, kalshi_code: str) -> Optional[str]: ...
    def insert(self, league: str, poly_code: str, kalshi_code: str) -> None: ...
    def __len__(self) -> int: ...
    def leagues(self) -> List[str]: ...
    def get_league_mappings(self, league: str) -> List[Tuple[str, str]]: ...

# ============================================================================
# League Config (terauss integration)
# ============================================================================

class LeagueConfig:
    """Configuration for a supported league."""

    league_code: str
    poly_prefix: str
    kalshi_series_game: str
    kalshi_series_spread: Optional[str]
    kalshi_series_total: Optional[str]
    kalshi_series_btts: Optional[str]

def get_league_configs() -> List[LeagueConfig]:
    """Get all league configurations."""
    ...

def get_league_config(league: str) -> Optional[LeagueConfig]:
    """Get configuration for a specific league (case-insensitive)."""
    ...

def get_league_codes() -> List[str]:
    """Get list of all supported league codes."""
    ...

# ============================================================================
# Original Functions
# ============================================================================

def find_cross_market_arbitrage(
    market_a: MarketPrice,
    market_b: MarketPrice,
    event_id: str,
    sport: Sport,
    market_title: str,
) -> List[ArbitrageOpportunity]: ...

def find_same_platform_arbitrage(
    market: MarketPrice,
    event_id: str,
    sport: Sport,
    market_title: str,
) -> Optional[ArbitrageOpportunity]: ...

def find_model_edges(
    market: MarketPrice,
    model_prob: float,
    event_id: str,
    sport: Sport,
    market_title: str,
    min_edge_pct: float,
) -> List[ArbitrageOpportunity]: ...

def detect_lagging_market(
    market: MarketPrice,
    current_time_ms: int,
    stale_threshold_ms: int,
    event_id: str,
    sport: Sport,
    market_title: str,
) -> Optional[ArbitrageOpportunity]: ...

def calculate_win_probability(state: GameState, for_home: bool) -> float: ...

def batch_calculate_win_probs(states: List[GameState], for_home: bool) -> List[float]: ...

def calculate_win_prob_delta(
    old_state: GameState,
    new_state: GameState,
    for_home: bool,
) -> float: ...

def expected_points(yard_line: int, down: int, yards_to_go: int) -> float: ...

def generate_signal_from_prob_change(
    game_id: str,
    sport: Sport,
    team: str,
    old_prob: float,
    new_prob: float,
    market_prob: float,
    min_edge_pct: float,
    timestamp_ms: int,
) -> Optional[TradingSignal]: ...

def batch_scan_arbitrage(
    markets: Dict[str, List[MarketPrice]],
    sport: Sport,
) -> List[ArbitrageOpportunity]: ...
