#!/usr/bin/env python3
"""Generate a multi-day trading report (default: 3 days)."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def multi_day_report(end_date: date, days: int = 3):
    pool = await get_pool()
    start_date = end_date - timedelta(days=days - 1)

    print('='*80)
    print(f'MULTI-DAY TRADING REPORT: {start_date} to {end_date} ({days} days)')
    print('='*80)

    # Daily breakdown
    print('\nDAILY BREAKDOWN:')
    print('-'*80)
    print(f'{"Date":<12} {"Trades":>7} {"Games":>6} {"Volume":>12} {"PnL":>12} {"Win Rate":>10}')
    print('-'*80)

    total_trades = 0
    total_volume = 0.0
    total_pnl = 0.0
    total_wins = 0
    total_losses = 0

    for i in range(days):
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

        total_trades += row['trades']
        total_volume += float(row['volume'])
        total_pnl += float(row['pnl'])
        total_wins += wins
        total_losses += losses

        day_name = d.strftime('%a')
        print(f'{d} ({day_name})  {row["trades"]:>5}  {row["games"]:>5}  '
              f'${float(row["volume"]):>10,.2f}  ${float(row["pnl"]):>+10.2f}  {wr:>8.1f}%')

    print('-'*80)
    total_closed = total_wins + total_losses
    total_wr = (total_wins / total_closed * 100) if total_closed > 0 else 0
    print(f'{"TOTAL":<12}  {total_trades:>5}  {"":>5}  '
          f'${total_volume:>10,.2f}  ${total_pnl:>+10.2f}  {total_wr:>8.1f}%')

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
            AVG(edge_at_entry) as avg_edge
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
    """, start_date, end_date)

    total = (summary['wins'] or 0) + (summary['losses'] or 0)
    wr = (summary['wins'] / total * 100) if total > 0 else 0

    print(f"""
PERIOD SUMMARY:
  Total Trades: {summary['total_trades']}
  Unique Games: {summary['games']}
  Volume: ${float(summary['volume'] or 0):,.2f}
  Total PnL: ${float(summary['total_pnl'] or 0):+,.2f}
  Win Rate: {wr:.1f}% ({summary['wins']}W / {summary['losses']}L)
  Avg Size: ${float(summary['avg_size'] or 0):.2f}
  Avg Edge: {float(summary['avg_edge'] or 0):.2f}%
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
        ORDER BY volume DESC
    """, start_date, end_date)

    print('BY SPORT:')
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
        LIMIT 5
    """, start_date, end_date)

    worst = await pool.fetch("""
        SELECT trade_id, market_title, sport, side, pnl, entry_time
        FROM paper_trades
        WHERE ((DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2))
           AND pnl IS NOT NULL
        ORDER BY pnl ASC
        LIMIT 5
    """, start_date, end_date)

    print()
    print('TOP 5 BEST TRADES:')
    print('-'*70)
    for t in best:
        print(f"  {t['entry_time'].strftime('%m/%d')} {t['sport']:5} {t['side']:4} "
              f"${float(t['pnl']):>+8.2f}  {t['market_title'][:35]}")

    print()
    print('TOP 5 WORST TRADES:')
    print('-'*70)
    for t in worst:
        print(f"  {t['entry_time'].strftime('%m/%d')} {t['sport']:5} {t['side']:4} "
              f"${float(t['pnl']):>+8.2f}  {t['market_title'][:35]}")

    # Win rate by day of week
    dow_stats = await pool.fetch("""
        SELECT
            EXTRACT(DOW FROM entry_time) as dow,
            TO_CHAR(entry_time, 'Dy') as day_name,
            COUNT(*) as trades,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE (DATE(entry_time) >= $1 AND DATE(entry_time) <= $2)
           OR (DATE(exit_time) >= $1 AND DATE(exit_time) <= $2)
        GROUP BY EXTRACT(DOW FROM entry_time), TO_CHAR(entry_time, 'Dy')
        ORDER BY dow
    """, start_date, end_date)

    if dow_stats:
        print()
        print('BY DAY OF WEEK:')
        print('-'*70)
        for d in dow_stats:
            total = (d['wins'] or 0) + (d['losses'] or 0)
            wr = (d['wins'] / total * 100) if total > 0 else 0
            print(f"  {d['day_name']:3}  {d['trades']:4} trades  "
                  f"${float(d['pnl'] or 0):>+10.2f} pnl  {wr:6.1f}% WR")

    await close_pool()


if __name__ == "__main__":
    days = 3
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

    asyncio.run(multi_day_report(end, days))
