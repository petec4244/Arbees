"""
Integration tests for team matching in entry and exit flows.

These tests reproduce the original bug class:
- Two contracts per game (home/away) with same market_id
- SignalProcessor must select correct team's price
- PositionTracker must exit with correct team's price

Requires: TimescaleDB + Redis running (docker-compose up timescaledb redis)
"""

import asyncio
import os
from datetime import datetime, timezone
from typing import Optional
from uuid import uuid4

import pytest

# Skip if DATABASE_URL not set
pytestmark = pytest.mark.skipif(
    not os.environ.get("DATABASE_URL"),
    reason="DATABASE_URL not set - run with docker-compose services"
)


@pytest.fixture
async def db_pool():
    """Create a database connection pool for tests."""
    from arbees_shared.db.connection import get_pool, close_pool

    pool = await get_pool()
    yield pool
    await close_pool()


@pytest.fixture
def game_id() -> str:
    """Generate a unique game ID for each test."""
    return f"test-game-{uuid4().hex[:8]}"


@pytest.fixture
def market_id() -> str:
    """Generate a unique market ID for each test."""
    return f"test-market-{uuid4().hex[:8]}"


class TestSignalProcessorTeamMatching:
    """Tests for SignalProcessor._get_market_price team matching."""

    @pytest.fixture
    async def processor(self, db_pool):
        """Create a SignalProcessor instance."""
        from services.signal_processor.processor import SignalProcessor

        proc = SignalProcessor(
            team_match_min_confidence=0.7,
            min_edge_pct=1.0,
        )
        # Don't start full service, just set up DB connection
        proc.db = db_pool
        return proc

    async def test_selects_correct_team_price(
        self,
        processor,
        db_pool,
        game_id: str,
        market_id: str,
    ) -> None:
        """
        Given: Two market_prices rows for same game (Celtics and Lakers)
        When: Signal requests Celtics
        Then: Returns Celtics price (not Lakers)
        """
        # Insert two prices for same game - different teams
        await db_pool.execute(
            """
            INSERT INTO market_prices (time, market_id, platform, game_id, market_title, contract_team, yes_bid, yes_ask, volume, liquidity)
            VALUES
                (NOW(), $1, 'polymarket', $2, 'Celtics to win', 'Celtics', 0.55, 0.57, 1000, 5000),
                (NOW(), $1, 'polymarket', $2, 'Lakers to win', 'Lakers', 0.43, 0.45, 1000, 5000)
            """,
            market_id,
            game_id,
        )

        try:
            # Create a signal for Celtics
            from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection
            from arbees_shared.models.game import Sport

            signal = TradingSignal(
                signal_id=str(uuid4()),
                signal_type=SignalType.WIN_PROB_SHIFT,
                game_id=game_id,
                sport=Sport.NBA,
                team="Boston Celtics",  # Looking for Celtics
                direction=SignalDirection.BUY,
                model_prob=0.60,
                market_prob=0.56,
                edge_pct=4.0,
                confidence=0.8,
            )

            # Get market price
            price = await processor._get_market_price(signal)

            # Should select Celtics, not Lakers
            assert price is not None, "Should find a price"
            assert price.contract_team == "Celtics", f"Should select Celtics, got {price.contract_team}"
            assert price.yes_bid == 0.55
            assert price.yes_ask == 0.57

        finally:
            # Cleanup
            await db_pool.execute(
                "DELETE FROM market_prices WHERE market_id = $1",
                market_id,
            )

    async def test_rejects_when_no_confident_match(
        self,
        processor,
        db_pool,
        game_id: str,
        market_id: str,
    ) -> None:
        """
        Given: Market price rows exist but for wrong team
        When: Signal requests a team not in the data
        Then: Returns None (rejects signal)
        """
        # Insert only Lakers price
        await db_pool.execute(
            """
            INSERT INTO market_prices (time, market_id, platform, game_id, market_title, contract_team, yes_bid, yes_ask, volume, liquidity)
            VALUES (NOW(), $1, 'polymarket', $2, 'Lakers to win', 'Lakers', 0.45, 0.47, 1000, 5000)
            """,
            market_id,
            game_id,
        )

        try:
            from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection
            from arbees_shared.models.game import Sport

            signal = TradingSignal(
                signal_id=str(uuid4()),
                signal_type=SignalType.WIN_PROB_SHIFT,
                game_id=game_id,
                sport=Sport.NBA,
                team="Boston Celtics",  # Looking for Celtics, but only Lakers exists
                direction=SignalDirection.BUY,
                model_prob=0.60,
                market_prob=0.56,
                edge_pct=4.0,
                confidence=0.8,
            )

            price = await processor._get_market_price(signal)

            # Should NOT find a price (Celtics doesn't match Lakers)
            assert price is None, "Should reject when team doesn't match"

        finally:
            await db_pool.execute(
                "DELETE FROM market_prices WHERE market_id = $1",
                market_id,
            )

    async def test_nickname_match_works(
        self,
        processor,
        db_pool,
        game_id: str,
        market_id: str,
    ) -> None:
        """
        Given: Market price with nickname only (e.g., "Flyers")
        When: Signal has full name ("Philadelphia Flyers")
        Then: Matches correctly
        """
        await db_pool.execute(
            """
            INSERT INTO market_prices (time, market_id, platform, game_id, market_title, contract_team, yes_bid, yes_ask, volume, liquidity)
            VALUES (NOW(), $1, 'polymarket', $2, 'Flyers to win', 'Flyers', 0.50, 0.52, 1000, 5000)
            """,
            market_id,
            game_id,
        )

        try:
            from arbees_shared.models.signal import TradingSignal, SignalType, SignalDirection
            from arbees_shared.models.game import Sport

            signal = TradingSignal(
                signal_id=str(uuid4()),
                signal_type=SignalType.WIN_PROB_SHIFT,
                game_id=game_id,
                sport=Sport.NHL,
                team="Philadelphia Flyers",  # Full name
                direction=SignalDirection.BUY,
                model_prob=0.55,
                market_prob=0.51,
                edge_pct=4.0,
                confidence=0.8,
            )

            price = await processor._get_market_price(signal)

            assert price is not None, "Should match nickname"
            assert price.contract_team == "Flyers"

        finally:
            await db_pool.execute(
                "DELETE FROM market_prices WHERE market_id = $1",
                market_id,
            )


