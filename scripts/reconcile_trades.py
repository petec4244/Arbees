#!/usr/bin/env python3
"""
Trade Reconciliation Script

Cross-reference paper_trades with actual game outcomes to verify P&L accuracy.

Usage:
    python scripts/reconcile_trades.py [--verbose] [--limit N]

Options:
    --verbose    Show detailed info for each trade
    --limit N    Only check the last N trades (default: all)
"""

import argparse
import asyncio
import os
import sys
from datetime import datetime, timezone
from typing import Optional

# Fix Windows console encoding
if sys.platform == "win32":
    sys.stdout.reconfigure(encoding='utf-8', errors='replace')

# Add parent directory to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import asyncpg


async def get_connection():
    """Get database connection."""
    database_url = os.environ.get(
        "DATABASE_URL",
        "postgresql://arbees:3f0a769149d87a06fec136565f3e3ee88297ff183f725e9c@localhost:5432/arbees"
    )
    return await asyncpg.connect(database_url)


def extract_team_from_title(title: str) -> Optional[str]:
    """Extract team name from market title.

    Handles formats:
    - "Game Title [Team Name]"
    - "Team (inverted from Other Team)"
    - "Team Name to win"
    - "Team A vs. Team B" - returns None (ambiguous)
    """
    if not title:
        return None

    # Format: "Game Title [Team Name]"
    if "[" in title and "]" in title:
        try:
            return title.rsplit("[", 1)[-1].split("]", 1)[0].strip()
        except Exception:
            pass

    # Format: "Team (inverted from Other Team)"
    if " (inverted from " in title:
        return title.split(" (inverted from ", 1)[0].strip()

    # Format: "Team Name to win" (Kalshi style)
    if " to win" in title.lower():
        return title.lower().replace(" to win", "").strip()

    return None


def teams_match(team1: Optional[str], team2: Optional[str]) -> bool:
    """Check if two team names match (case-insensitive, partial)."""
    if not team1 or not team2:
        return False
    t1 = team1.lower().strip()
    t2 = team2.lower().strip()
    return t1 in t2 or t2 in t1


async def get_closed_trades(conn, limit: Optional[int] = None) -> list[dict]:
    """Get all closed paper trades."""
    query = """
        SELECT
            trade_id, game_id, platform, market_id, market_title,
            side, entry_price, exit_price, size, pnl, pnl_pct,
            outcome, entry_time, exit_time
        FROM paper_trades
        WHERE status = 'closed'
        ORDER BY exit_time DESC
    """
    if limit:
        query += f" LIMIT {limit}"

    rows = await conn.fetch(query)
    return [dict(row) for row in rows]


async def get_game_result(conn, game_id: str) -> Optional[dict]:
    """Get final game result.

    First checks games table, then falls back to game_states for actual scores
    since games.final_home_score may not be populated.
    """
    # Get basic game info
    row = await conn.fetchrow(
        """
        SELECT
            game_id, home_team, away_team,
            final_home_score, final_away_score, status
        FROM games
        WHERE game_id = $1
        """,
        game_id
    )
    if not row:
        return None

    result = dict(row)

    # If final scores are NULL, get from game_states
    if result["final_home_score"] is None or result["final_away_score"] is None:
        state_row = await conn.fetchrow(
            """
            SELECT home_score, away_score, status
            FROM game_states
            WHERE game_id = $1
            ORDER BY time DESC
            LIMIT 1
            """,
            game_id
        )
        if state_row:
            result["final_home_score"] = state_row["home_score"]
            result["final_away_score"] = state_row["away_score"]
            # Update status if game_states shows final
            if state_row["status"] in ("final", "complete", "closed"):
                result["status"] = state_row["status"]

    return result


def determine_expected_outcome(
    trade_team: Optional[str],
    trade_side: str,
    home_team: str,
    away_team: str,
    home_score: int,
    away_score: int
) -> tuple[Optional[float], str]:
    """
    Determine expected exit price based on actual game outcome.

    Returns:
        (expected_exit, reason)
        expected_exit: 1.0 if trade's team won, 0.0 if lost, None if can't determine
    """
    if trade_team is None:
        return None, "no_team_extracted"

    # Determine which team the trade was on
    is_home_bet = teams_match(trade_team, home_team)
    is_away_bet = teams_match(trade_team, away_team)

    if not is_home_bet and not is_away_bet:
        return None, f"team_not_matched: {trade_team}"

    # Determine winner
    if home_score == away_score:
        return 0.5, "game_tied"  # Push scenario

    home_won = home_score > away_score

    # For YES (buy) positions
    if trade_side == "yes":
        if is_home_bet:
            return 1.0 if home_won else 0.0, "home_bet"
        else:
            return 0.0 if home_won else 1.0, "away_bet"
    # For NO (sell) positions - inverted
    else:  # trade_side == "no"
        if is_home_bet:
            return 0.0 if home_won else 1.0, "home_bet_no"
        else:
            return 1.0 if home_won else 0.0, "away_bet_no"


