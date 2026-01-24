#!/usr/bin/env python3
"""
Paper Trading Validation Script

Validates that the emergency debug fixes are working by checking:
1. No immediate exits (< MIN_HOLD_SECONDS)
2. No bad entry prices (> 85%)
3. Positive P&L
4. Win rate > 40%
5. Average hold time > 30 seconds

Usage:
    # With DATABASE_URL from environment
    python scripts/validate_paper_trading.py

    # With explicit DATABASE_URL
    DATABASE_URL="postgresql://..." python scripts/validate_paper_trading.py

    # Check specific time window (default: 24 hours)
    python scripts/validate_paper_trading.py --hours 12
"""

import argparse
import asyncio
import os
import sys
from datetime import datetime, timezone

# Add project root to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


async def main():
    parser = argparse.ArgumentParser(description="Validate paper trading results")
    parser.add_argument(
        "--hours",
        type=float,
        default=24.0,
        help="Number of hours to look back (default: 24)",
    )
    parser.add_argument(
        "--min-hold-seconds",
        type=float,
        default=10.0,
        help="Minimum hold time threshold (default: 10)",
    )
    parser.add_argument(
        "--max-entry-price",
        type=float,
        default=0.85,
        help="Maximum acceptable entry price (default: 0.85)",
    )
    parser.add_argument(
        "--min-win-rate",
        type=float,
        default=40.0,
        help="Minimum win rate percentage (default: 40)",
    )
    parser.add_argument(
        "--min-avg-hold",
        type=float,
        default=30.0,
        help="Minimum average hold time in seconds (default: 30)",
    )
    args = parser.parse_args()

    # Check DATABASE_URL
    database_url = os.environ.get("DATABASE_URL")
    if not database_url:
        print("ERROR: DATABASE_URL environment variable not set")
        print("Set it via: set DATABASE_URL=postgresql://arbees:password@localhost:5432/arbees")
        sys.exit(1)

    import asyncpg

    print("=" * 80)
    print("PAPER TRADING VALIDATION REPORT")
    print(f"Time Window: Last {args.hours:.1f} hours")
    print(f"Generated: {datetime.now(timezone.utc).isoformat()}")
    print("=" * 80)
    print()

    conn = await asyncpg.connect(database_url)

    try:
        all_passed = True

        # =====================================================================
        # Test 1: Check for immediate exits
        # =====================================================================
        print("Test 1: Checking for immediate exits...")
        immediate_exits = await conn.fetch(
            """
            SELECT 
                trade_id,
                game_id,
                market_title,
                side,
                entry_price,
                exit_price,
                pnl,
                EXTRACT(EPOCH FROM (exit_time - entry_time)) as hold_seconds
            FROM paper_trades
            WHERE status = 'closed'
              AND entry_time > NOW() - make_interval(hours => $1)
              AND EXTRACT(EPOCH FROM (exit_time - entry_time)) < $2
            ORDER BY entry_time DESC
            """,
            args.hours,
            args.min_hold_seconds,
        )

        if immediate_exits:
            print(f"  FAILED: Found {len(immediate_exits)} immediate exits (< {args.min_hold_seconds}s)")
            for trade in immediate_exits[:5]:
                print(f"     Trade {trade['trade_id'][:8]}...: held {trade['hold_seconds']:.1f}s")
            all_passed = False
        else:
            print(f"  PASSED: No immediate exits found (threshold: {args.min_hold_seconds}s)")
        print()

        # =====================================================================
        # Test 2: Check for bad entry prices
        # =====================================================================
        print(f"Test 2: Checking for bad entry prices (>{args.max_entry_price*100:.0f}%)...")
        bad_entries = await conn.fetch(
            """
            SELECT 
                trade_id,
                game_id,
                market_title,
                side,
                entry_price,
                size
            FROM paper_trades
            WHERE entry_time > NOW() - make_interval(hours => $1)
              AND entry_price > $2
            ORDER BY entry_price DESC
            """,
            args.hours,
            args.max_entry_price,
        )

        if bad_entries:
            print(f"  FAILED: Found {len(bad_entries)} trades with entry > {args.max_entry_price*100:.0f}%")
            for trade in bad_entries[:5]:
                print(f"     Trade {trade['trade_id'][:8]}...: entry={trade['entry_price']*100:.1f}%")
            all_passed = False
        else:
            print(f"  PASSED: No bad entry prices found")
        print()

        # =====================================================================
        # Test 3: Check for repeated losses on same teams
        # =====================================================================
        print("Test 3: Checking for repeated losses on same teams...")
        repeated_losses = await conn.fetch(
            """
            SELECT 
                market_title,
                COUNT(*) as loss_count,
                SUM(pnl) as total_loss,
                MIN(entry_time) as first_loss,
                MAX(entry_time) as last_loss
            FROM paper_trades
            WHERE status = 'closed'
              AND outcome = 'loss'
              AND entry_time > NOW() - make_interval(hours => $1)
            GROUP BY market_title
            HAVING COUNT(*) >= 2
            ORDER BY loss_count DESC
            """,
            args.hours,
        )

        if repeated_losses:
            print(f"  WARNING: Found {len(repeated_losses)} teams with multiple losses")
            for team in repeated_losses[:5]:
                print(f"     {team['market_title'][:40]}...: {team['loss_count']} losses, ${float(team['total_loss'] or 0):.2f}")
        else:
            print("  PASSED: No repeated losses on same teams")
        print()

        # =====================================================================
        # Test 4: Overall P&L
        # =====================================================================
        print("Test 4: Checking overall P&L...")
        pnl_stats = await conn.fetchrow(
            """
            SELECT 
                COUNT(*) FILTER (WHERE outcome = 'win') as wins,
                COUNT(*) FILTER (WHERE outcome = 'loss') as losses,
                COUNT(*) FILTER (WHERE outcome = 'push') as pushes,
                COUNT(*) as total,
                SUM(pnl) FILTER (WHERE outcome = 'win') as win_total,
                SUM(pnl) FILTER (WHERE outcome = 'loss') as loss_total,
                SUM(pnl) as net_pnl,
                AVG(EXTRACT(EPOCH FROM (exit_time - entry_time))) as avg_hold_seconds
            FROM paper_trades
            WHERE status = 'closed'
              AND entry_time > NOW() - make_interval(hours => $1)
            """,
            args.hours,
        )

        if pnl_stats and pnl_stats['total']:
            wins = pnl_stats['wins'] or 0
            losses = pnl_stats['losses'] or 0
            pushes = pnl_stats['pushes'] or 0
            total = pnl_stats['total'] or 0
            win_total = float(pnl_stats['win_total'] or 0)
            loss_total = float(pnl_stats['loss_total'] or 0)
            net_pnl = float(pnl_stats['net_pnl'] or 0)
            avg_hold = float(pnl_stats['avg_hold_seconds'] or 0)

            print(f"  Total Trades: {total}")
            print(f"  Wins: {wins} (${win_total:.2f})")
            print(f"  Losses: {losses} (${loss_total:.2f})")
            print(f"  Pushes: {pushes}")
            print(f"  Net P&L: ${net_pnl:.2f}")

            if net_pnl > 0:
                print("  PASSED: Positive P&L!")
            else:
                print("  WARNING: Negative P&L")
                all_passed = False
        else:
            print("  INFO: No closed trades in time window")
        print()

        # =====================================================================
        # Test 5: Win Rate
        # =====================================================================
        print(f"Test 5: Checking win rate (>={args.min_win_rate:.0f}%)...")
        if pnl_stats and pnl_stats['total'] and (pnl_stats['wins'] or 0) + (pnl_stats['losses'] or 0) > 0:
            wins = pnl_stats['wins'] or 0
            losses = pnl_stats['losses'] or 0
            win_rate = wins / (wins + losses) * 100 if (wins + losses) > 0 else 0

            print(f"  Win Rate: {win_rate:.1f}%")

            if win_rate >= args.min_win_rate:
                print(f"  PASSED: Win rate >= {args.min_win_rate:.0f}%")
            else:
                print(f"  WARNING: Win rate < {args.min_win_rate:.0f}%")
                all_passed = False
        else:
            print("  INFO: Not enough trades to calculate win rate")
        print()

        # =====================================================================
        # Test 6: Average Hold Time
        # =====================================================================
        print(f"Test 6: Checking average hold time (>={args.min_avg_hold:.0f}s)...")
        if pnl_stats and pnl_stats['avg_hold_seconds']:
            avg_hold = float(pnl_stats['avg_hold_seconds'])
            print(f"  Average Hold Time: {avg_hold:.1f}s")

            if avg_hold >= args.min_avg_hold:
                print(f"  PASSED: Average hold time >= {args.min_avg_hold:.0f}s")
            else:
                print(f"  WARNING: Average hold time < {args.min_avg_hold:.0f}s")
                all_passed = False
        else:
            print("  INFO: No closed trades to calculate hold time")
        print()

        # =====================================================================
        # Test 7: Recent Trade Details
        # =====================================================================
        print("Recent Trades (last 10):")
        recent_trades = await conn.fetch(
            """
            SELECT 
                trade_id,
                market_title,
                side,
                entry_price,
                exit_price,
                pnl,
                outcome,
                EXTRACT(EPOCH FROM (exit_time - entry_time)) as hold_seconds
            FROM paper_trades
            WHERE status = 'closed'
              AND entry_time > NOW() - make_interval(hours => $1)
            ORDER BY entry_time DESC
            LIMIT 10
            """,
            args.hours,
        )

        if recent_trades:
            for trade in recent_trades:
                pnl = float(trade['pnl'] or 0)
                hold = trade['hold_seconds'] or 0
                outcome = trade['outcome'] or 'unknown'
                emoji = "+" if pnl > 0 else "-" if pnl < 0 else "="
                print(
                    f"  {emoji} {trade['trade_id'][:8]}... | "
                    f"{trade['side']:4} @ {float(trade['entry_price'])*100:5.1f}% -> "
                    f"{float(trade['exit_price'] or 0)*100:5.1f}% | "
                    f"${pnl:+6.2f} | {outcome:5} | {hold:5.1f}s"
                )
        else:
            print("  No recent trades")
        print()

        # =====================================================================
        # Summary
        # =====================================================================
        print("=" * 80)
        if all_passed:
            print("VALIDATION PASSED - All critical checks passed!")
            print("Ready for continued paper trading or real execution.")
        else:
            print("VALIDATION FAILED - Some checks did not pass.")
            print("Review the issues above before proceeding.")
        print("=" * 80)

        return 0 if all_passed else 1

    finally:
        await conn.close()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