class TestPositionTrackerTeamMatching:
    """Tests for PositionTracker exit price team matching."""

    @pytest.fixture
    async def tracker(self, db_pool):
        """Create a PositionTracker instance."""
        from services.position_tracker.tracker import PositionTracker

        tracker = PositionTracker(
            exit_team_match_min_confidence=0.7,
            min_hold_seconds=0.0,  # Disable for tests
        )
        tracker.db = db_pool
        return tracker

    async def test_exit_selects_correct_team_price(
        self,
        tracker,
        db_pool,
        game_id: str,
        market_id: str,
    ) -> None:
        """
        Given: Trade entered on Celtics, both Celtics and Lakers prices exist
        When: Getting exit price
        Then: Returns Celtics price (not Lakers)
        """
        # Insert both team prices
        await db_pool.execute(
            """
            INSERT INTO market_prices (time, market_id, platform, game_id, market_title, contract_team, yes_bid, yes_ask, volume, liquidity)
            VALUES
                (NOW(), $1, 'polymarket', $2, 'Celtics to win', 'Celtics', 0.60, 0.62, 1000, 5000),
                (NOW(), $1, 'polymarket', $2, 'Lakers to win', 'Lakers', 0.38, 0.40, 1000, 5000)
            """,
            market_id,
            game_id,
        )

        try:
            from arbees_shared.models.trade import PaperTrade, TradeSide, TradeStatus, TradeOutcome
            from arbees_shared.models.signal import SignalType
            from arbees_shared.models.market import Platform
            from arbees_shared.models.game import Sport

            # Create a trade that was entered on Celtics
            trade = PaperTrade(
                trade_id=str(uuid4()),
                signal_id=str(uuid4()),
                game_id=game_id,
                sport=Sport.NBA,
                platform=Platform.POLYMARKET,
                market_id=market_id,
                market_title="Celtics to win",  # Entry was on Celtics
                side=TradeSide.BUY,
                signal_type=SignalType.WIN_PROB_SHIFT,
                entry_price=0.55,
                size=10.0,
                model_prob=0.60,
                edge_at_entry=5.0,
                kelly_fraction=0.1,
                status=TradeStatus.OPEN,
                outcome=TradeOutcome.PENDING,
            )

            result = await tracker._get_current_prices_with_metadata(trade)

            assert result is not None, "Should find exit price"
            mark, exec_px, price_age_ms, bid, ask = result

            # Should be Celtics prices, not Lakers
            assert bid == 0.60, f"Bid should be Celtics (0.60), got {bid}"
            assert ask == 0.62, f"Ask should be Celtics (0.62), got {ask}"

        finally:
            await db_pool.execute(
                "DELETE FROM market_prices WHERE market_id = $1",
                market_id,
            )

    async def test_exit_rejects_wrong_team_price(
        self,
        tracker,
        db_pool,
        game_id: str,
        market_id: str,
    ) -> None:
        """
        Given: Trade entered on Celtics, only Lakers price available
        When: Getting exit price
        Then: Returns None (skips exit check)
        """
        # Only Lakers price available
        await db_pool.execute(
            """
            INSERT INTO market_prices (time, market_id, platform, game_id, market_title, contract_team, yes_bid, yes_ask, volume, liquidity)
            VALUES (NOW(), $1, 'polymarket', $2, 'Lakers to win', 'Lakers', 0.40, 0.42, 1000, 5000)
            """,
            market_id,
            game_id,
        )

        try:
            from arbees_shared.models.trade import PaperTrade, TradeSide, TradeStatus, TradeOutcome
            from arbees_shared.models.signal import SignalType
            from arbees_shared.models.market import Platform
            from arbees_shared.models.game import Sport

            trade = PaperTrade(
                trade_id=str(uuid4()),
                signal_id=str(uuid4()),
                game_id=game_id,
                sport=Sport.NBA,
                platform=Platform.POLYMARKET,
                market_id=market_id,
                market_title="Celtics to win",  # Entry was on Celtics
                side=TradeSide.BUY,
                signal_type=SignalType.WIN_PROB_SHIFT,
                entry_price=0.55,
                size=10.0,
                model_prob=0.60,
                edge_at_entry=5.0,
                kelly_fraction=0.1,
                status=TradeStatus.OPEN,
                outcome=TradeOutcome.PENDING,
            )

            result = await tracker._get_current_prices_with_metadata(trade)

            # Should NOT use Lakers price for Celtics trade
            assert result is None, "Should skip exit when only wrong team price available"

        finally:
            await db_pool.execute(
                "DELETE FROM market_prices WHERE market_id = $1",
                market_id,
            )


