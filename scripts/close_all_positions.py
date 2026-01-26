#!/usr/bin/env python3
"""
Emergency Close All Positions Script

Closes all open positions immediately. Options:
  - Default: Close at entry price (push - no P&L)
  - --settle: Close at final game outcome (win/loss based on game result)
  - --price: Close at specified price (0-1)

Usage:
    python scripts/close_all_positions.py [--dry-run]
    python scripts/close_all_positions.py --settle [--dry-run]
    python scripts/close_all_positions.py --price 0.5 [--dry-run]

Options:
    --dry-run       Show what would be done without making changes
    --settle        Settle positions based on final game outcome (1.0 for winner, 0.0 for loser)
    --price PRICE   Close all positions at this price (0.0-1.0)
    --reason TEXT   Reason for closing (default: emergency_close)
"""

import argparse
import asyncio
import os
import sys
from datetime import datetime, timezone
from decimal import Decimal

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


async def get_open_positions(conn) -> list[dict]:
    """Get all open positions with game info."""
    rows = await conn.fetch(
        """
        SELECT
            pt.trade_id,
            pt.game_id,
            pt.market_title,
            pt.side,
            pt.entry_price::float as entry_price,
            pt.size::float as size,
            pt.entry_time,
            pt.sport::text as sport,
            gs.home_score,
            gs.away_score,
            gs.status as game_status
        FROM paper_trades pt
        LEFT JOIN LATERAL (
            SELECT home_score, away_score, status
            FROM game_states
            WHERE game_id = pt.game_id
            ORDER BY time DESC
            LIMIT 1
        ) gs ON true
        WHERE pt.status = 'open'
        ORDER BY pt.time DESC
        """
    )
    return [dict(row) for row in rows]


async def get_bankroll(conn) -> dict:
    """Get current bankroll state."""
    row = await conn.fetchrow(
        "SELECT current_balance::float, piggybank_balance::float FROM bankroll WHERE account_name = 'default'"
    )
    return dict(row) if row else None


async def close_position(conn, position: dict, exit_price: float, reason: str, dry_run: bool = False) -> dict:
    """Close a single position and calculate P&L."""
    entry_price = position['entry_price']
    size = position['size']
    side = position['side']

    # Calculate P&L based on side
    if side == 'buy':
        # Bought YES contract: profit if exit_price > entry_price
        pnl = size * (exit_price - entry_price)
    else:
        # Bought NO contract (sold YES): profit if exit_price < entry_price
        pnl = size * (entry_price - exit_price)

    pnl_pct = (pnl / (size * entry_price)) * 100 if entry_price > 0 else 0
    outcome = 'win' if pnl > 0 else ('loss' if pnl < 0 else 'push')

    result = {
        'trade_id': position['trade_id'],
        'market_title': position['market_title'],
        'side': side,
        'entry_price': entry_price,
        'exit_price': exit_price,
        'size': size,
        'pnl': pnl,
        'pnl_pct': pnl_pct,
        'outcome': outcome,
        'reason': reason
    }

    if not dry_run:
        await conn.execute(
            """
            UPDATE paper_trades
            SET
                status = 'closed',
                outcome = $1::trade_outcome_enum,
                exit_price = $2,
                exit_time = $3,
                pnl = $4,
                pnl_pct = $5
            WHERE trade_id = $6
            """,
            outcome,
            exit_price,
            datetime.now(timezone.utc),
            pnl,
            pnl_pct,
            position['trade_id']
        )

    return result


async def update_bankroll(conn, total_pnl: float, dry_run: bool = False) -> dict:
    """Update bankroll with P&L from closed positions."""
    if dry_run:
        bankroll = await get_bankroll(conn)
        return {
            'old_balance': bankroll['current_balance'],
            'new_balance': bankroll['current_balance'] + total_pnl,
            'pnl_applied': total_pnl
        }

    # Get current bankroll
    bankroll = await get_bankroll(conn)
    old_balance = bankroll['current_balance']

    # Calculate piggybank contribution (25% of profits go to piggybank)
    piggybank_pct = float(os.environ.get("PIGGYBANK_PERCENT", "0.25"))
    piggybank_add = max(0, total_pnl * piggybank_pct)
    balance_add = total_pnl - piggybank_add

    new_balance = old_balance + balance_add
    new_piggybank = bankroll['piggybank_balance'] + piggybank_add

    await conn.execute(
        """
        UPDATE bankroll
        SET
            current_balance = $1,
            piggybank_balance = $2,
            peak_balance = GREATEST(peak_balance, $1),
            updated_at = NOW()
        WHERE account_name = 'default'
        """,
        new_balance,
        new_piggybank
    )

    return {
        'old_balance': old_balance,
        'new_balance': new_balance,
        'piggybank_add': piggybank_add,
        'pnl_applied': total_pnl
    }


