"""
SignalProcessor service - Phase 1 split from PositionManager.

Responsibilities:
- Subscribe to trading signals from Redis (signals:new)
- Apply pre-trade filtering (edge threshold, probability bounds, cooldowns, duplicates)
- Check risk limits via RiskController
- Emit ExecutionRequest messages to execution:requests channel
"""

import asyncio
import logging
import os
from datetime import datetime, timedelta, timezone
from typing import Optional
from uuid import uuid4

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.game import Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.signal import TradingSignal, SignalDirection
from arbees_shared.models.execution import (
    ExecutionRequest,
    ExecutionSide,
    RiskCheckResult,
)
from arbees_shared.risk import RiskController
from arbees_shared.health.heartbeat import HeartbeatPublisher
from arbees_shared.models.health import ServiceStatus
from arbees_shared.utils.trace_logger import TraceContext
from arbees_shared.team_matching import TeamMatchingClient, TeamMatchResult

from .rule_evaluator import RuleEvaluator, RuleDecisionType

logger = logging.getLogger(__name__)


# Sport-specific stop-loss thresholds (% probability move against us)
SPORT_STOP_LOSS_DEFAULTS: dict[str, float] = {
    "NBA": 3.0,
    "NCAAB": 3.0,
    "NFL": 5.0,
    "NCAAF": 5.0,
    "NHL": 7.0,
    "MLB": 6.0,
    "MLS": 7.0,
    "SOCCER": 7.0,
    "TENNIS": 4.0,
    "MMA": 8.0,
}


