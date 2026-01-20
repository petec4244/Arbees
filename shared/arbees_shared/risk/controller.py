"""
Risk Management Controller for Arbees trading system.

Enforces:
- Max daily loss limits
- Per-game and per-sport exposure limits
- Position correlation checks (don't bet against yourself)
- Latency circuit breaker
"""

import asyncio
import logging
from datetime import datetime, date, timedelta
from enum import Enum
from typing import Optional
from dataclasses import dataclass, field

from asyncpg import Pool

logger = logging.getLogger(__name__)


class RiskRejection(str, Enum):
    """Reasons a trade can be rejected by risk management."""
    DAILY_LOSS_LIMIT = "daily_loss_limit_reached"
    GAME_EXPOSURE_LIMIT = "game_exposure_limit_reached"
    SPORT_EXPOSURE_LIMIT = "sport_exposure_limit_reached"
    CORRELATED_POSITION = "correlated_position_detected"
    CIRCUIT_BREAKER_OPEN = "circuit_breaker_open"
    LATENCY_TOO_HIGH = "latency_too_high"


@dataclass
class RiskDecision:
    """Result of risk evaluation."""
    approved: bool
    rejection_reason: Optional[RiskRejection] = None
    rejection_details: str = ""
    # Metrics at time of decision
    daily_pnl: float = 0.0
    game_exposure: float = 0.0
    sport_exposure: float = 0.0
    current_latency_ms: float = 0.0

    def __str__(self) -> str:
        if self.approved:
            return "APPROVED"
        return f"REJECTED: {self.rejection_reason.value} - {self.rejection_details}"


@dataclass
class PositionInfo:
    """Information about an open position for correlation checks."""
    game_id: str
    sport: str
    team: str
    side: str  # 'buy' or 'sell'
    size: float
    entry_price: float
    home_team: str = ""
    away_team: str = ""


@dataclass
class RiskMetrics:
    """Current risk metrics for monitoring."""
    daily_pnl: float = 0.0
    open_positions_count: int = 0
    total_exposure: float = 0.0
    exposure_by_game: dict = field(default_factory=dict)
    exposure_by_sport: dict = field(default_factory=dict)
    circuit_breaker_open: bool = False
    circuit_breaker_reason: str = ""
    avg_latency_ms: float = 0.0
    last_updated: datetime = field(default_factory=datetime.utcnow)


