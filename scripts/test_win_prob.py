#!/usr/bin/env python3
"""
Win Probability Sanity Harness

Tests the win probability calculation using the same Rust call path as production.
Validates basic invariants and catches obviously broken model inputs.

Usage:
    python scripts/test_win_prob.py

    # Run with live games
    python scripts/test_win_prob.py --live
"""

import argparse
import asyncio
import logging
import sys
from dataclasses import dataclass
from typing import Optional

import arbees_core
from arbees_shared.models.game import GameState, Sport

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger(__name__)


@dataclass
class TestCase:
    """A test case for win probability calculation."""
    name: str
    sport: Sport
    home_team: str
    away_team: str
    home_score: int
    away_score: int
    period: int
    time_remaining_seconds: int
    # Expected properties
    expect_home_favored: Optional[bool] = None  # True if home should have > 50%
    expect_min_prob: Optional[float] = None
    expect_max_prob: Optional[float] = None


# Test cases covering various scenarios
TEST_CASES = [
    # NBA tests
    TestCase(
        name="NBA: Tied game Q1",
        sport=Sport.NBA,
        home_team="Boston Celtics",
        away_team="Los Angeles Lakers",
        home_score=24,
        away_score=24,
        period=1,
        time_remaining_seconds=180,
        expect_home_favored=True,  # Home court advantage
        expect_min_prob=0.45,
        expect_max_prob=0.60,
    ),
    TestCase(
        name="NBA: Home winning big late Q4",
        sport=Sport.NBA,
        home_team="Boston Celtics",
        away_team="Los Angeles Lakers",
        home_score=115,
        away_score=95,
        period=4,
        time_remaining_seconds=60,
        expect_home_favored=True,
        expect_min_prob=0.95,
    ),
    TestCase(
        name="NBA: Home losing big late Q4",
        sport=Sport.NBA,
        home_team="Boston Celtics",
        away_team="Los Angeles Lakers",
        home_score=85,
        away_score=110,
        period=4,
        time_remaining_seconds=60,
        expect_home_favored=False,
        expect_max_prob=0.05,
    ),
    TestCase(
        name="NBA: Close game final minute Q4",
        sport=Sport.NBA,
        home_team="Boston Celtics",
        away_team="Los Angeles Lakers",
        home_score=105,
        away_score=103,
        period=4,
        time_remaining_seconds=30,
        expect_home_favored=True,
        expect_min_prob=0.60,
        expect_max_prob=0.90,
    ),
    # NFL tests
    TestCase(
        name="NFL: Tied game Q2",
        sport=Sport.NFL,
        home_team="Kansas City Chiefs",
        away_team="San Francisco 49ers",
        home_score=14,
        away_score=14,
        period=2,
        time_remaining_seconds=600,
        expect_home_favored=True,
        expect_min_prob=0.45,
        expect_max_prob=0.60,
    ),
    TestCase(
        name="NFL: Home up 2 TDs Q4",
        sport=Sport.NFL,
        home_team="Kansas City Chiefs",
        away_team="San Francisco 49ers",
        home_score=28,
        away_score=14,
        period=4,
        time_remaining_seconds=300,
        expect_home_favored=True,
        expect_min_prob=0.85,
    ),
    # NHL tests
    TestCase(
        name="NHL: Tied game 2nd period",
        sport=Sport.NHL,
        home_team="Edmonton Oilers",
        away_team="Florida Panthers",
        home_score=2,
        away_score=2,
        period=2,
        time_remaining_seconds=900,
        expect_home_favored=True,
        expect_min_prob=0.45,
        expect_max_prob=0.60,
    ),
    TestCase(
        name="NHL: Home up 2 goals 3rd period",
        sport=Sport.NHL,
        home_team="Edmonton Oilers",
        away_team="Florida Panthers",
        home_score=4,
        away_score=2,
        period=3,
        time_remaining_seconds=300,
        expect_home_favored=True,
        expect_min_prob=0.80,
    ),
    # MLB tests
    TestCase(
        name="MLB: Tied game 5th inning",
        sport=Sport.MLB,
        home_team="New York Yankees",
        away_team="Boston Red Sox",
        home_score=3,
        away_score=3,
        period=5,
        time_remaining_seconds=0,  # MLB doesn't use time
        expect_home_favored=True,
        expect_min_prob=0.45,
        expect_max_prob=0.60,
    ),
    # NCAAB tests
    TestCase(
        name="NCAAB: Home winning by 10 late",
        sport=Sport.NCAAB,
        home_team="Duke Blue Devils",
        away_team="North Carolina Tar Heels",
        home_score=72,
        away_score=62,
        period=2,
        time_remaining_seconds=120,
        expect_home_favored=True,
        expect_min_prob=0.85,
    ),
]


def calculate_win_prob_rust(state: GameState) -> float:
    """
    Calculate win probability using the same Rust call path as GameShard.
    
    This mirrors the exact logic in services/game_shard/shard.py::_calculate_win_prob
    """
    try:
        # Map Python Sport to Rust Sport
        rust_sport = getattr(arbees_core.Sport, state.sport.value.upper(), None)
        if not rust_sport:
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
            rust_sport = sport_map.get(state.sport.value.lower())

        if not rust_sport:
            logger.warning(f"Unsupported sport for win prob: {state.sport}")
            return 0.5

        # Create Rust GameState
        rust_state = arbees_core.GameState(
            state.game_id,
            rust_sport,
            state.home_team,
            state.away_team,
            state.home_score,
            state.away_score,
            state.period,
            state.time_remaining_seconds,
        )

        # Calculate probability (for home team)
        raw_prob = arbees_core.calculate_win_probability(rust_state, True)

        # Clamp probability to [0.05, 0.95] as done in production
        return max(0.05, min(0.95, raw_prob))

    except Exception as e:
        logger.error(f"Error calculating win prob with Rust core: {e}")
        return 0.5