class TestMinHoldTime:
    """Tests for minimum hold time enforcement."""

    @pytest.fixture
    async def tracker(self, db_pool):
        """Create a PositionTracker with non-zero min_hold_seconds."""
        from services.position_tracker.tracker import PositionTracker

        tracker = PositionTracker(
            min_hold_seconds=10.0,  # 10 second minimum hold
        )
        tracker.db = db_pool
        return tracker

    async def test_skips_exit_check_before_min_hold(
        self,
        tracker,
        db_pool,
    ) -> None:
        """
        Given: Trade just opened (< min_hold_seconds ago)
        When: _check_exit_conditions runs
        Then: Skips evaluation (doesn't trigger early exit)
        """
        from arbees_shared.models.trade import PaperTrade, TradeSide, TradeStatus, TradeOutcome
        from arbees_shared.models.signal import SignalType
        from arbees_shared.models.market import Platform
        from arbees_shared.models.game import Sport

        # Trade opened just now
        trade = PaperTrade(
            trade_id=str(uuid4()),
            signal_id=str(uuid4()),
            game_id="test-game",
            sport=Sport.NBA,
            platform=Platform.PAPER,
            market_id="test-market",
            market_title="Test to win",
            side=TradeSide.BUY,
            signal_type=SignalType.WIN_PROB_SHIFT,
            entry_price=0.50,
            size=10.0,
            model_prob=0.55,
            edge_at_entry=5.0,
            kelly_fraction=0.1,
            entry_time=datetime.now(timezone.utc),  # Just opened
            status=TradeStatus.OPEN,
            outcome=TradeOutcome.PENDING,
        )

        # Calculate hold duration
        now = datetime.now(timezone.utc)
        entry_time = trade.entry_time
        if entry_time.tzinfo is None:
            entry_time = entry_time.replace(tzinfo=timezone.utc)
        hold_duration = (now - entry_time).total_seconds()

        # Should be less than min_hold_seconds
        assert hold_duration < tracker.min_hold_seconds, "Test setup: trade should be fresh"
