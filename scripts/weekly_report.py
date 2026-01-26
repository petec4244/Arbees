#!/usr/bin/env python3
"""Generate a weekly trading report (last 7 days)."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def weekly_report(end_date: date):
    pool = await get_pool()
    start_date = end_date - timedelta(days=6)

    print('='*80)
    print(f'WEEKLY TRADING REPORT: {start_date} to {end_date}')
    print('='*80)

    # Daily breakdown
    print('\nDAILY BREAKDOWN:')
    print('-'*80)
    print(f'{"Date":<12} {"Day":>4} {"Trades":>7} {"Games":>6} {"Volume":>12} {"PnL":>12} {"WR":>8}')
    print('-'*80)

    daily_pnl = []
    total_trades = 0
    total_volume = 0.0
    total_pnl = 0.0
    total_wins = 0
    total_losses = 0

    for i in range(7):
        d = start_date + timedelta(days=i)
        row = await pool.fetchrow("""
            SELECT
                COUNT(*) as trades,
                COUNT(DISTINCT game_id) as games,
                COALESCE(SUM(size), 0) as volume,
                COALESCE(SUM(pnl), 0) as pnl,
                SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
                SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
            FROM paper_trades
            WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        """, d)

        wins = row['wins'] or 0
        losses = row['losses'] or 0
        total_closed = wins + losses
        wr = (wins / total_closed * 100) if total_closed > 0 else 0
        pnl = float(row['pnl'])

        total_trades += row['trades']
        total_volume += float(row['volume'])
        total_pnl += pnl
        total_wins += wins
        total_losses += losses
        daily_pnl.append((d, pnl))

        day_name = d.strftime('%a')
        status = '***' if row['trades'] == 0 else ''
        print(f'{d} {day_name:>4}  {row["trades"]:>5}  {row["games"]:>5}  '
              f'${float(row["volume"]):>10,.2f}  ${pnl:>+10.2f}  {wr:>6.1f}% {status}')

    print('-'*80)
    total_closed = total_wins + total_losses
    total_wr = (total_wins / total_closed * 100) if total_closed > 0 else 0
    print(f'{"TOTAL":<12} {"":>4}  {total_trades:>5}  {"":>5}  '
          f'${total_volume:>10,.2f}  ${total_pnl:>+10.2f}  {total_wr:>6.1f}%')

    # Streak analysis
    print()
    print('STREAK ANALYSIS:')
    print('-'*60)
    winning_days = sum(1 for _, pnl in daily_pnl if pnl > 0)
    losing_days = sum(1 for _, pnl in daily_pnl if pnl < 0)
    flat_days = sum(1 for _, pnl in daily_pnl if pnl == 0)
    print(f'  Winning days: {winning_days}')
    print(f'  Losing days: {losing_days}')
    print(f'  Flat days: {flat_days}')

    if daily_pnl:
        best_day = max(daily_pnl, key=lambda x: x[1])
        worst_day = min(daily_pnl, key=lambda x: x[1])
        print(f'  Best day: {best_day[0]} (${best_day[1]:+.2f})')
        print(f'  Worst day: {worst_day[0]} (${worst_day[1]:+.2f})')

    # Overall summary
    summary = await pool.fetchrow("""
        SELECT
            COUNT(*) as total_trades,
            COUNT(DISTINCT game_id) as games,
            SUM(size) as volume,
            SUM(pnl) as total_pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses,
            AVG(size) as avg_size,
            AVG(edge_at_entry) as avg_edge,
            AVG(pnl) as avg_pnl,
            STDDEV(pnl) as stddev_pnl
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
    """, start_date, end_date)

    total = (summary['wins'] or 0) + (summary['losses'] or 0)
    wr = (summary['wins'] / total * 100) if total > 0 else 0
    avg_pnl = float(summary['avg_pnl'] or 0)
    stddev = float(summary['stddev_pnl'] or 0)
    sharpe = (avg_pnl / stddev) if stddev > 0 else 0

    print(f"""