class SignalProcessor:
    """
    Processes incoming trading signals, applies filtering and risk checks,
    and emits ExecutionRequests for approved trades.
    """

    def __init__(
        self,
        min_edge_pct: float = 2.0,
        kelly_fraction: float = 0.25,
        max_position_pct: float = 10.0,
        max_buy_prob: float = 0.95,
        min_sell_prob: float = 0.05,
        # Position policy
        allow_hedging: bool = False,
        # Risk management
        max_daily_loss: float = 100.0,
        max_game_exposure: float = 50.0,
        max_sport_exposure: float = 200.0,
        max_latency_ms: float = 5000.0,
        # Cooldowns
        win_cooldown_seconds: float = 180.0,
        loss_cooldown_seconds: float = 300.0,
        # Initial bankroll (for sizing estimation)
        initial_bankroll: float = 1000.0,
        # Team matching confidence threshold
        team_match_min_confidence: float = 0.7,
    ):
        self.min_edge_pct = min_edge_pct
        self.kelly_fraction = kelly_fraction
        self.max_position_pct = max_position_pct
        self.max_buy_prob = max_buy_prob
        self.min_sell_prob = min_sell_prob
        self.allow_hedging = allow_hedging
        self.max_daily_loss = max_daily_loss
        self.max_game_exposure = max_game_exposure
        self.max_sport_exposure = max_sport_exposure
        self.max_latency_ms = max_latency_ms
        self.win_cooldown_seconds = win_cooldown_seconds
        self.loss_cooldown_seconds = loss_cooldown_seconds
        self.initial_bankroll = initial_bankroll
        self.team_match_min_confidence = team_match_min_confidence

        # Unified team matching client (Rust-based via Redis RPC)
        self.team_matching: Optional[TeamMatchingClient] = None

        # Connections
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.risk_controller: Optional[RiskController] = None

        # State
        self._running = False
        self._signal_count = 0
        self._approved_count = 0
        self._rejected_counts = {
            "edge": 0,
            "prob": 0,
            "duplicate": 0,
            "no_market": 0,
            "cooldown": 0,
            "risk": 0,
            "rule_blocked": 0,
        }

        # Heartbeat publisher
        self._heartbeat_publisher: Optional[HeartbeatPublisher] = None

        # Cooldown tracking: game_id -> (last_trade_time, was_win)
        self._game_cooldowns: dict[str, tuple[datetime, bool]] = {}

        # In-flight dedupe: idempotency_key -> timestamp
        self._in_flight: dict[str, datetime] = {}

        # Rule evaluator for feedback loop
        self.rule_evaluator: Optional[RuleEvaluator] = None

    async def start(self) -> None:
        """Start the signal processor."""
        logger.info("Starting SignalProcessor")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Initialize risk controller
        self.risk_controller = RiskController(
            pool=pool,
            max_daily_loss=self.max_daily_loss,
            max_game_exposure=self.max_game_exposure,
            max_sport_exposure=self.max_sport_exposure,
            max_latency_ms=self.max_latency_ms,
        )

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Connect to unified team matching service (Rust-based via Redis RPC)
        self.team_matching = TeamMatchingClient()
        await self.team_matching.connect()
        logger.info("Connected to unified team matching service")

        # Initialize rule evaluator for feedback loop
        self.rule_evaluator = RuleEvaluator(redis=self.redis, load_from_db=True)
        await self.rule_evaluator.start()

        self._running = True

        # Subscribe to signals
        await self.redis.subscribe(Channel.SIGNALS_NEW.value, self._handle_signal)

        # Start listening
        asyncio.create_task(self.redis.start_listening())

        # Start background tasks
        asyncio.create_task(self._heartbeat_loop())
        asyncio.create_task(self._cleanup_stale_inflight())

        # Start heartbeat publisher
        self._heartbeat_publisher = HeartbeatPublisher(
            service="signal_processor",
            instance_id=os.environ.get("HOSTNAME", "signal-processor-1"),
        )
        await self._heartbeat_publisher.start()
        self._heartbeat_publisher.set_status(ServiceStatus.HEALTHY)
        self._heartbeat_publisher.update_checks({
            "redis_ok": True,
            "db_ok": True,
        })

        logger.info(
            f"SignalProcessor started (min_edge={self.min_edge_pct}%, "
            f"max_buy_prob={self.max_buy_prob}, min_sell_prob={self.min_sell_prob})"
        )

    async def stop(self) -> None:
        """Stop the signal processor."""
        logger.info("Stopping SignalProcessor")
        self._running = False
        if self.rule_evaluator:
            await self.rule_evaluator.stop()
        if self._heartbeat_publisher:
            await self._heartbeat_publisher.stop()
        if self.team_matching:
            await self.team_matching.disconnect()
        if self.redis:
            await self.redis.disconnect()
        logger.info("SignalProcessor stopped")

    async def _handle_signal(self, data: dict) -> None:
        """Handle incoming trading signal."""
        self._signal_count += 1

        try:
            signal = TradingSignal(**data)

            # Create trace context for this signal flow
            trace = TraceContext(
                service="signal_processor",
                signal_id=signal.signal_id,
                game_id=signal.game_id,
                sport=signal.sport.value if signal.sport else None,
                side="buy" if signal.direction == SignalDirection.BUY else "sell",
            )

            # Log signal receipt
            trace.log(
                "signal_received",
                signal_type=signal.signal_type.value,
                direction=signal.direction.value,
                team=signal.team,
                model_prob=signal.model_prob,
                market_prob=signal.market_prob,
                edge_pct=signal.edge_pct,
                created_at=signal.created_at.isoformat() if signal.created_at else None,
            )

            logger.info(
                f"Received signal: {signal.signal_type.value} {signal.direction.value} "
                f"{signal.team} (edge: {signal.edge_pct:.1f}%)"
            )

            # Pre-trade filtering
            rejection = await self._apply_filters(signal, trace)
            if rejection:
                return

            # Get market price for execution
            market_price = await self._get_market_price(signal, trace)
            if not market_price:
                if signal.market_prob is not None:
                    market_price = self._create_price_from_signal(signal)
                    trace.log("market_lookup_fallback", source="signal_data")
                else:
                    self._rejected_counts["no_market"] += 1
                    trace.log("filter_rejected", filter="no_market", reason="no price available")
                    logger.warning(f"No market price for signal {signal.signal_id}")
                    return

            # Update trace with market details
            trace.update(
                platform=market_price.platform.value,
                market_id=str(market_price.market_id),
                contract_team=market_price.contract_team,
            )

            # If hedging is enabled, we allow multiple positions per game as long as they are not
            # the same position (same platform + market_id + side).
            # If hedging is disabled, _apply_filters enforces "one position per game" behavior.
            if self.allow_hedging:
                side = "buy" if signal.direction == SignalDirection.BUY else "sell"
                existing = await self._get_open_position_for_market(
                    platform=market_price.platform.value,
                    market_id=str(market_price.market_id),
                    side=side,
                )
                if existing:
                    self._rejected_counts["duplicate"] += 1
                    trace.log(
                        "filter_rejected",
                        filter="duplicate_hedging",
                        existing_trade_id=existing.get("trade_id"),
                    )
                    logger.info(
                        "Signal rejected: duplicate position - already have %s on %s:%s (game %s) (trade_id=%s)",
                        side,
                        market_price.platform.value,
                        market_price.market_id,
                        signal.game_id,
                        existing.get("trade_id"),
                    )
                    return

            # Estimate position size
            proposed_size = self._estimate_position_size(signal, market_price)

            # Risk check
            if self.risk_controller:
                risk_decision = await self.risk_controller.evaluate_trade(
                    game_id=signal.game_id,
                    sport=signal.sport.value,
                    team=signal.team,
                    side="buy" if signal.direction == SignalDirection.BUY else "sell",
                    proposed_size=proposed_size,
                    signal_timestamp=signal.created_at,
                )

                if not risk_decision.approved:
                    self._rejected_counts["risk"] += 1
                    trace.log(
                        "filter_rejected",
                        filter="risk",
                        reason=risk_decision.rejection_reason.value,
                        details=risk_decision.rejection_details,
                    )
                    logger.warning(
                        f"Trade REJECTED by risk: {risk_decision.rejection_reason.value} - "
                        f"{risk_decision.rejection_details}"
                    )
                    return

            # Create and emit ExecutionRequest
            exec_request = self._create_execution_request(signal, market_price, proposed_size)
            trace.update(trace_id=exec_request.idempotency_key)

            # Dedupe check
            if exec_request.idempotency_key in self._in_flight:
                self._rejected_counts["duplicate"] += 1
                trace.log(
                    "filter_rejected",
                    filter="in_flight_dedupe",
                    idempotency_key=exec_request.idempotency_key,
                )
                logger.info(f"Duplicate signal in-flight: {exec_request.idempotency_key}")
                return

            self._in_flight[exec_request.idempotency_key] = datetime.now(timezone.utc)

            # Log execution request creation
            trace.log(
                "execution_request_created",
                request_id=exec_request.request_id,
                idempotency_key=exec_request.idempotency_key,
                limit_price=exec_request.limit_price,
                proposed_size=proposed_size,
                yes_bid=market_price.yes_bid,
                yes_ask=market_price.yes_ask,
            )

            # Publish to execution channel
            await self.redis.publish(Channel.EXECUTION_REQUESTS.value, exec_request)
            self._approved_count += 1

            logger.info(
                f"Emitted ExecutionRequest: {exec_request.request_id} "
                f"({signal.direction.value} {signal.team} @ {exec_request.limit_price:.3f})"
            )

        except Exception as e:
            logger.error(f"Error handling signal: {e}", exc_info=True)

    async def _apply_filters(
        self, signal: TradingSignal, trace: Optional[TraceContext] = None
    ) -> Optional[str]:
        """Apply pre-trade filters. Returns rejection reason or None if passed."""

        def log_reject(filter_name: str, reason: str) -> None:
            if trace:
                trace.log("filter_rejected", filter=filter_name, reason=reason)

        # No market data
        if signal.market_prob is None:
            self._rejected_counts["no_market"] += 1
            log_reject("no_market", "no real market price available")
            logger.info("Signal rejected: no real market price available")
            return "no_market"

        # Edge threshold
        if signal.edge_pct < self.min_edge_pct:
            self._rejected_counts["edge"] += 1
            log_reject("edge", f"edge {signal.edge_pct:.1f}% < min {self.min_edge_pct}%")
            logger.debug(f"Signal rejected: edge {signal.edge_pct}% < min {self.min_edge_pct}%")
            return "edge"

        # Probability bounds
        if signal.direction == SignalDirection.BUY and signal.model_prob > self.max_buy_prob:
            self._rejected_counts["prob"] += 1
            log_reject("prob_high", f"BUY at {signal.model_prob*100:.1f}% > max {self.max_buy_prob*100:.0f}%")
            logger.info(f"Signal rejected: BUY at {signal.model_prob*100:.1f}% > max {self.max_buy_prob*100:.0f}%")
            return "prob_high"

        if signal.direction == SignalDirection.SELL and signal.model_prob < self.min_sell_prob:
            self._rejected_counts["prob"] += 1
            log_reject("prob_low", f"SELL at {signal.model_prob*100:.1f}% < min {self.min_sell_prob*100:.0f}%")
            logger.info(f"Signal rejected: SELL at {signal.model_prob*100:.1f}% < min {self.min_sell_prob*100:.0f}%")
            return "prob_low"

        # Duplicate position check (game-level).
        # When hedging is enabled we do *not* block by game_id; we only block identical positions
        # later once we know (platform, market_id, side) from MarketPrice.
        if not self.allow_hedging:
            existing = await self._get_open_position_for_game(signal.game_id)
            if existing:
                existing_side = existing.get("side")
                new_side = "buy" if signal.direction == SignalDirection.BUY else "sell"
                if existing_side == new_side:
                    self._rejected_counts["duplicate"] += 1
                    log_reject("duplicate", f"already have {existing_side} position on game {signal.game_id}")
                    logger.info(f"Signal rejected: already have {existing_side} position on game {signal.game_id}")
                    return "duplicate"
                # Opposite direction would close position - let it through for now
                # (PositionTracker handles the close)

        # Cooldown check
        in_cooldown, reason = self._is_game_in_cooldown(signal.game_id)
        if in_cooldown:
            self._rejected_counts["cooldown"] += 1
            log_reject("cooldown", reason or "game in cooldown")
            logger.info(f"Signal rejected: game {signal.game_id} in {reason}")
            return "cooldown"

        # Feedback loop rule evaluation
        if self.rule_evaluator:
            rule_decision = await self.rule_evaluator.evaluate(signal)
            if not rule_decision.allowed:
                self._rejected_counts["rule_blocked"] += 1
                log_reject(
                    "rule_blocked",
                    f"{rule_decision.rule_id}: {rule_decision.reason}"
                )
                logger.info(
                    f"Signal rejected by rule: {rule_decision.rule_id} - {rule_decision.reason}"
                )
                return f"rule_blocked:{rule_decision.rule_id}"

        # Log filters passed
        if trace:
            trace.log("filters_passed")

        return None

    async def _get_open_position_for_game(self, game_id: str) -> Optional[dict]:
        """Get existing open position for a game."""
        pool = await get_pool()
        row = await pool.fetchrow("""
            SELECT trade_id, game_id, side, entry_price, size, time
            FROM paper_trades
            WHERE game_id = $1 AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
        """, game_id)
        return dict(row) if row else None

    async def _get_open_position_for_market(
        self,
        platform: str,
        market_id: str,
        side: str,
    ) -> Optional[dict]:
        """Get existing open position for an exact position identity (platform/market/side)."""
        pool = await get_pool()
        row = await pool.fetchrow(
            """
            SELECT trade_id, game_id, side, entry_price, size, time, platform, market_id
            FROM paper_trades
            WHERE platform = $1
              AND market_id = $2
              AND side = $3
              AND status = 'open'
            ORDER BY time DESC
            LIMIT 1
            """,
            platform,
            market_id,
            side,
        )
        return dict(row) if row else None

    def _is_game_in_cooldown(self, game_id: str) -> tuple[bool, Optional[str]]:
        """Check if game is in cooldown period."""
        if game_id not in self._game_cooldowns:
            return False, None

        last_trade_time, was_win = self._game_cooldowns[game_id]
        elapsed = (datetime.now(timezone.utc) - last_trade_time).total_seconds()

        cooldown = self.win_cooldown_seconds if was_win else self.loss_cooldown_seconds
        if elapsed < cooldown:
            remaining = cooldown - elapsed
            return True, f"{'win' if was_win else 'loss'} cooldown ({remaining:.0f}s remaining)"

        # Cooldown expired
        del self._game_cooldowns[game_id]
        return False, None

    async def _get_market_price(
        self, signal: TradingSignal, trace: Optional[TraceContext] = None
    ) -> Optional[MarketPrice]:
        """
        Get current market price for the signal with strict team validation.

        Uses confidence-scored team matching to prevent wrong-team price selection.
        """
        pool = await get_pool()
        target_team = (signal.team or "").strip()

        # If signal has a team, use confidence-scored matching
        if target_team:
            # Fetch recent prices with contract_team for this game
            rows = await pool.fetch(
                """
                SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                       yes_bid_size, yes_ask_size, volume, liquidity, time, platform
                FROM market_prices
                WHERE game_id = $1
                  AND contract_team IS NOT NULL
                  AND time > NOW() - INTERVAL '2 minutes'
                ORDER BY time DESC
                LIMIT 10
                """,
                signal.game_id,
            )

            # Score each candidate and track all for logging
            candidates_scored: list[dict] = []
            best_match: Optional[dict] = None
            best_confidence = 0.0
            best_result: Optional[TeamMatchResult] = None

            for row in rows:
                contract_team = row["contract_team"]
                sport_value = signal.sport.value.lower() if signal.sport else "nba"

                # Use unified team matching client (Rust-based)
                if self.team_matching:
                    match_result = await self.team_matching.match_teams(
                        target_team=target_team,
                        candidate_team=contract_team,
                        sport=sport_value,
                        timeout=2.0,
                    )
                else:
                    # Fallback: no match client - reject entry
                    match_result = None

                # Handle timeout/error (fail-closed: reject entry)
                if match_result is None:
                    candidates_scored.append({
                        "contract_team": contract_team,
                        "confidence": 0.0,
                        "method": "rpc_timeout",
                        "is_match": False,
                    })
                    if trace:
                        trace.log(
                            "market_lookup_rpc_timeout",
                            target_team=target_team,
                            contract_team=contract_team,
                        )
                    continue

                candidates_scored.append({
                    "contract_team": contract_team,
                    "confidence": match_result.confidence,
                    "method": match_result.method,
                    "is_match": match_result.is_match,
                })

                if match_result.is_match and match_result.confidence > best_confidence:
                    best_confidence = match_result.confidence
                    best_result = match_result
                    best_match = dict(row)

            # Log all candidates evaluated
            if trace:
                trace.log(
                    "market_lookup_candidates",
                    target_team=target_team,
                    candidates_found=len(rows),
                    candidates_scored=candidates_scored,
                    best_confidence=best_confidence,
                    min_confidence_threshold=self.team_match_min_confidence,
                )

            # Only accept matches with confidence >= threshold
            if best_match and best_confidence >= self.team_match_min_confidence:
                price = MarketPrice(
                    market_id=best_match["market_id"],
                    platform=Platform(best_match["platform"]),
                    market_title=best_match["market_title"],
                    contract_team=best_match["contract_team"],
                    yes_bid=float(best_match["yes_bid"]),
                    yes_ask=float(best_match["yes_ask"]),
                    yes_bid_size=float(best_match.get("yes_bid_size") or 0),
                    yes_ask_size=float(best_match.get("yes_ask_size") or 0),
                    volume=float(best_match["volume"] or 0),
                    liquidity=float(best_match.get("liquidity") or 0),
                )

                price_age_ms = None
                if best_match.get("time"):
                    price_age_ms = (datetime.now(timezone.utc) - best_match["time"]).total_seconds() * 1000

                if trace:
                    trace.log(
                        "market_lookup_selected",
                        target_team=target_team,
                        contract_team=price.contract_team,
                        confidence=best_confidence,
                        match_method=best_result.method if best_result else None,
                        match_reason=best_result.reason if best_result else None,
                        market_id=price.market_id,
                        platform=price.platform.value,
                        yes_bid=price.yes_bid,
                        yes_ask=price.yes_ask,
                        price_age_ms=price_age_ms,
                    )

                logger.info(
                    f"Team match validated: '{target_team}' -> '{price.contract_team}' "
                    f"(confidence={best_confidence:.0%}, method={best_result.method if best_result else 'N/A'})"
                )

                return price

            # No confident match found - reject
            if trace:
                trace.log(
                    "market_lookup_rejected",
                    reason="low_confidence_team_match",
                    target_team=target_team,
                    best_confidence=best_confidence,
                    min_confidence_threshold=self.team_match_min_confidence,
                    candidates_found=len(rows),
                )

            logger.warning(
                f"No confident team match for '{target_team}' in game {signal.game_id} "
                f"(best_confidence={best_confidence:.0%}, threshold={self.team_match_min_confidence:.0%}, "
                f"candidates={len(rows)})"
            )
            return None

        # Fallback: no team specified - use any recent price (legacy behavior)
        # This path is for signals that don't have a team specified
        row = await pool.fetchrow(
            """
            SELECT market_id, market_title, contract_team, yes_bid, yes_ask,
                   yes_bid_size, yes_ask_size, volume, liquidity, time, platform
            FROM market_prices
            WHERE game_id = $1
            ORDER BY time DESC
            LIMIT 1
            """,
            signal.game_id,
        )

        if row:
            if trace:
                trace.log(
                    "market_lookup_selected",
                    source="fallback_no_team_in_signal",
                    market_id=row["market_id"],
                    platform=row["platform"],
                    contract_team=row.get("contract_team"),
                    yes_bid=float(row["yes_bid"]),
                    yes_ask=float(row["yes_ask"]),
                )
            return MarketPrice(
                market_id=row["market_id"],
                platform=Platform(row["platform"]),
                market_title=row["market_title"],
                contract_team=row.get("contract_team"),
                yes_bid=float(row["yes_bid"]),
                yes_ask=float(row["yes_ask"]),
                yes_bid_size=float(row.get("yes_bid_size") or 0),
                yes_ask_size=float(row.get("yes_ask_size") or 0),
                volume=float(row["volume"] or 0),
                liquidity=float(row.get("liquidity") or 0),
            )

        if trace:
            trace.log("market_lookup_rejected", reason="no_price_found")
        return None

    def _create_price_from_signal(self, signal: TradingSignal) -> MarketPrice:
        """Create market price from signal's captured market data."""
        market_prob = signal.market_prob
        spread = 0.02
        return MarketPrice(
            market_id=f"signal_{signal.game_id}",
            platform=Platform.PAPER,
            market_title=f"{signal.team} to win",
            contract_team=signal.team,
            yes_bid=max(0.01, market_prob - spread),
            yes_ask=min(0.99, market_prob + spread),
            volume=0,
            liquidity=10000,
        )

    def _estimate_position_size(self, signal: TradingSignal, market_price: MarketPrice) -> float:
        """Estimate position size using Kelly criterion."""
        # Get bankroll from DB or use initial
        bankroll = self.initial_bankroll

        kelly = signal.kelly_fraction if signal.kelly_fraction > 0 else 0.0
        fractional_kelly = kelly * self.kelly_fraction
        position_pct = min(fractional_kelly * 100, self.max_position_pct)
        position_size = bankroll * (position_pct / 100)

        return max(1.0, position_size)

    def _create_execution_request(
        self,
        signal: TradingSignal,
        market_price: MarketPrice,
        size: float,
    ) -> ExecutionRequest:
        """Create an ExecutionRequest from signal and market price."""
        side = ExecutionSide.YES if signal.direction == SignalDirection.BUY else ExecutionSide.NO
        limit_price = market_price.yes_ask if side == ExecutionSide.YES else market_price.yes_bid

        return ExecutionRequest(
            request_id=str(uuid4()),
            idempotency_key=f"{signal.signal_id}_{signal.game_id}_{signal.team}",
            game_id=signal.game_id,
            sport=signal.sport,
            platform=market_price.platform,
            market_id=market_price.market_id,
            contract_team=market_price.contract_team or signal.team,
            side=side,
            limit_price=limit_price,
            size=size,
            signal_id=signal.signal_id,
            signal_type=signal.signal_type.value,
            edge_pct=signal.edge_pct,
            model_prob=signal.model_prob,
            market_prob=signal.market_prob,
            reason=signal.reason,
            created_at=datetime.now(timezone.utc),
        )

    async def _heartbeat_loop(self) -> None:
        """Send periodic status updates."""
        while self._running:
            try:
                status = {
                    "type": "signal_processor",
                    "signals_received": self._signal_count,
                    "approved": self._approved_count,
                    "rejected": self._rejected_counts,
                    "in_flight": len(self._in_flight),
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                }
                logger.info(
                    f"SignalProcessor: {self._approved_count}/{self._signal_count} approved, "
                    f"rejected={self._rejected_counts}"
                )
                
                # Update health monitoring heartbeat
                if self._heartbeat_publisher:
                    total_rejected = sum(self._rejected_counts.values())
                    approval_rate = (
                        self._approved_count / self._signal_count * 100
                        if self._signal_count > 0 else 100.0
                    )
                    self._heartbeat_publisher.update_metrics({
                        "signals_received": float(self._signal_count),
                        "signals_approved": float(self._approved_count),
                        "signals_rejected": float(total_rejected),
                        "approval_rate_pct": approval_rate,
                    })
            except Exception as e:
                logger.warning(f"Heartbeat error: {e}")
            await asyncio.sleep(30)

    async def _cleanup_stale_inflight(self) -> None:
        """Clean up stale in-flight entries."""
        while self._running:
            try:
                cutoff = datetime.now(timezone.utc) - timedelta(minutes=5)
                stale = [k for k, v in self._in_flight.items() if v < cutoff]
                for k in stale:
                    del self._in_flight[k]
                if stale:
                    logger.debug(f"Cleaned {len(stale)} stale in-flight entries")
            except Exception as e:
                logger.warning(f"Cleanup error: {e}")
            await asyncio.sleep(60)


