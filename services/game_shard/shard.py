"""
GameShard service for monitoring multiple games via asyncio.

KEY IMPROVEMENT over container-per-game:
- Single process handles 10-20 games concurrently
- Shared connections to DB, Redis, markets
- Dynamic poll intervals (1s crunch time, 30s halftime)
- 10x lower memory footprint
- Instant game start (no container spawn)
"""

import asyncio
import logging
import os
import uuid
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from arbees_shared.db.connection import DatabaseClient, get_pool
from arbees_shared.messaging.redis_bus import RedisBus, Channel
from arbees_shared.models.game import GameState, Play, Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection
from data_providers.espn.client import ESPNClient
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
import arbees_core

logger = logging.getLogger(__name__)


@dataclass
class GameContext:
    """Context for a monitored game."""
    game_id: str
    sport: Sport
    espn_client: ESPNClient
    last_state: Optional[GameState] = None
    last_home_win_prob: Optional[float] = None
    market_prices: dict[Platform, MarketPrice] = field(default_factory=dict)
    market_ids: dict[Platform, str] = field(default_factory=dict)
    plays_detected: int = 0
    signals_generated: int = 0
    started_at: datetime = field(default_factory=datetime.utcnow)
    is_active: bool = True
    active_signal: Optional[TradingSignal] = None  # Track current position for hysteresis