async def reconcile_trades(conn, trades: list[dict], verbose: bool = False) -> dict:
    """Reconcile trades against actual game outcomes."""
    results = {
        "total": len(trades),
        "verified_correct": 0,
        "suspicious": [],
        "game_not_found": [],
        "could_not_determine": [],
    }

    for trade in trades:
        game_id = trade["game_id"]

        if not game_id:
            results["could_not_determine"].append({
                "trade": trade,
                "reason": "no_game_id"
            })
            continue

        # Get game result
        game = await get_game_result(conn, game_id)

        if not game:
            results["game_not_found"].append({
                "trade": trade,
                "reason": "game_not_in_db"
            })
            continue

        # Check if game is final
        game_status = game.get("status", "").lower()
        if game_status not in ("final", "complete", "closed"):
            results["could_not_determine"].append({
                "trade": trade,
                "game": game,
                "reason": f"game_not_final: {game_status}"
            })
            continue

        # Extract team from trade
        trade_team = extract_team_from_title(trade["market_title"])

        # Determine expected outcome
        expected_exit, reason = determine_expected_outcome(
            trade_team=trade_team,
            trade_side=trade["side"],
            home_team=game["home_team"],
            away_team=game["away_team"],
            home_score=game["final_home_score"] or 0,
            away_score=game["final_away_score"] or 0
        )

        if expected_exit is None:
            results["could_not_determine"].append({
                "trade": trade,
                "game": game,
                "reason": reason
            })
            continue

        # Compare actual exit price to expected
        actual_exit = float(trade["exit_price"]) if trade["exit_price"] else None

        if actual_exit is None:
            results["suspicious"].append({
                "trade": trade,
                "game": game,
                "expected_exit": expected_exit,
                "actual_exit": None,
                "reason": "no_exit_price",
                "match_reason": reason
            })
            continue

        # Allow some tolerance for mid-game exits (e.g., stop-loss, take-profit)
        # Suspicious if exit was close to binary (0 or 1) but wrong value
        is_binary_exit = actual_exit < 0.1 or actual_exit > 0.9

        if is_binary_exit:
            # For binary exits, check if it matches expected outcome
            actual_binary = 1.0 if actual_exit > 0.5 else 0.0
            if abs(actual_binary - expected_exit) > 0.01:
                results["suspicious"].append({
                    "trade": trade,
                    "game": game,
                    "expected_exit": expected_exit,
                    "actual_exit": actual_exit,
                    "reason": "binary_mismatch",
                    "match_reason": reason
                })
                continue

        # Trade looks correct or was a mid-game exit
        results["verified_correct"] += 1

        if verbose:
            print(f"  OK: {trade['trade_id'][:8]} | {trade['market_title'][:40]} | "
                  f"exit={actual_exit:.3f} expected={expected_exit:.1f}")

    return results


def print_report(results: dict):
    """Print reconciliation report."""
    print("\n" + "=" * 70)
    print("TRADE RECONCILIATION REPORT")
    print("=" * 70)

    total = results["total"]
    verified = results["verified_correct"]
    suspicious = len(results["suspicious"])
    not_found = len(results["game_not_found"])
    undetermined = len(results["could_not_determine"])

    print(f"\nTotal closed trades: {total}")
    print(f"Verified correct:    {verified} ({100*verified/total:.1f}%)" if total > 0 else "")
    print(f"Suspicious P&L:      {suspicious} ({100*suspicious/total:.1f}%)" if total > 0 else "")
    print(f"Game not found:      {not_found} ({100*not_found/total:.1f}%)" if total > 0 else "")
    print(f"Could not determine: {undetermined} ({100*undetermined/total:.1f}%)" if total > 0 else "")

    if results["suspicious"]:
        print("\n" + "-" * 70)
        print("SUSPICIOUS TRADES")
        print("-" * 70)
        for item in results["suspicious"]:
            trade = item["trade"]
            game = item.get("game", {})
            expected = item["expected_exit"]
            actual = item["actual_exit"]
            reason = item["reason"]

            home = game.get("home_team", "?")
            away = game.get("away_team", "?")
            h_score = game.get("final_home_score", "?")
            a_score = game.get("final_away_score", "?")

            print(f"\n  trade_id: {trade['trade_id']}")
            print(f"  title:    {trade['market_title']}")
            print(f"  side:     {trade['side']}")
            print(f"  game:     {away} @ {home} ({a_score}-{h_score})")
            print(f"  exit:     {actual:.3f if actual else 'None'} (expected: {expected:.1f})")
            print(f"  pnl:      ${float(trade['pnl'] or 0):.2f}")
            print(f"  reason:   {reason}")

    if results["game_not_found"]:
        print("\n" + "-" * 70)
        print("GAMES NOT FOUND")
        print("-" * 70)
        for item in results["game_not_found"][:10]:  # Show first 10
            trade = item["trade"]
            print(f"  {trade['trade_id'][:8]} | game_id={trade['game_id']} | {trade['market_title'][:40]}")
        if len(results["game_not_found"]) > 10:
            print(f"  ... and {len(results['game_not_found']) - 10} more")

    if results["could_not_determine"]:
        print("\n" + "-" * 70)
        print("COULD NOT DETERMINE (first 10)")
        print("-" * 70)
        for item in results["could_not_determine"][:10]:
            trade = item["trade"]
            reason = item["reason"]
            print(f"  {trade['trade_id'][:8]} | {reason} | {trade['market_title'][:40]}")
        if len(results["could_not_determine"]) > 10:
            print(f"  ... and {len(results['could_not_determine']) - 10} more")

    print("\n" + "=" * 70)


async def main():
    parser = argparse.ArgumentParser(
        description="Reconcile paper trades against actual game outcomes"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Show detailed info for each trade"
    )
    parser.add_argument(
        "--limit", "-n",
        type=int,
        default=None,
        help="Only check the last N trades"
    )
    args = parser.parse_args()

    conn = await get_connection()

    try:
        print("Fetching closed trades...")
        trades = await get_closed_trades(conn, limit=args.limit)
        print(f"Found {len(trades)} closed trades")

        if not trades:
            print("No closed trades to reconcile.")
            return

        print("Reconciling against game outcomes...")
        results = await reconcile_trades(conn, trades, verbose=args.verbose)

        print_report(results)

    finally:
        await conn.close()


if __name__ == "__main__":
    asyncio.run(main())