class RiskController:
    """
    Risk management controller that gates all trade execution.

    Sits between signal generation and trade execution to enforce:
    - Max daily loss ($X configurable)
    - Per-game exposure limits
    - Per-sport exposure limits
    - Position correlation checks
    - Latency circuit breaker
    """

    def __init__(
        self,
        pool: Pool,
        # Daily loss limit
        max_daily_loss: float = 100.0,
        # Exposure limits
        max_game_exposure: float = 50.0,
        max_sport_exposure: float = 200.0,
        # Latency settings
        max_latency_ms: float = 5000.0,  # 5 second max latency
        latency_window_size: int = 10,  # Rolling window for average
        circuit_breaker_threshold_ms: float = 10000.0,  # 10 second triggers breaker
        circuit_breaker_cooldown_seconds: float = 60.0,  # 1 minute cooldown
    ):
        """
        Initialize the Risk Controller.

        Args:
            pool: Database connection pool
            max_daily_loss: Maximum allowed loss per day before halting
            max_game_exposure: Maximum $ exposure to any single game
            max_sport_exposure: Maximum $ exposure to any single sport
            max_latency_ms: Maximum acceptable signal latency
            latency_window_size: Number of samples for rolling average
            circuit_breaker_threshold_ms: Latency that triggers circuit breaker
            circuit_breaker_cooldown_seconds: Time to wait before re-enabling
        """
        self._pool = pool

        # Limits
        self.max_daily_loss = max_daily_loss
        self.max_game_exposure = max_game_exposure
        self.max_sport_exposure = max_sport_exposure

        # Latency tracking
        self.max_latency_ms = max_latency_ms
        self.latency_window_size = latency_window_size
        self.circuit_breaker_threshold_ms = circuit_breaker_threshold_ms
        self.circuit_breaker_cooldown_seconds = circuit_breaker_cooldown_seconds

        # State
        self._latency_samples: list[float] = []
        self._circuit_breaker_open = False
        self._circuit_breaker_opened_at: Optional[datetime] = None
        self._circuit_breaker_reason = ""

        # Cached metrics
        self._metrics_cache: Optional[RiskMetrics] = None
        self._metrics_cache_time: Optional[datetime] = None
        self._metrics_cache_ttl = timedelta(seconds=5)  # Refresh every 5 seconds

        logger.info(
            f"RiskController initialized: max_daily_loss=${max_daily_loss}, "
            f"max_game_exposure=${max_game_exposure}, max_sport_exposure=${max_sport_exposure}, "
            f"max_latency={max_latency_ms}ms"
        )

    async def evaluate_trade(
        self,
        game_id: str,
        sport: str,
        team: str,
        side: str,
        proposed_size: float,
        signal_timestamp: Optional[datetime] = None,
    ) -> RiskDecision:
        """
        Evaluate whether a proposed trade should be allowed.

        Args:
            game_id: ID of the game
            sport: Sport type (e.g., 'nfl', 'nba')
            team: Team the signal is for
            side: 'buy' or 'sell'
            proposed_size: Dollar size of proposed trade
            signal_timestamp: When the signal was generated (for latency check)

        Returns:
            RiskDecision with approval status and reason if rejected
        """
        # Calculate current latency if timestamp provided
        current_latency_ms = 0.0
        if signal_timestamp:
            latency = datetime.utcnow() - signal_timestamp
            current_latency_ms = latency.total_seconds() * 1000
            self._record_latency(current_latency_ms)

        # Check circuit breaker first (fast path)
        if self._circuit_breaker_open:
            if not self._check_circuit_breaker_reset():
                return RiskDecision(
                    approved=False,
                    rejection_reason=RiskRejection.CIRCUIT_BREAKER_OPEN,
                    rejection_details=f"Circuit breaker open: {self._circuit_breaker_reason}",
                    current_latency_ms=current_latency_ms,
                )

        # Check latency
        if current_latency_ms > self.max_latency_ms:
            logger.warning(f"Signal latency {current_latency_ms:.0f}ms exceeds max {self.max_latency_ms}ms")
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.LATENCY_TOO_HIGH,
                rejection_details=f"Signal latency {current_latency_ms:.0f}ms > max {self.max_latency_ms}ms",
                current_latency_ms=current_latency_ms,
            )

        # Check if latency triggers circuit breaker
        if current_latency_ms > self.circuit_breaker_threshold_ms:
            self._open_circuit_breaker(f"Extreme latency: {current_latency_ms:.0f}ms")
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.CIRCUIT_BREAKER_OPEN,
                rejection_details=f"Circuit breaker triggered by {current_latency_ms:.0f}ms latency",
                current_latency_ms=current_latency_ms,
            )

        # Get current metrics from database
        daily_pnl = await self._get_daily_pnl()
        game_exposure = await self._get_game_exposure(game_id)
        sport_exposure = await self._get_sport_exposure(sport)

        # Check daily loss limit
        if daily_pnl <= -self.max_daily_loss:
            logger.warning(f"Daily loss limit reached: ${daily_pnl:.2f} <= -${self.max_daily_loss}")
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.DAILY_LOSS_LIMIT,
                rejection_details=f"Daily P&L ${daily_pnl:.2f} has hit -${self.max_daily_loss} limit",
                daily_pnl=daily_pnl,
                game_exposure=game_exposure,
                sport_exposure=sport_exposure,
                current_latency_ms=current_latency_ms,
            )

        # Check game exposure limit (including proposed trade)
        if game_exposure + proposed_size > self.max_game_exposure:
            logger.warning(
                f"Game exposure limit: ${game_exposure:.2f} + ${proposed_size:.2f} "
                f"> ${self.max_game_exposure} for game {game_id}"
            )
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.GAME_EXPOSURE_LIMIT,
                rejection_details=(
                    f"Would exceed game limit: ${game_exposure:.2f} + ${proposed_size:.2f} "
                    f"> ${self.max_game_exposure}"
                ),
                daily_pnl=daily_pnl,
                game_exposure=game_exposure,
                sport_exposure=sport_exposure,
                current_latency_ms=current_latency_ms,
            )

        # Check sport exposure limit (including proposed trade)
        if sport_exposure + proposed_size > self.max_sport_exposure:
            logger.warning(
                f"Sport exposure limit: ${sport_exposure:.2f} + ${proposed_size:.2f} "
                f"> ${self.max_sport_exposure} for {sport}"
            )
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.SPORT_EXPOSURE_LIMIT,
                rejection_details=(
                    f"Would exceed {sport} limit: ${sport_exposure:.2f} + ${proposed_size:.2f} "
                    f"> ${self.max_sport_exposure}"
                ),
                daily_pnl=daily_pnl,
                game_exposure=game_exposure,
                sport_exposure=sport_exposure,
                current_latency_ms=current_latency_ms,
            )

        # Check position correlation
        correlation_check = await self._check_position_correlation(game_id, team, side)
        if not correlation_check["allowed"]:
            logger.warning(f"Correlated position detected: {correlation_check['reason']}")
            return RiskDecision(
                approved=False,
                rejection_reason=RiskRejection.CORRELATED_POSITION,
                rejection_details=correlation_check["reason"],
                daily_pnl=daily_pnl,
                game_exposure=game_exposure,
                sport_exposure=sport_exposure,
                current_latency_ms=current_latency_ms,
            )

        # All checks passed
        logger.debug(
            f"Trade approved: game={game_id}, sport={sport}, team={team}, side={side}, "
            f"size=${proposed_size:.2f}"
        )
        return RiskDecision(
            approved=True,
            daily_pnl=daily_pnl,
            game_exposure=game_exposure,
            sport_exposure=sport_exposure,
            current_latency_ms=current_latency_ms,
        )

    async def _get_daily_pnl(self) -> float:
        """Get today's realized + unrealized P&L."""
        today = date.today()

        # Get realized P&L from closed trades today
        realized_row = await self._pool.fetchrow("""
            SELECT COALESCE(SUM(pnl), 0) as realized_pnl
            FROM paper_trades
            WHERE status = 'closed'
              AND DATE(exit_time) = $1
        """, today)
        realized_pnl = float(realized_row["realized_pnl"]) if realized_row else 0.0

        # Get unrealized P&L from open positions
        # We estimate using entry_price vs current model assumption
        # In production, we'd fetch current market prices
        open_rows = await self._pool.fetch("""
            SELECT side, entry_price, size
            FROM paper_trades
            WHERE status = 'open'
        """)

        unrealized_pnl = 0.0
        for row in open_rows:
            # Conservative estimate: assume we're slightly underwater
            # Better approach would be to fetch current prices
            side = row["side"]
            entry_price = float(row["entry_price"])
            size = float(row["size"])

            # Assume 2% adverse move as conservative estimate
            if side == "buy":
                unrealized_pnl -= size * 0.02
            else:
                unrealized_pnl -= size * 0.02

        return realized_pnl + unrealized_pnl

    async def _get_game_exposure(self, game_id: str) -> float:
        """Get current exposure to a specific game."""
        row = await self._pool.fetchrow("""
            SELECT COALESCE(SUM(size), 0) as total_exposure
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
        """, game_id)
        return float(row["total_exposure"]) if row else 0.0

    async def _get_sport_exposure(self, sport: str) -> float:
        """Get current exposure to a specific sport."""
        row = await self._pool.fetchrow("""
            SELECT COALESCE(SUM(size), 0) as total_exposure
            FROM paper_trades
            WHERE sport = $1 AND status = 'open'
        """, sport)
        return float(row["total_exposure"]) if row else 0.0

    async def _check_position_correlation(
        self,
        game_id: str,
        team: str,
        new_side: str,
    ) -> dict:
        """
        Check if new position would be correlated with existing positions.

        Correlation rules:
        1. Don't BUY team A AND SELL team A in the same game (direct conflict)
        2. Don't BUY home team AND BUY away team in same game (one must lose)
        3. For same team: BUY is bullish, SELL is bearish - don't mix

        Returns:
            dict with 'allowed' (bool) and 'reason' (str)
        """
        # Get game info to know home/away teams
        game_row = await self._pool.fetchrow("""
            SELECT home_team, away_team
            FROM games
            WHERE game_id = $1
        """, game_id)

        if not game_row:
            # No game info - try to infer from game_states
            game_row = await self._pool.fetchrow("""
                SELECT home_team, away_team
                FROM game_states
                WHERE game_id = $1
                ORDER BY time DESC
                LIMIT 1
            """, game_id)

        if not game_row:
            # Can't determine teams - allow trade but log warning
            logger.warning(f"Cannot determine teams for game {game_id} - skipping correlation check")
            return {"allowed": True, "reason": ""}

        home_team = game_row["home_team"]
        away_team = game_row["away_team"]

        # Determine if new position is for home or away team
        team_lower = team.lower()
        is_home_team = (
            team_lower in home_team.lower() or
            home_team.lower() in team_lower
        )
        is_away_team = (
            team_lower in away_team.lower() or
            away_team.lower() in team_lower
        )

        # Get existing positions for this game
        existing_positions = await self._pool.fetch("""
            SELECT trade_id, side, size, market_title
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
        """, game_id)

        if not existing_positions:
            return {"allowed": True, "reason": ""}

        for pos in existing_positions:
            existing_side = pos["side"]
            market_title = pos.get("market_title", "")

            # Check for direct conflict: same game, opposite side
            # This is already handled in position_manager, but double-check
            if existing_side != new_side:
                # Opposite sides on same game - might be intentional hedge,
                # but likely a problem
                logger.info(
                    f"Potential correlation: existing {existing_side.upper()} vs new {new_side.upper()} "
                    f"on game {game_id}"
                )

                # Allow if it's explicitly closing the position
                # The position_manager handles this separately
                pass

            # Check for team-based correlation
            # If we're BUY on home team, we shouldn't also BUY on away team
            # because in a head-to-head game, one must lose
            if existing_side == new_side == "buy":
                # Both bullish - check if on opposing teams
                existing_is_home = "home" in market_title.lower() or home_team.lower() in market_title.lower()
                existing_is_away = "away" in market_title.lower() or away_team.lower() in market_title.lower()

                if (is_home_team and existing_is_away) or (is_away_team and existing_is_home):
                    return {
                        "allowed": False,
                        "reason": (
                            f"Correlated bullish positions: existing BUY on "
                            f"{'home' if existing_is_home else 'away'} team, "
                            f"new BUY on {'home' if is_home_team else 'away'} team. "
                            f"In head-to-head game, one must lose."
                        )
                    }

            # Similar check for SELL on both teams (both bearish = one must win)
            if existing_side == new_side == "sell":
                existing_is_home = "home" in market_title.lower() or home_team.lower() in market_title.lower()
                existing_is_away = "away" in market_title.lower() or away_team.lower() in market_title.lower()

                if (is_home_team and existing_is_away) or (is_away_team and existing_is_home):
                    return {
                        "allowed": False,
                        "reason": (
                            f"Correlated bearish positions: existing SELL on "
                            f"{'home' if existing_is_home else 'away'} team, "
                            f"new SELL on {'home' if is_home_team else 'away'} team. "
                            f"In head-to-head game, one must win."
                        )
                    }

        return {"allowed": True, "reason": ""}

    def _record_latency(self, latency_ms: float) -> None:
        """Record a latency sample for rolling average."""
        self._latency_samples.append(latency_ms)
        if len(self._latency_samples) > self.latency_window_size:
            self._latency_samples.pop(0)

    def _get_avg_latency(self) -> float:
        """Get rolling average latency."""
        if not self._latency_samples:
            return 0.0
        return sum(self._latency_samples) / len(self._latency_samples)

    def _open_circuit_breaker(self, reason: str) -> None:
        """Open the circuit breaker to halt all trading."""
        if not self._circuit_breaker_open:
            logger.error(f"CIRCUIT BREAKER OPENED: {reason}")
            self._circuit_breaker_open = True
            self._circuit_breaker_opened_at = datetime.utcnow()
            self._circuit_breaker_reason = reason

    def _check_circuit_breaker_reset(self) -> bool:
        """Check if circuit breaker should be reset after cooldown."""
        if not self._circuit_breaker_open:
            return True

        if self._circuit_breaker_opened_at is None:
            return True

        elapsed = (datetime.utcnow() - self._circuit_breaker_opened_at).total_seconds()
        if elapsed >= self.circuit_breaker_cooldown_seconds:
            logger.info(
                f"Circuit breaker reset after {elapsed:.0f}s cooldown "
                f"(was: {self._circuit_breaker_reason})"
            )
            self._circuit_breaker_open = False
            self._circuit_breaker_opened_at = None
            self._circuit_breaker_reason = ""
            return True

        return False

    def force_open_circuit_breaker(self, reason: str) -> None:
        """Manually open circuit breaker (for external triggers)."""
        self._open_circuit_breaker(f"Manual: {reason}")

    def force_close_circuit_breaker(self) -> None:
        """Manually close circuit breaker."""
        if self._circuit_breaker_open:
            logger.info("Circuit breaker manually closed")
            self._circuit_breaker_open = False
            self._circuit_breaker_opened_at = None
            self._circuit_breaker_reason = ""

    async def get_metrics(self, force_refresh: bool = False) -> RiskMetrics:
        """Get current risk metrics (cached for performance)."""
        now = datetime.utcnow()

        # Return cached if fresh
        if (
            not force_refresh
            and self._metrics_cache is not None
            and self._metrics_cache_time is not None
            and (now - self._metrics_cache_time) < self._metrics_cache_ttl
        ):
            return self._metrics_cache

        # Refresh metrics
        daily_pnl = await self._get_daily_pnl()

        # Get exposure by game
        game_rows = await self._pool.fetch("""
            SELECT game_id, SUM(size) as exposure
            FROM paper_trades
            WHERE status = 'open'
            GROUP BY game_id
        """)
        exposure_by_game = {row["game_id"]: float(row["exposure"]) for row in game_rows}

        # Get exposure by sport
        sport_rows = await self._pool.fetch("""
            SELECT sport, SUM(size) as exposure
            FROM paper_trades
            WHERE status = 'open'
            GROUP BY sport
        """)
        exposure_by_sport = {row["sport"]: float(row["exposure"]) for row in sport_rows}

        # Get total
        total_row = await self._pool.fetchrow("""
            SELECT COUNT(*) as count, COALESCE(SUM(size), 0) as exposure
            FROM paper_trades
            WHERE status = 'open'
        """)

        self._metrics_cache = RiskMetrics(
            daily_pnl=daily_pnl,
            open_positions_count=int(total_row["count"]) if total_row else 0,
            total_exposure=float(total_row["exposure"]) if total_row else 0.0,
            exposure_by_game=exposure_by_game,
            exposure_by_sport=exposure_by_sport,
            circuit_breaker_open=self._circuit_breaker_open,
            circuit_breaker_reason=self._circuit_breaker_reason,
            avg_latency_ms=self._get_avg_latency(),
            last_updated=now,
        )
        self._metrics_cache_time = now

        return self._metrics_cache

    async def get_status_report(self) -> str:
        """Get human-readable status report."""
        metrics = await self.get_metrics(force_refresh=True)

        lines = [
            "=" * 50,
            "RISK CONTROLLER STATUS",
            "=" * 50,
            f"Daily P&L:        ${metrics.daily_pnl:+.2f} / -${self.max_daily_loss:.2f} limit",
            f"Open Positions:   {metrics.open_positions_count}",
            f"Total Exposure:   ${metrics.total_exposure:.2f}",
            f"Avg Latency:      {metrics.avg_latency_ms:.0f}ms",
            "",
            "Exposure by Sport:",
        ]

        for sport, exposure in sorted(metrics.exposure_by_sport.items()):
            limit_pct = (exposure / self.max_sport_exposure) * 100
            lines.append(f"  {sport.upper():10} ${exposure:8.2f} ({limit_pct:.0f}% of ${self.max_sport_exposure})")

        lines.append("")
        lines.append("Exposure by Game (top 5):")

        sorted_games = sorted(metrics.exposure_by_game.items(), key=lambda x: -x[1])[:5]
        for game_id, exposure in sorted_games:
            limit_pct = (exposure / self.max_game_exposure) * 100
            lines.append(f"  {game_id[:15]:15} ${exposure:8.2f} ({limit_pct:.0f}% of ${self.max_game_exposure})")

        lines.append("")
        if metrics.circuit_breaker_open:
            lines.append(f"CIRCUIT BREAKER: OPEN - {metrics.circuit_breaker_reason}")
        else:
            lines.append("Circuit Breaker: Closed (normal operation)")

        lines.append("=" * 50)

        return "\n".join(lines)
