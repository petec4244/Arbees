#!/usr/bin/env python3
"""
Clean up today's invalid trades and run anomaly detection.

This script:
1. Runs anomaly detection on today's trades
2. Shows problematic trades
3. Optionally deletes/resets the bad data
"""

import asyncio
from datetime import date
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool
from services.ml_analyzer.anomaly_detector import AnomalyDetector


async def run_anomaly_detection():
    """Run anomaly detection on today's data."""
    pool = await get_pool()
    today = date.today()

    print(f"ANOMALY DETECTION FOR {today}")
    print("="*80)

    # Get all trades for today
    trades = await pool.fetch("""
        SELECT * FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        ORDER BY entry_time
    """, today)

    if not trades:
        print("No trades found for today.")
        return

    # Convert to list of dicts
    trade_list = [dict(t) for t in trades]

    # Run anomaly detection
    detector = AnomalyDetector()
    report = detector.analyze(trade_list, today)

    # Print results
    print(detector.format_report(report))

    return report


async def show_problematic_games(report):
    """Show details of games with detected anomalies."""
    pool = await get_pool()
    today = date.today()

    # Get game IDs from anomalies
    game_ids = set()
    for anomaly in report.anomalies:
        if "game_id" in anomaly.details:
            game_ids.add(anomaly.details["game_id"])

    if not game_ids:
        print("\nNo specific games flagged.")
        return

    print("\n" + "="*80)
    print("PROBLEMATIC GAMES DETAILS")
    print("="*80)

    for game_id in game_ids:
        trades = await pool.fetch("""
            SELECT trade_id, side, entry_price, exit_price, size, pnl, outcome, entry_time, market_title
            FROM paper_trades
            WHERE game_id = $1
            ORDER BY entry_time
        """, game_id)

        print(f"\n### Game: {game_id}")
        print(f"Trades: {len(trades)}")

        total_pnl = 0
        total_volume = 0
        for t in trades:
            pnl = float(t['pnl'] or 0)
            size = float(t['size'])
            total_pnl += pnl
            total_volume += size
            print(f"  {t['entry_time'].strftime('%H:%M:%S')} | {t['side']:4} | "
                  f"entry={float(t['entry_price']):.3f} exit={float(t['exit_price'] or 0):.3f} | "
                  f"size=${size:.2f} | pnl=${pnl:+.2f}")

        print(f"  TOTAL: ${total_volume:,.2f} volume, ${total_pnl:+,.2f} PnL")


async def cleanup_invalid_trades():
    """Delete today's invalid trades and reset bankroll."""
    pool = await get_pool()
    today = date.today()

    print("\n" + "="*80)
    print("CLEANUP OPTIONS")
    print("="*80)

    # Count trades
    count = await pool.fetchval("""
        SELECT COUNT(*) FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
    """, today)

    print(f"\nTotal trades today: {count}")

    # Get current bankroll
    row = await pool.fetchrow("""
        SELECT * FROM bankroll WHERE account_name = 'default'
    """)

    if row:
        print(f"\nCurrent bankroll:")
        print(f"  Trading: ${float(row['current_balance']):,.2f}")
        piggy = float(row.get('piggybank_balance') or 0)
        print(f"  Piggybank: ${piggy:,.2f}")
        print(f"  Initial: ${float(row['initial_balance']):,.2f}")

    # Calculate what bankroll should be from yesterday
    yesterday_result = await pool.fetchrow("""
        SELECT
            SUM(CASE WHEN DATE(exit_time) = $1 THEN pnl ELSE 0 END) as today_pnl,
            SUM(CASE WHEN DATE(exit_time) < $1 THEN pnl ELSE 0 END) as past_pnl
        FROM paper_trades
        WHERE status = 'closed'
    """, today)

    today_pnl = float(yesterday_result['today_pnl'] or 0)
    past_pnl = float(yesterday_result['past_pnl'] or 0)

    print(f"\nPnL breakdown:")
    print(f"  Today's PnL: ${today_pnl:,.2f}")
    print(f"  Past PnL: ${past_pnl:,.2f}")

    if row:
        initial = float(row['initial_balance'])
        expected_bankroll = initial + past_pnl  # Exclude today's problematic trades
        print(f"\nExpected bankroll (without today): ${expected_bankroll:,.2f}")

    print("\n" + "-"*80)
    print("TO CLEANUP:")
    print("  1. Delete all trades from today")
    print("  2. Reset bankroll to initial + past_pnl")
    print("  3. Reset piggybank to 0")
    print("-"*80)

    # Prompt for confirmation
    response = input("\nType 'DELETE' to cleanup today's trades, or anything else to cancel: ")

    if response.strip() == "DELETE":
        # Delete today's trades
        deleted = await pool.execute("""
            DELETE FROM paper_trades
            WHERE DATE(entry_time) = $1
        """, today)
        print(f"\nDeleted trades from {today}")

        # Reset bankroll
        if row:
            initial = float(row['initial_balance'])
            new_balance = initial + past_pnl
            await pool.execute("""
                UPDATE bankroll
                SET current_balance = $1, piggybank_balance = 0,
                    peak_balance = GREATEST(peak_balance, $1),
                    trough_balance = LEAST(trough_balance, $1),
                    updated_at = NOW()
                WHERE account_name = 'default'
            """, new_balance)
            print(f"Reset bankroll to ${new_balance:,.2f}")

        print("\nCleanup complete!")
    else:
        print("\nCleanup cancelled.")


async def main():
    """Run the full cleanup process."""
    try:
        # 1. Run anomaly detection
        report = await run_anomaly_detection()

        if report and report.anomalies:
            # 2. Show details of problematic games
            await show_problematic_games(report)

            # 3. Offer to cleanup
            await cleanup_invalid_trades()
        else:
            print("\nNo anomalies detected. No cleanup needed.")

    finally:
        await close_pool()


if __name__ == "__main__":
    asyncio.run(main())