def run_test_case(test: TestCase) -> tuple[bool, str]:
    """
    Run a single test case and return (passed, message).
    """
    # Create GameState from test case
    state = GameState(
        game_id=f"test-{test.name.replace(' ', '-').lower()}",
        sport=test.sport,
        home_team=test.home_team,
        away_team=test.away_team,
        home_score=test.home_score,
        away_score=test.away_score,
        period=test.period,
        time_remaining=f"{test.time_remaining_seconds // 60}:{test.time_remaining_seconds % 60:02d}",
        time_remaining_seconds=test.time_remaining_seconds,
        status="in_progress",
    )

    # Calculate probability
    prob = calculate_win_prob_rust(state)

    # Validate invariants
    errors = []

    # Basic range check
    if not (0.0 <= prob <= 1.0):
        errors.append(f"Probability {prob:.4f} is out of [0, 1] range")

    # Clamped range check (production uses 0.05-0.95)
    if not (0.05 <= prob <= 0.95):
        errors.append(f"Probability {prob:.4f} is outside clamped range [0.05, 0.95]")

    # Home favored check
    if test.expect_home_favored is not None:
        if test.expect_home_favored and prob <= 0.5:
            errors.append(f"Expected home favored (>50%) but got {prob*100:.1f}%")
        elif not test.expect_home_favored and prob >= 0.5:
            errors.append(f"Expected home NOT favored (<50%) but got {prob*100:.1f}%")

    # Min/max probability checks
    if test.expect_min_prob is not None and prob < test.expect_min_prob:
        errors.append(f"Probability {prob*100:.1f}% below expected min {test.expect_min_prob*100:.1f}%")
    if test.expect_max_prob is not None and prob > test.expect_max_prob:
        errors.append(f"Probability {prob*100:.1f}% above expected max {test.expect_max_prob*100:.1f}%")

    if errors:
        return False, f"FAIL: {test.name} - prob={prob*100:.1f}% - {'; '.join(errors)}"
    else:
        return True, f"PASS: {test.name} - prob={prob*100:.1f}%"


async def run_live_tests():
    """Run tests against actual live games."""
    from data_providers.espn.client import ESPNClient

    logger.info("Fetching live games for testing...")
    
    sports_to_check = [Sport.NBA, Sport.NHL, Sport.NCAAB, Sport.NFL]
    games_tested = 0
    errors = []

    for sport in sports_to_check:
        try:
            async with ESPNClient(sport=sport) as client:
                live_games = await client.get_live_games()

                for game_info in live_games[:3]:  # Limit to 3 per sport
                    state = await client.get_game_state(game_info.game_id)
                    if not state:
                        continue

                    prob = calculate_win_prob_rust(state)
                    games_tested += 1

                    # Basic invariant checks
                    if not (0.05 <= prob <= 0.95):
                        errors.append(
                            f"{sport.value} {state.home_team} vs {state.away_team}: "
                            f"prob={prob*100:.1f}% outside clamped range"
                        )

                    # Late game consistency check
                    if state.game_progress and state.game_progress > 0.9:
                        score_diff = state.home_score - state.away_score
                        if score_diff > 10 and prob < 0.7:
                            errors.append(
                                f"{sport.value} {state.home_team} vs {state.away_team}: "
                                f"home winning by {score_diff} late but prob={prob*100:.1f}%"
                            )
                        elif score_diff < -10 and prob > 0.3:
                            errors.append(
                                f"{sport.value} {state.home_team} vs {state.away_team}: "
                                f"home losing by {-score_diff} late but prob={prob*100:.1f}%"
                            )

                    logger.info(
                        f"[{sport.value}] {state.home_team} {state.home_score} - "
                        f"{state.away_score} {state.away_team}: {prob*100:.1f}% home"
                    )

        except Exception as e:
            logger.warning(f"Error checking {sport.value}: {e}")

    return games_tested, errors


def main():
    parser = argparse.ArgumentParser(description="Win Probability Sanity Harness")
    parser.add_argument("--live", action="store_true", help="Test against live games")
    args = parser.parse_args()

    print("=" * 60)
    print("Win Probability Sanity Harness")
    print("=" * 60)
    print()

    # Run static test cases
    print("Running static test cases...")
    print("-" * 40)

    passed = 0
    failed = 0

    for test in TEST_CASES:
        success, message = run_test_case(test)
        if success:
            passed += 1
            print(f"  {message}")
        else:
            failed += 1
            print(f"  {message}")

    print()
    print(f"Static tests: {passed} passed, {failed} failed")
    print()

    # Run live tests if requested
    if args.live:
        print("Running live game tests...")
        print("-" * 40)

        games_tested, errors = asyncio.run(run_live_tests())

        print()
        print(f"Live tests: {games_tested} games tested, {len(errors)} issues found")

        if errors:
            print()
            print("Issues found:")
            for error in errors:
                print(f"  - {error}")

        if errors:
            failed += len(errors)

    print()
    print("=" * 60)
    if failed == 0:
        print("ALL TESTS PASSED")
        sys.exit(0)
    else:
        print(f"TESTS FAILED: {failed} issues")
        sys.exit(1)


if __name__ == "__main__":
    main()