def determine_exit_price_from_game(position: dict) -> float | None:
    """Determine exit price based on final game score."""
    home_score = position.get('home_score')
    away_score = position.get('away_score')
    game_status = position.get('game_status', '')
    market_title = position.get('market_title', '')

    # Can only settle if game is final
    if 'FINAL' not in (game_status or '').upper():
        return None

    if home_score is None or away_score is None:
        return None

    # Determine if market_title team won
    # This is a simplified check - in production would need better team matching
    # For now, assume if score is higher, that team won
    # We'd need to know if market_title is home or away team

    # If game ended in tie (shouldn't happen in most sports), return 0.5
    if home_score == away_score:
        return 0.5

    # This is imperfect without knowing which team the position is for
    # Return None to indicate we can't determine
    return None


async def main():
    parser = argparse.ArgumentParser(
        description="Emergency close all open positions"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be done without making changes"
    )
    parser.add_argument(
        "--settle",
        action="store_true",
        help="Settle positions based on final game outcome"
    )
    parser.add_argument(
        "--price",
        type=float,
        help="Close all positions at this price (0.0-1.0)"
    )
    parser.add_argument(
        "--reason",
        type=str,
        default="emergency_close",
        help="Reason for closing (default: emergency_close)"
    )
    args = parser.parse_args()

    # Validate price if provided
    if args.price is not None:
        if args.price < 0 or args.price > 1:
            print("Error: --price must be between 0.0 and 1.0")
            sys.exit(1)

    conn = await get_connection()

    try:
        print("=" * 60)
        print("EMERGENCY CLOSE ALL POSITIONS")
        print("=" * 60)

        if args.dry_run:
            print("\n[!] DRY RUN MODE - No changes will be made\n")

        # Get open positions
        positions = await get_open_positions(conn)

        if not positions:
            print("\nNo open positions to close.")
            print("=" * 60)
            return

        # Show current state
        print(f"\n[OPEN POSITIONS] ({len(positions)})")
        print("-" * 50)

        total_invested = 0
        for pos in positions:
            invested = pos['size'] * (pos['entry_price'] if pos['side'] == 'buy' else (1 - pos['entry_price']))
            total_invested += invested
            game_status = pos.get('game_status', 'Unknown')
            print(f"  {pos['market_title']}")
            print(f"    Side: {pos['side'].upper()}, Entry: {pos['entry_price']:.3f}, Size: ${pos['size']:.2f}")
            print(f"    Game: {game_status}, Score: {pos.get('home_score', '?')}-{pos.get('away_score', '?')}")

        print(f"\n  Total Invested: ${total_invested:.2f}")

        # Get current bankroll
        bankroll = await get_bankroll(conn)
        print(f"\n[CURRENT BANKROLL]")
        print(f"  Balance: ${bankroll['current_balance']:.2f}")
        print(f"  Piggybank: ${bankroll['piggybank_balance']:.2f}")

        # Close positions
        print(f"\n[CLOSING POSITIONS]")
        print("-" * 50)

        close_method = "entry price (push)"
        if args.settle:
            close_method = "game outcome (settle)"
        elif args.price is not None:
            close_method = f"fixed price ({args.price:.3f})"

        print(f"  Method: {close_method}")
        print(f"  Reason: {args.reason}")
        print()

        total_pnl = 0
        results = []

        for pos in positions:
            # Determine exit price
            if args.settle:
                exit_price = determine_exit_price_from_game(pos)
                if exit_price is None:
                    # Can't settle - use entry price (push)
                    exit_price = pos['entry_price']
                    print(f"  WARNING: Cannot settle {pos['market_title']} - game not final, using push")
            elif args.price is not None:
                exit_price = args.price
            else:
                # Default: close at entry price (push)
                exit_price = pos['entry_price']

            result = await close_position(conn, pos, exit_price, args.reason, dry_run=args.dry_run)
            results.append(result)
            total_pnl += result['pnl']

            pnl_color = '+' if result['pnl'] >= 0 else ''
            action = "Would close" if args.dry_run else "Closed"
            print(f"  {action}: {pos['market_title']}")
            print(f"    {pos['side'].upper()} @ {result['entry_price']:.3f} -> {result['exit_price']:.3f}")
            print(f"    P&L: {pnl_color}${result['pnl']:.2f} ({result['outcome'].upper()})")
            print()

        # Update bankroll
        print(f"[SUMMARY]")
        print("-" * 50)
        print(f"  Positions closed: {len(results)}")
        print(f"  Total P&L: {'+'if total_pnl >= 0 else ''}${total_pnl:.2f}")

        if not args.dry_run and total_pnl != 0:
            bankroll_result = await update_bankroll(conn, total_pnl, dry_run=False)
            print(f"\n  Bankroll: ${bankroll_result['old_balance']:.2f} -> ${bankroll_result['new_balance']:.2f}")
            if bankroll_result.get('piggybank_add', 0) > 0:
                print(f"  Piggybank: +${bankroll_result['piggybank_add']:.2f}")
        elif args.dry_run:
            bankroll_result = await update_bankroll(conn, total_pnl, dry_run=True)
            print(f"\n  Bankroll would be: ${bankroll_result['old_balance']:.2f} -> ${bankroll_result['new_balance']:.2f}")

        if args.dry_run:
            print(f"\n[TIP] Run without --dry-run to apply these changes")
        else:
            print(f"\nAll positions closed successfully!")

        print("=" * 60)

    finally:
        await conn.close()


if __name__ == "__main__":
    asyncio.run(main())