WEEK SUMMARY:
  Total Trades: {summary['total_trades']}
  Unique Games: {summary['games']}
  Volume: ${float(summary['volume'] or 0):,.2f}
  Total PnL: ${float(summary['total_pnl'] or 0):+,.2f}
  Win Rate: {wr:.1f}% ({summary['wins']}W / {summary['losses']}L)
  Avg Trade Size: ${float(summary['avg_size'] or 0):.2f}
  Avg Edge: {float(summary['avg_edge'] or 0):.2f}%
  Avg PnL/Trade: ${avg_pnl:.2f}
  PnL Std Dev: ${stddev:.2f}
  Sharpe (per trade): {sharpe:.2f}
""")

    # By sport
    sports = await pool.fetch("""
        SELECT sport,
            COUNT(*) as trades,
            SUM(size) as volume,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
        GROUP BY sport
        ORDER BY pnl DESC
    """, start_date, end_date)

    print('BY SPORT (sorted by PnL):')
    print('-'*70)
    for s in sports:
        total = (s['wins'] or 0) + (s['losses'] or 0)
        wr = (s['wins'] / total * 100) if total > 0 else 0
        print(f"  {s['sport']:5} {s['trades']:4} trades  ${float(s['volume']):>10,.2f} vol  "
              f"${float(s['pnl'] or 0):>+10.2f} pnl  {wr:6.1f}% WR")

    # Best and worst trades
    best = await pool.fetch("""
        SELECT trade_id, market_title, sport, side, pnl, entry_time
        FROM paper_trades
        WHERE ((DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2))
           AND pnl IS NOT NULL
        ORDER BY pnl DESC
        LIMIT 10
    """, start_date, end_date)

    worst = await pool.fetch("""
        SELECT trade_id, market_title, sport, side, pnl, entry_time
        FROM paper_trades
        WHERE ((DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2))
           AND pnl IS NOT NULL
        ORDER BY pnl ASC
        LIMIT 10
    """, start_date, end_date)

    print()
    print('TOP 10 BEST TRADES:')
    print('-'*70)
    for t in best:
        print(f"  {t['entry_time'].strftime('%m/%d')} {t['sport']:5} {t['side']:4} "
              f"${float(t['pnl']):>+8.2f}  {t['market_title'][:35]}")

    print()
    print('TOP 10 WORST TRADES:')
    print('-'*70)
    for t in worst:
        print(f"  {t['entry_time'].strftime('%m/%d')} {t['sport']:5} {t['side']:4} "
              f"${float(t['pnl']):>+8.2f}  {t['market_title'][:35]}")

    # Win rate by hour (to find best trading times)
    hourly = await pool.fetch("""
        SELECT
            EXTRACT(HOUR FROM entry_time) as hour,
            COUNT(*) as trades,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
        GROUP BY EXTRACT(HOUR FROM entry_time)
        ORDER BY hour
    """, start_date, end_date)

    if hourly:
        print()
        print('BY HOUR (UTC):')
        print('-'*70)
        for h in hourly:
            total = (h['wins'] or 0) + (h['losses'] or 0)
            wr = (h['wins'] / total * 100) if total > 0 else 0
            hour = int(h['hour'])
            print(f"  {hour:02d}:00  {h['trades']:4} trades  "
                  f"${float(h['pnl'] or 0):>+10.2f} pnl  {wr:6.1f}% WR")

    # Buy vs Sell performance
    sides = await pool.fetch("""
        SELECT
            side,
            COUNT(*) as trades,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
        GROUP BY side
    """, start_date, end_date)

    print()
    print('BUY vs SELL:')
    print('-'*70)
    for s in sides:
        total = (s['wins'] or 0) + (s['losses'] or 0)
        wr = (s['wins'] / total * 100) if total > 0 else 0
        print(f"  {s['side']:5} {s['trades']:4} trades  "
              f"${float(s['pnl'] or 0):>+10.2f} pnl  {wr:6.1f}% WR")

    await close_pool()


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

    asyncio.run(weekly_report(end))
