#!/usr/bin/env python3
"""
Reset Paper Trading Script

Reset trading state for a fresh start. Options:
  - Default: Close positions, reset bankroll (preserves history)
  - --full: Also clears all trade history, signals, and opportunities

Usage:
    python scripts/reset_paper_trading.py [--bankroll 1000] [--dry-run] [--full]

Options:
    --bankroll AMOUNT   Reset bankroll to this amount (default: 1000)
    --dry-run           Show what would be done without making changes
    --full              Full reset: also clears trade history, signals, opportunities
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


async def reset_daily_pnl(conn, dry_run: bool = False) -> dict:
    """
    Reset today's daily P&L by moving all trades closed today to yesterday.
    This ensures the daily loss limit counter starts fresh.
    """
    from datetime import date, timedelta

    today = date.today()
    yesterday = today - timedelta(days=1)
    yesterday_dt = datetime(yesterday.year, yesterday.month, yesterday.day, 23, 59, 59, tzinfo=timezone.utc)

    if dry_run:
        # Count trades closed today
        count = await conn.fetchval(
            """
            SELECT COUNT(*) FROM paper_trades
            WHERE status = 'closed' AND DATE(exit_time) = $1
            """,
            today
        )
        pnl = await conn.fetchval(
            """
            SELECT COALESCE(SUM(pnl), 0) FROM paper_trades
            WHERE status = 'closed' AND DATE(exit_time) = $1
            """,
            today
        )
        return {"trades_moved": count, "pnl_cleared": float(pnl) if pnl else 0.0}

    # Move today's closed trades to yesterday (so they don't count in daily P&L)
    result = await conn.execute(
        """
        UPDATE paper_trades
        SET exit_time = $1
        WHERE status = 'closed' AND DATE(exit_time) = $2
        """,
        yesterday_dt,
        today
    )
    count = int(result.split()[-1]) if result else 0
    return {"trades_moved": count}


async def reset_bankroll(conn, amount: float, dry_run: bool = False) -> dict:
    """Reset bankroll to specified amount, including piggybank."""
    if dry_run:
        return {"new_balance": amount}

    await conn.execute(
        """
        UPDATE bankroll
        SET
            initial_balance = $1,
            current_balance = $1,
            reserved_balance = 0,
            peak_balance = $1,
            trough_balance = $1,
            piggybank_balance = 0,
            updated_at = NOW()
        WHERE account_name = 'default'
        """,
        amount
    )
    return {"new_balance": amount}


async def clear_trade_history(conn, dry_run: bool = False) -> dict:
    """Clear all trade history from paper_trades table."""
    if dry_run:
        count = await conn.fetchval("SELECT COUNT(*) FROM paper_trades")
        return {"trades_cleared": count}

    # Delete all paper trades
    result = await conn.execute("DELETE FROM paper_trades")
    count = int(result.split()[-1]) if result else 0
    return {"trades_cleared": count}


async def clear_trading_signals(conn, dry_run: bool = False) -> dict:
    """Clear all trading signals."""
    if dry_run:
        count = await conn.fetchval("SELECT COUNT(*) FROM trading_signals")
        return {"signals_cleared": count}

    result = await conn.execute("DELETE FROM trading_signals")
    count = int(result.split()[-1]) if result else 0
    return {"signals_cleared": count}


async def clear_arbitrage_opportunities(conn, dry_run: bool = False) -> dict:
    """Clear all arbitrage opportunities."""
    if dry_run:
        count = await conn.fetchval("SELECT COUNT(*) FROM arbitrage_opportunities")
        return {"opportunities_cleared": count}

    result = await conn.execute("DELETE FROM arbitrage_opportunities")
    count = int(result.split()[-1]) if result else 0
    return {"opportunities_cleared": count}


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
    parser.add_argument(
        "--full",
        action="store_true",
        help="Full reset: also clears trade history, signals, and opportunities"
    )
    args = parser.parse_args()

    conn = await get_connection()

    try:
        print("=" * 60)
        print("PAPER TRADING RESET SCRIPT")
        if args.full:
            print("  [FULL RESET MODE - History will be cleared]")
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
            # Check for piggybank column
            piggybank = bankroll.get('piggybank_balance', 0) or 0
            print(f"  Piggybank:        ${float(piggybank):,.2f}")

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
            for pos in open_positions[:5]:  # Show first 5
                print(f"  - {pos['market_title']}")
                print(f"    Side: {pos['side'].upper()}, Entry: ${float(pos['entry_price']):.4f}, Size: ${float(pos['size']):.2f}")
            if len(open_positions) > 5:
                print(f"  ... and {len(open_positions) - 5} more")

        # Perform reset
        print(f"\n[ACTIONS]")
        print("-" * 40)

        # Force close positions
        closed_count = await force_close_positions(conn, dry_run=args.dry_run)
        action = "Would close" if args.dry_run else "Closed"
        print(f"  {action} {closed_count} open position(s) as PUSH (no P&L)")

        # Reset daily P&L counter (move today's trades to yesterday)
        daily_result = await reset_daily_pnl(conn, dry_run=args.dry_run)
        action = "Would move" if args.dry_run else "Moved"
        trades_moved = daily_result.get('trades_moved', 0)
        if trades_moved > 0:
            pnl_info = f" (P&L: ${daily_result.get('pnl_cleared', 0):,.2f})" if args.dry_run else ""
            print(f"  {action} {trades_moved} trade(s) to yesterday (resets daily P&L limit){pnl_info}")
        else:
            print(f"  No trades to move (daily P&L already clean)")

        # Reset bankroll (includes piggybank)
        await reset_bankroll(conn, args.bankroll, dry_run=args.dry_run)
        action = "Would reset" if args.dry_run else "Reset"
        print(f"  {action} bankroll to ${args.bankroll:,.2f} (piggybank cleared)")

        # Full reset - clear history if requested
        if args.full:
            print("\n  [FULL RESET - Clearing history...]")

            result = await clear_trade_history(conn, dry_run=args.dry_run)
            action = "Would clear" if args.dry_run else "Cleared"
            print(f"  {action} {result['trades_cleared']} trade records")

            result = await clear_trading_signals(conn, dry_run=args.dry_run)
            print(f"  {action} {result['signals_cleared']} trading signals")

            result = await clear_arbitrage_opportunities(conn, dry_run=args.dry_run)
            print(f"  {action} {result['opportunities_cleared']} arbitrage opportunities")

        # Show new state
        if not args.dry_run:
            print(f"\n[NEW STATE]")
            print("-" * 40)

            bankroll = await get_bankroll(conn)
            if bankroll:
                print(f"  Current Balance:  ${bankroll['current_balance']:,.2f}")
                piggybank = bankroll.get('piggybank_balance', 0) or 0
                print(f"  Piggybank:        ${float(piggybank):,.2f}")

            summary = await get_trade_summary(conn)
            print(f"  Open Positions:   {summary.get('open_positions', 0)}")
            if not args.full:
                print(f"  Historical P&L:   ${float(summary.get('total_pnl', 0)):,.2f} (preserved)")
            else:
                print(f"  Total Trades:     {summary.get('total_trades', 0)}")

            print("\nReset complete!")
            if not args.full:
                print("  Trade history preserved (use --full to clear).")
            print(f"  Starting fresh with ${args.bankroll:,.2f}")
        else:
            print(f"\n[TIP] Run without --dry-run to apply these changes")

        print("=" * 60)

    finally:
        await conn.close()


if __name__ == "__main__":
    asyncio.run(main())