async def main():
    """Main entry point."""
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO"),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    processor = SignalProcessor(
        min_edge_pct=float(os.environ.get("MIN_EDGE_PCT", "2.0")),
        kelly_fraction=float(os.environ.get("KELLY_FRACTION", "0.25")),
        max_position_pct=float(os.environ.get("MAX_POSITION_PCT", "10.0")),
        max_buy_prob=float(os.environ.get("MAX_BUY_PROB", "0.95")),
        min_sell_prob=float(os.environ.get("MIN_SELL_PROB", "0.05")),
        allow_hedging=os.environ.get("ALLOW_HEDGING", "false").lower() in ("1", "true", "yes", "y", "on"),
        max_daily_loss=float(os.environ.get("MAX_DAILY_LOSS", "100.0")),
        max_game_exposure=float(os.environ.get("MAX_GAME_EXPOSURE", "50.0")),
        max_sport_exposure=float(os.environ.get("MAX_SPORT_EXPOSURE", "200.0")),
        max_latency_ms=float(os.environ.get("MAX_LATENCY_MS", "5000.0")),
        win_cooldown_seconds=float(os.environ.get("WIN_COOLDOWN_SECONDS", "180.0")),
        loss_cooldown_seconds=float(os.environ.get("LOSS_COOLDOWN_SECONDS", "300.0")),
        initial_bankroll=float(os.environ.get("INITIAL_BANKROLL", "1000")),
        team_match_min_confidence=float(os.environ.get("TEAM_MATCH_MIN_CONFIDENCE", "0.7")),
    )

    await processor.start()

    try:
        while True:
            await asyncio.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        await processor.stop()


if __name__ == "__main__":
    asyncio.run(main())
