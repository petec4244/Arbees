#!/usr/bin/env python3
"""
Reset Paper Trading Script

Force closes all open positions and resets bankroll while preserving trade history.

Usage:
    python scripts/reset_paper_trading.py [--bankroll 1000] [--dry-run]

Options:
    --bankroll AMOUNT   Reset bankroll to this amount (default: 1000)
    --dry-run           Show what would be done without making changes
"""

import argparse
import asyncio
import os
import sys
from datetime import datetime, timezone

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
    """Get all open positions."""
    rows = await conn.fetch(
        """
        SELECT trade_id, game_id, market_title, side, entry_price, size, entry_time
        FROM paper_trades
        WHERE status = 'open'
        ORDER BY time DESC
        """
    )
    return [dict(row) for row in rows]


async def get_bankroll(conn) -> dict:
    """Get current bankroll state."""
    row = await conn.fetchrow(
        "SELECT * FROM bankroll WHERE account_name = 'default'"
    )
    return dict(row) if row else None


async def get_trade_summary(conn) -> dict:
    """Get summary of all trades."""
    row = await conn.fetchrow(
        """
        SELECT
            COUNT(*) as total_trades,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
            SUM(CASE WHEN outcome = 'push' THEN 1 ELSE 0 END) as pushes,
            SUM(CASE WHEN status = 'open' THEN 1 ELSE 0 END) as open_positions,
            COALESCE(SUM(pnl), 0) as total_pnl
        FROM paper_trades
        """
    )
    return dict(row) if row else {}


async def force_close_positions(conn, dry_run: bool = False) -> int:
    """Force close all open positions with push outcome (no P&L)."""
    if dry_run:
        result = await conn.fetchval(
            "SELECT COUNT(*) FROM paper_trades WHERE status = 'open'"
        )
        return result

    # Close all open positions as 'push' (neutral outcome)
    result = await conn.execute(
        """
        UPDATE paper_trades
        SET
            status = 'closed',
            outcome = 'push',
            exit_price = entry_price,
            exit_time = $1,
            pnl = 0,
            pnl_pct = 0
        WHERE status = 'open'
        """,
        datetime.now(timezone.utc)
    )
    # Extract count from "UPDATE X" string
    count = int(result.split()[-1]) if result else 0
    return count


async def reset_bankroll(conn, amount: float, dry_run: bool = False) -> dict:
    """Reset bankroll to specified amount."""
    if dry_run:
        return {"new_balance": amount}

    await conn.execute(
        """
        UPDATE bankroll
        SET
            current_balance = $1,
            reserved_balance = 0,
            peak_balance = $1,
            trough_balance = $1,
            updated_at = NOW()
        WHERE account_name = 'default'
        """,
        amount
    )
    return {"new_balance": amount}


async def main():
    parser = argparse.ArgumentParser(
        description="Reset paper trading - close positions and reset bankroll"
    )
    parser.add_argument(
        "--bankroll",
        type=float,
        default=1000.0,
        help="Reset bankroll to this amount (default: 1000)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be done without making changes"
    )
    args = parser.parse_args()

    conn = await get_connection()

    try:
        print("=" * 60)
        print("PAPER TRADING RESET SCRIPT")
        print("=" * 60)

        if args.dry_run:
            print("\n[!] DRY RUN MODE - No changes will be made\n")

        # Show current state
        print("\n[CURRENT STATE]")
        print("-" * 40)

        bankroll = await get_bankroll(conn)
        if bankroll:
            print(f"  Initial Balance:  ${bankroll['initial_balance']:,.2f}")
            print(f"  Current Balance:  ${bankroll['current_balance']:,.2f}")
            print(f"  Reserved Balance: ${bankroll['reserved_balance']:,.2f}")
            print(f"  Peak Balance:     ${bankroll['peak_balance']:,.2f}")
            print(f"  Trough Balance:   ${bankroll['trough_balance']:,.2f}")

        summary = await get_trade_summary(conn)
        print(f"\n  Total Trades:     {summary.get('total_trades', 0)}")
        print(f"  Wins:             {summary.get('wins', 0)}")
        print(f"  Losses:           {summary.get('losses', 0)}")
        print(f"  Pushes:           {summary.get('pushes', 0)}")
        print(f"  Open Positions:   {summary.get('open_positions', 0)}")
        print(f"  Total P&L:        ${float(summary.get('total_pnl', 0)):,.2f}")

        # Show open positions
        open_positions = await get_open_positions(conn)
        if open_positions:
            print(f"\n[OPEN POSITIONS] ({len(open_positions)}):")
            print("-" * 40)
            for pos in open_positions:
                print(f"  - {pos['market_title']}")
                print(f"    Side: {pos['side'].upper()}, Entry: ${float(pos['entry_price']):.4f}, Size: ${float(pos['size']):.2f}")

        # Perform reset
        print(f"\n[ACTIONS]")
        print("-" * 40)

        # Force close positions
        closed_count = await force_close_positions(conn, dry_run=args.dry_run)
        action = "Would close" if args.dry_run else "Closed"
        print(f"  {action} {closed_count} open position(s) as PUSH (no P&L)")

        # Reset bankroll
        await reset_bankroll(conn, args.bankroll, dry_run=args.dry_run)
        action = "Would reset" if args.dry_run else "Reset"
        print(f"  {action} bankroll to ${args.bankroll:,.2f}")

        # Show new state
        if not args.dry_run:
            print(f"\n[NEW STATE]")
            print("-" * 40)

            bankroll = await get_bankroll(conn)
            if bankroll:
                print(f"  Current Balance:  ${bankroll['current_balance']:,.2f}")

            summary = await get_trade_summary(conn)
            print(f"  Open Positions:   {summary.get('open_positions', 0)}")
            print(f"  Total P&L:        ${float(summary.get('total_pnl', 0)):,.2f}")

            print("\nPaper trading reset complete!")
            print("   Trade history has been preserved.")
        else:
            print(f"\n[TIP] Run without --dry-run to apply these changes")

        print("=" * 60)

    finally:
        await conn.close()


if __name__ == "__main__":
    asyncio.run(main())