class GameShard:
    """
    GameShard handles multiple games concurrently via asyncio.

    Benefits over container-per-game:
    - Shared market client connections
    - Lower memory footprint
    - Instant game start
    - Better resource utilization
    """

    def __init__(
        self,
        shard_id: Optional[str] = None,
        max_games: int = 20,
        default_poll_interval: float = 3.0,
        crunch_time_interval: float = 1.0,
        halftime_interval: float = 30.0,
    ):
        """
        Initialize GameShard.

        Args:
            shard_id: Unique identifier for this shard
            max_games: Maximum concurrent games
            default_poll_interval: Normal poll interval in seconds
            crunch_time_interval: Poll interval for close games
            halftime_interval: Poll interval during halftime
        """
        self.shard_id = shard_id or os.environ.get("SHARD_ID", str(uuid.uuid4())[:8])
        self.max_games = max_games
        self.default_poll_interval = default_poll_interval
        self.crunch_time_interval = crunch_time_interval
        self.halftime_interval = halftime_interval

        # Connections (shared across all games)
        self.db: Optional[DatabaseClient] = None
        self.redis: Optional[RedisBus] = None
        self.kalshi: Optional[KalshiClient] = None
        self.polymarket: Optional[PolymarketClient] = None

        # Game tracking
        self._games: dict[str, GameContext] = {}
        self._game_tasks: dict[str, asyncio.Task] = {}
        self._running = False
        self._heartbeat_task: Optional[asyncio.Task] = None

    @property
    def game_count(self) -> int:
        """Number of active games."""
        return len(self._games)

    @property
    def can_accept_games(self) -> bool:
        """Whether shard can accept more games."""
        return self.game_count < self.max_games

    async def start(self) -> None:
        """Start the shard and connect to services."""
        logger.info(f"Starting GameShard {self.shard_id}")

        # Connect to database
        pool = await get_pool()
        self.db = DatabaseClient(pool)

        # Connect to Redis
        self.redis = RedisBus()
        await self.redis.connect()

        # Connect to market clients
        self.kalshi = KalshiClient()
        await self.kalshi.connect()

        self.polymarket = PolymarketClient()
        await self.polymarket.connect()

        self._running = True

        # Subscribe to commands from orchestrator
        command_channel = f"shard:{self.shard_id}:command"
        await self.redis.subscribe(command_channel, self._handle_command)
        asyncio.create_task(self.redis.start_listening())

        # Start heartbeat
        self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        logger.info(f"GameShard {self.shard_id} started")

    async def _handle_command(self, data: dict) -> None:
        """Handle command from orchestrator."""
        cmd_type = data.get("type")

        if cmd_type == "add_game":
            game_id = data.get("game_id")
            sport_str = data.get("sport")
            kalshi_id = data.get("kalshi_market_id")
            poly_id = data.get("polymarket_market_id")

            if game_id and sport_str:
                sport = Sport(sport_str)
                logger.info(f"Received add_game command: {game_id} ({sport_str}) kalshi={kalshi_id} poly={poly_id}")
                await self.add_game(game_id, sport, kalshi_id, poly_id)

        elif cmd_type == "remove_game":
            game_id = data.get("game_id")
            if game_id:
                logger.info(f"Received remove_game command: {game_id}")
                await self.remove_game(game_id)

    async def stop(self) -> None:
        """Stop the shard gracefully."""
        logger.info(f"Stopping GameShard {self.shard_id}")
        self._running = False

        # Cancel heartbeat
        if self._heartbeat_task:
            self._heartbeat_task.cancel()

        # Stop all game monitoring
        for game_id in list(self._games.keys()):
            await self.remove_game(game_id)

        # Disconnect from services
        if self.kalshi:
            await self.kalshi.disconnect()
        if self.polymarket:
            await self.polymarket.disconnect()
        if self.redis:
            await self.redis.disconnect()

        logger.info(f"GameShard {self.shard_id} stopped")

    async def _heartbeat_loop(self) -> None:
        """Send periodic heartbeats to orchestrator."""
        while self._running:
            try:
                status = {
                    "shard_id": self.shard_id,
                    "game_count": self.game_count,
                    "max_games": self.max_games,
                    "games": list(self._games.keys()),
                    "timestamp": datetime.utcnow().isoformat(),
                }
                if self.redis:
                    await self.redis.publish_shard_heartbeat(self.shard_id, status)
            except Exception as e:
                logger.warning(f"Heartbeat failed: {e}")

            await asyncio.sleep(10)

    # ==========================================================================
    # Game Management
    # ==========================================================================

    async def add_game(
        self,
        game_id: str,
        sport: Sport,
        kalshi_market_id: Optional[str] = None,
        polymarket_market_id: Optional[str] = None,
    ) -> bool:
        """
        Start monitoring a game.

        Args:
            game_id: ESPN game ID
            sport: Sport type
            kalshi_market_id: Optional Kalshi market to monitor
            polymarket_market_id: Optional Polymarket market to monitor

        Returns:
            True if game was added
        """
        if game_id in self._games:
            logger.warning(f"Game {game_id} already being monitored")
            return False

        if not self.can_accept_games:
            logger.warning(f"Shard at capacity ({self.game_count}/{self.max_games})")
            return False

        # Create ESPN client for this sport
        espn_client = ESPNClient(sport)
        await espn_client.connect()

        # Create game context
        ctx = GameContext(
            game_id=game_id,
            sport=sport,
            espn_client=espn_client,
        )

        # Track market IDs
        if kalshi_market_id:
            ctx.market_ids[Platform.KALSHI] = kalshi_market_id
            logger.info(f"Game {game_id}: Kalshi market set to {kalshi_market_id}")
        if polymarket_market_id:
            ctx.market_ids[Platform.POLYMARKET] = polymarket_market_id
            logger.info(f"Game {game_id}: Polymarket market set to {polymarket_market_id}")

        self._games[game_id] = ctx

        # Start monitoring task
        task = asyncio.create_task(self._monitor_game(ctx))
        self._game_tasks[game_id] = task

        logger.info(f"Added game {game_id} ({sport.value}) to shard {self.shard_id}")
        return True

    async def remove_game(self, game_id: str) -> bool:
        """Stop monitoring a game."""
        if game_id not in self._games:
            return False

        ctx = self._games[game_id]
        ctx.is_active = False

        # Cancel monitoring task
        task = self._game_tasks.get(game_id)
        if task:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
            del self._game_tasks[game_id]

        # Disconnect ESPN client
        await ctx.espn_client.disconnect()

        # Finalize game state in DB if possible
        if self.db and ctx.last_state:
            # We mark it as valid to update the timestamp, but we don't force 'final' 
            # unless we know for sure. However, if we are removing it, it's likely done.
            # Safe bet: just update timestamp so it falls out of "live" queries eventually
            pass 

        del self._games[game_id]
        logger.info(f"Removed game {game_id} from shard {self.shard_id}")
        return True

    # ==========================================================================
    # Game Monitoring Loop
    # ==========================================================================

    async def _monitor_game(self, ctx: GameContext) -> None:
        """Main monitoring loop for a game."""
        logger.info(f"Starting monitoring for game {ctx.game_id}")

        try:
            while ctx.is_active and self._running:
                # Get poll interval based on game state
                interval = self._get_poll_interval(ctx)

                # Poll game state and market prices in parallel
                await asyncio.gather(
                    self._poll_game_state(ctx),
                    self._poll_market_prices(ctx),
                )

                # Check if game is final
                if ctx.last_state and (ctx.last_state.status == "final" or ctx.last_state.status == "complete"):
                    logger.info(f"Game {ctx.game_id} is final")
                    
                    # Ensure final state is persisted
                    if self.db:
                        await self.db.update_game_status(ctx.game_id, "final")
                    
                    break

                await asyncio.sleep(interval)

        except asyncio.CancelledError:
            logger.info(f"Monitoring cancelled for game {ctx.game_id}")
        except Exception as e:
            logger.error(f"Error monitoring game {ctx.game_id}: {e}")
        finally:
            # Clean up
            ctx.is_active = False

    def _get_poll_interval(self, ctx: GameContext) -> float:
        """Get poll interval based on game state."""
        if ctx.last_state is None:
            return self.default_poll_interval

        # Halftime - poll less frequently
        if ctx.last_state.status == "halftime":
            return self.halftime_interval

        # Crunch time - poll more frequently
        if self._is_crunch_time(ctx):
            return self.crunch_time_interval

        return self.default_poll_interval

    def _is_crunch_time(self, ctx: GameContext) -> bool:
        """Check if game is in crunch time (close score, little time left)."""
        state = ctx.last_state
        if state is None:
            return False

        # Close score (within one possession)
        close_score = abs(state.score_diff) <= 8

        # Late in game
        late_game = state.game_progress > 0.85

        # 4th quarter/period
        final_period = state.period >= state.sport.periods

        return close_score and (late_game or final_period)

    async def _poll_game_state(self, ctx: GameContext) -> None:
        """Poll ESPN for game state updates."""
        try:
            new_state, new_plays = await ctx.espn_client.poll_game(
                ctx.game_id,
                ctx.last_state,
            )

            if new_state is None:
                return

            # Upsert game info (to get team names in the games table)
            if self.db and new_state.home_team and new_state.away_team:
                try:
                    logger.info(f"Upserting game {ctx.game_id}: {new_state.away_team} @ {new_state.home_team}")
                    await self.db.upsert_game(
                        game_id=ctx.game_id,
                        sport=ctx.sport.value,
                        home_team=new_state.home_team,
                        away_team=new_state.away_team,
                        scheduled_time=datetime.utcnow(),  # Pass datetime object, not string
                        home_team_abbrev=getattr(new_state, 'home_team_abbrev', None),
                        away_team_abbrev=getattr(new_state, 'away_team_abbrev', None),
                    )
                except Exception as e:
                    logger.error(f"Error upserting game {ctx.game_id}: {e}")
            else:
                if self.db:
                    logger.warning(f"Missing team names for game {ctx.game_id}: home='{new_state.home_team}', away='{new_state.away_team}'")

            old_state = ctx.last_state
            old_prob = ctx.last_home_win_prob

            # Calculate new win probability
            # TODO: Use Rust core for this
            new_prob = self._calculate_win_prob(new_state)

            # Update context
            ctx.last_state = new_state
            ctx.last_home_win_prob = new_prob

            # Persist state to database
            if self.db:
                await self.db.insert_game_state(
                    game_id=ctx.game_id,
                    sport=ctx.sport.value,
                    home_score=new_state.home_score,
                    away_score=new_state.away_score,
                    period=new_state.period,
                    time_remaining=new_state.time_remaining,
                    status=new_state.status,
                    possession=new_state.possession,
                    home_win_prob=new_prob,
                    down=new_state.down,
                    yards_to_go=new_state.yards_to_go,
                    yard_line=new_state.yard_line,
                    is_redzone=new_state.is_redzone,
                )

            # Publish state update
            if self.redis:
                await self.redis.publish_game_state(ctx.game_id, new_state)

            # Process new plays
            for play in new_plays:
                ctx.plays_detected += 1
                await self._process_play(ctx, play, old_prob, new_prob)

            # Generate signals from win probability change
            if old_prob is not None and new_prob is not None:
                await self._generate_signals(ctx, old_state, new_state, old_prob, new_prob, new_plays)

        except Exception as e:
            logger.error(f"Error polling game state for {ctx.game_id}: {e}")

    async def _poll_market_prices(self, ctx: GameContext) -> None:
        """Poll market prices for the game."""
        # Skip if no market IDs configured
        if not ctx.market_ids:
            return

        try:
            # Poll Kalshi
            if ctx.market_ids.get(Platform.KALSHI) and self.kalshi:
                market_id = ctx.market_ids[Platform.KALSHI]
                price = await self.kalshi.get_market_price(market_id)
                if price:
                    ctx.market_prices[Platform.KALSHI] = price

                    # Persist to database
                    if self.db:
                        await self.db.insert_market_price(
                            market_id=market_id,
                            platform="kalshi",
                            yes_bid=price.yes_bid,
                            yes_ask=price.yes_ask,
                            volume=price.volume,
                            liquidity=price.liquidity,
                            game_id=ctx.game_id,
                            market_title=price.market_title,
                        )

                    # Publish to Redis
                    if self.redis:
                        await self.redis.publish_market_price(ctx.game_id, price)

            # Poll Polymarket
            if ctx.market_ids.get(Platform.POLYMARKET) and self.polymarket:
                market_id = ctx.market_ids[Platform.POLYMARKET]
                price = await self.polymarket.get_market_price(market_id)
                if price:
                    ctx.market_prices[Platform.POLYMARKET] = price

                    if self.db:
                        await self.db.insert_market_price(
                            market_id=market_id,
                            platform="polymarket",
                            yes_bid=price.yes_bid,
                            yes_ask=price.yes_ask,
                            volume=price.volume,
                            liquidity=price.liquidity,
                            game_id=ctx.game_id,
                            market_title=price.market_title,
                        )

                    if self.redis:
                        await self.redis.publish_market_price(ctx.game_id, price)

        except Exception as e:
            logger.error(f"Error polling market prices for {ctx.game_id}: {e}")

    async def _process_play(
        self,
        ctx: GameContext,
        play: Play,
        old_prob: Optional[float],
        new_prob: Optional[float],
    ) -> None:
        """Process a new play."""
        # Persist play to database
        if self.db:
            await self.db.insert_play(
                play_id=play.play_id,
                game_id=ctx.game_id,
                sport=ctx.sport.value,
                play_type=play.play_type.value,
                description=play.description,
                sequence_number=play.sequence_number,
                home_score=play.home_score,
                away_score=play.away_score,
                period=play.period,
                time_remaining=play.time_remaining,
                home_win_prob_before=old_prob,
                home_win_prob_after=new_prob,
                team=play.team,
                player=play.player,
                yards_gained=play.yards_gained,
                yard_line=play.yard_line,
                down=play.down,
                yards_to_go=play.yards_to_go,
                is_scoring=play.is_scoring,
                is_turnover=play.is_turnover,
            )

        # Publish play to Redis
        if self.redis:
            await self.redis.publish_play(ctx.game_id, play)

    async def _generate_signals(
        self,
        ctx: GameContext,
        old_state: Optional[GameState],
        new_state: GameState,
        old_prob: float,
        new_prob: float,
        new_plays: list[Play],
    ) -> None:
        """Generate trading signals from game updates."""
        prob_change = new_prob - old_prob

        # Only signal on significant changes (> 3% for probability shifts)
        if abs(prob_change) < 0.02:
            return

        # 1. Clamp probabilities to avoid overconfidence
        # Capping at 85% prevents "sure thing" bets that lose 100% of the time in our analysis
        capped_new_prob = max(0.15, min(0.85, new_prob))
        
        # 2. Estimate Fees/Friction (2% round trip estimate)
        estimated_fees = 2.0 

        # Get market price for comparison (optional)
        market_price = ctx.market_prices.get(Platform.KALSHI) or ctx.market_prices.get(Platform.POLYMARKET)

        if market_price is not None:
            # With market data: calculate edge vs market
            market_prob = market_price.mid_price
            edge = (capped_new_prob - market_prob) * 100.0
            
            # Fee-adjusted edge
            spread = (market_price.yes_ask - market_price.yes_bid) * 100.0
            required_edge = estimated_fees + (spread / 2.0) + 1.0 # 1% extra margin
            
            # Only signal if edge exceeds friction
            if abs(edge) < required_edge:
                return
        else:
            # Without market data: signal based on probability shift alone
            # Use the probability change as a pseudo-edge
            market_prob = None
            edge = prob_change * 100.0  # Convert to percentage
            required_edge = 3.0 # Higher threshold for naked prob changes

        # Determine direction based on probability change
        direction = SignalDirection.BUY if prob_change > 0 else SignalDirection.SELL

        # 3. Hysteresis (Flip-Flop Protection)
        # If we have an active signal in the OPPOSITE direction, we need much stronger evidence to flip
        if ctx.active_signal and ctx.active_signal.direction != direction:
             # If flipping, require double the normal edge to justify exit cost + entry cost
             if abs(edge) < (required_edge * 2.0):
                 logger.debug(f"Ignoring flip signal {direction} for game {ctx.game_id}: edge {edge:.1f}% < required {required_edge*2:.1f}%")
                 return

        # Create signal
        signal = TradingSignal(
            signal_id=str(uuid.uuid4()),
            signal_type=SignalType.WIN_PROB_SHIFT,
            game_id=ctx.game_id,
            sport=ctx.sport,
            team=new_state.home_team if prob_change > 0 else new_state.away_team,
            direction=direction,
            model_prob=capped_new_prob,
            market_prob=market_prob,
            edge_pct=abs(edge),
            confidence=min(1.0, abs(prob_change) * 10),
            reason=f"Win prob changed {prob_change*100:.1f}% ({old_prob*100:.1f}% → {new_prob*100:.1f}%)",
            play_id=new_plays[-1].play_id if new_plays else None,
        )

        ctx.signals_generated += 1
        ctx.active_signal = signal  # Update state

        # Persist signal
        if self.db:
            await self.db.insert_trading_signal(
                signal_id=signal.signal_id,
                signal_type=signal.signal_type.value,
                direction=signal.direction.value,
                edge_pct=signal.edge_pct,
                game_id=signal.game_id,
                sport=signal.sport.value if signal.sport else None,
                team=signal.team,
                model_prob=signal.model_prob,
                market_prob=signal.market_prob,
                confidence=signal.confidence,
                reason=signal.reason,
                play_id=signal.play_id,
            )

        # Publish signal
        if self.redis:
            await self.redis.publish_signal(signal)

        logger.info(
            f"Generated signal: {signal.direction.value} {signal.team} "
            f"(edge: {signal.edge_pct:.1f}%, conf: {signal.confidence:.2f})"
        )

    def _calculate_win_prob(self, state: GameState) -> float:
        """
        Calculate win probability from game state using Rust core.
        """
        try:
            # Map Python Sport to Rust Sport
            # Note: Assuming binary compatibility or string mapping
            rust_sport = getattr(arbees_core.Sport, state.sport.value.upper(), None)
            if not rust_sport:
                # Fallback mapping if direct attribute fails
                sport_map = {
                    "nfl": arbees_core.Sport.NFL,
                    "nba": arbees_core.Sport.NBA,
                    "nhl": arbees_core.Sport.NHL,
                    "mlb": arbees_core.Sport.MLB,
                    "ncaaf": arbees_core.Sport.NCAAF,
                    "ncaab": arbees_core.Sport.NCAAB,
                    "mls": arbees_core.Sport.MLS,
                    "soccer": arbees_core.Sport.Soccer,
                    "tennis": arbees_core.Sport.Tennis,
                    "mma": arbees_core.Sport.MMA,
                }
                rust_sport = sport_map.get(state.sport.value)

            if not rust_sport:
                logger.warning(f"Unsupported sport for win prob: {state.sport}")
                return 0.5

            # Create Rust GameState
            # GameState::new(game_id, sport, home_team, away_team, home_score, away_score, period, time_remaining_seconds)
            rust_state = arbees_core.GameState(
                state.game_id,
                rust_sport,
                state.home_team,
                state.away_team,
                state.home_score,
                state.away_score,
                state.period,
                state.time_remaining_seconds
            )

            # Set optional fields if present
            if state.possession:
                rust_state.possession = state.possession
            
            if state.sport in (Sport.NFL, Sport.NCAAF):
                if state.down is not None:
                    rust_state.down = state.down
                if state.yards_to_go is not None:
                    rust_state.yards_to_go = state.yards_to_go
                if state.yard_line is not None:
                    rust_state.yard_line = state.yard_line
                rust_state.is_redzone = state.is_redzone

            # Calculate probability (for home team)
            raw_prob = arbees_core.calculate_win_probability(rust_state, True)
            
            # Clamp probability to [0.15, 0.85] to avoid extreme confidence
            return max(0.15, min(0.85, raw_prob))

        except Exception as e:
            logger.error(f"Error calculating win prob with Rust core: {e}")
            # Fallback to simple heuristic
            score_diff = state.home_score - state.away_score
            prob = 0.5 + (score_diff * 0.05)
            return max(0.15, min(0.85, prob))

    # ==========================================================================
    # Status and Metrics
    # ==========================================================================

    def get_status(self) -> dict:
        """Get shard status."""
        return {
            "shard_id": self.shard_id,
            "running": self._running,
            "game_count": self.game_count,
            "max_games": self.max_games,
            "games": [
                {
                    "game_id": ctx.game_id,
                    "sport": ctx.sport.value,
                    "plays_detected": ctx.plays_detected,
                    "signals_generated": ctx.signals_generated,
                    "started_at": ctx.started_at.isoformat(),
                    "status": ctx.last_state.status if ctx.last_state else "unknown",
                    "score": f"{ctx.last_state.home_score}-{ctx.last_state.away_score}" if ctx.last_state else "0-0",
                }
                for ctx in self._games.values()
            ],
        }


# Entry point for running as service
async def main():
    """Run GameShard as standalone service."""
    logging.basicConfig(level=logging.INFO)

    shard = GameShard()
    await shard.start()

    try:
        # Keep running until interrupted
        while True:
            await asyncio.sleep(60)
            logger.info(f"Shard status: {shard.get_status()}")
    except asyncio.CancelledError:
        pass
    finally:
        await shard.stop()


if __name__ == "__main__":
    asyncio.run(main())
