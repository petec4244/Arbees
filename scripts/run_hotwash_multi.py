#!/usr/bin/env python3
"""Run the hot wash report for multiple days."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

import logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

from services.ml_analyzer.analyzer import MLAnalyzer


async def run_hotwash_multi(end_date: date, days: int):
    """Run the hot wash analysis for multiple days."""
    analyzer = MLAnalyzer()

    print(f"\n{'='*80}")
    print(f"RUNNING HOT WASH REPORTS FOR {days} DAYS ({end_date - timedelta(days=days-1)} to {end_date})")
    print(f"{'='*80}\n")

    total_trades = 0
    total_pnl = 0.0
    results = []

    for i in range(days):
        target = end_date - timedelta(days=days - 1 - i)
        print(f"\n--- Processing {target} ---")

        try:
            insights = await analyzer.run_nightly_analysis(target)
            trades = insights.total_trades
            pnl = insights.total_pnl
            total_trades += trades
            total_pnl += pnl
            results.append((target, trades, pnl, insights.win_rate))
            print(f"    Completed: {trades} trades, ${pnl:+.2f} P&L, {insights.win_rate:.1f}% WR")
        except Exception as e:
            print(f"    Error: {e}")
            results.append((target, 0, 0, 0))

    # Summary
    print(f"\n{'='*80}")
    print("MULTI-DAY SUMMARY")
    print(f"{'='*80}")
    print(f"\n{'Date':<12} {'Trades':>8} {'P&L':>12} {'Win Rate':>10}")
    print("-" * 44)

    for d, trades, pnl, wr in results:
        print(f"{d}    {trades:>6}   ${pnl:>+9.2f}   {wr:>8.1f}%")

    print("-" * 44)
    print(f"{'TOTAL':<12} {total_trades:>6}   ${total_pnl:>+9.2f}")
    print(f"\nReports saved to reports/hot_wash_*.md and .html")


if __name__ == "__main__":
    days = 3  # Default to 3 days
    end = date.today()

    if len(sys.argv) > 1:
        # Check if first arg is a number (days) or a date
        try:
            days = int(sys.argv[1])
        except ValueError:
            # It's a date
            parts = sys.argv[1].split('-')
            if len(parts) == 3:
                end = date(int(parts[0]), int(parts[1]), int(parts[2]))
            else:
                parts = sys.argv[1].split('/')
                end = date(2026, int(parts[0]), int(parts[1]))

    if len(sys.argv) > 2:
        days = int(sys.argv[2])

    asyncio.run(run_hotwash_multi(end, days))
