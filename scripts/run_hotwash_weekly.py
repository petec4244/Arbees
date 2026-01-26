#!/usr/bin/env python3
"""Run the hot wash report for a full week (7 days)."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

import logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

from services.ml_analyzer.analyzer import MLAnalyzer


async def run_hotwash_weekly(end_date: date):
    """Run the hot wash analysis for a full week."""
    analyzer = MLAnalyzer()
    start_date = end_date - timedelta(days=6)

    print(f"\n{'='*80}")
    print(f"WEEKLY HOT WASH REPORT: {start_date} to {end_date}")
    print(f"{'='*80}\n")

    total_trades = 0
    total_pnl = 0.0
    total_wins = 0
    total_losses = 0
    results = []
    sport_totals = {}

    for i in range(7):
        target = start_date + timedelta(days=i)
        day_name = target.strftime('%a')
        print(f"\n--- Processing {target} ({day_name}) ---")

        try:
            insights = await analyzer.run_nightly_analysis(target)
            trades = insights.total_trades
            pnl = insights.total_pnl
            wins = insights.winning_trades
            losses = insights.losing_trades

            total_trades += trades
            total_pnl += pnl
            total_wins += wins
            total_losses += losses
            results.append((target, day_name, trades, pnl, insights.win_rate))

            # Aggregate sport data
            for sport, data in insights.by_sport.items():
                if sport not in sport_totals:
                    sport_totals[sport] = {'trades': 0, 'pnl': 0, 'wins': 0, 'losses': 0}
                sport_totals[sport]['trades'] += data.get('trades', 0)
                sport_totals[sport]['pnl'] += data.get('pnl', 0)
                sport_totals[sport]['wins'] += data.get('wins', 0)
                sport_totals[sport]['losses'] += data.get('losses', 0)

            print(f"    Completed: {trades} trades, ${pnl:+.2f} P&L, {insights.win_rate:.1f}% WR")
        except Exception as e:
            print(f"    Error: {e}")
            results.append((target, day_name, 0, 0, 0))

    # Summary
    print(f"\n{'='*80}")
    print("WEEKLY SUMMARY")
    print(f"{'='*80}")

    print(f"\n{'Date':<12} {'Day':>4} {'Trades':>8} {'P&L':>12} {'Win Rate':>10}")
    print("-" * 50)

    for d, day, trades, pnl, wr in results:
        status = "***" if trades == 0 else ""
        print(f"{d}  {day:>4}   {trades:>6}   ${pnl:>+9.2f}   {wr:>8.1f}% {status}")

    print("-" * 50)
    total_wr = (total_wins / (total_wins + total_losses) * 100) if (total_wins + total_losses) > 0 else 0
    print(f"{'TOTAL':<12}  {'':>4}   {total_trades:>6}   ${total_pnl:>+9.2f}   {total_wr:>8.1f}%")

    # Sport breakdown
    if sport_totals:
        print(f"\n{'='*60}")
        print("BY SPORT (Weekly Totals)")
        print("-" * 60)
        for sport, data in sorted(sport_totals.items(), key=lambda x: x[1]['pnl'], reverse=True):
            wr = (data['wins'] / (data['wins'] + data['losses']) * 100) if (data['wins'] + data['losses']) > 0 else 0
            print(f"  {sport:5} {data['trades']:4} trades   ${data['pnl']:>+9.2f} pnl   {wr:6.1f}% WR")

    # Streak analysis
    winning_days = sum(1 for _, _, trades, pnl, _ in results if pnl > 0)
    losing_days = sum(1 for _, _, trades, pnl, _ in results if pnl < 0)
    flat_days = sum(1 for _, _, trades, pnl, _ in results if pnl == 0)

    print(f"\n{'='*60}")
    print("STREAK ANALYSIS")
    print("-" * 60)
    print(f"  Winning days: {winning_days}")
    print(f"  Losing days:  {losing_days}")
    print(f"  Flat days:    {flat_days}")

    if results:
        best_day = max(results, key=lambda x: x[3])
        worst_day = min(results, key=lambda x: x[3])
        print(f"  Best day:     {best_day[0]} ({best_day[1]}) ${best_day[3]:+.2f}")
        print(f"  Worst day:    {worst_day[0]} ({worst_day[1]}) ${worst_day[3]:+.2f}")

    print(f"\nIndividual reports saved to reports/hot_wash_*.md and .html")


if __name__ == "__main__":
    if len(sys.argv) > 1:
        parts = sys.argv[1].split('-')
        if len(parts) == 3:
            end = date(int(parts[0]), int(parts[1]), int(parts[2]))
        else:
            parts = sys.argv[1].split('/')
            end = date(2026, int(parts[0]), int(parts[1]))
    else:
        end = date.today()

    asyncio.run(run_hotwash_weekly(end))
