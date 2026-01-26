#!/usr/bin/env python3
"""Generate a full daily trading report."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def full_report(target_date: date):
    pool = await get_pool()

    print('='*80)
    print(f'FULL TRADING REPORT FOR {target_date} (UTC)')
    print('='*80)

    # Summary stats
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
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
    """, target_date)

    total = (summary['wins'] or 0) + (summary['losses'] or 0)
    wr = (summary['wins'] / total * 100) if total > 0 else 0

    print(f"""
SUMMARY:
  Total Trades: {summary['total_trades']}
  Games: {summary['games']}
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
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        GROUP BY sport
        ORDER BY volume DESC
    """, target_date)

    print('BY SPORT:')
    print('-'*60)
    for s in sports:
        total = (s['wins'] or 0) + (s['losses'] or 0)
        wr = (s['wins'] / total * 100) if total > 0 else 0
        print(f"  {s['sport']:5} {s['trades']:3} trades  ${float(s['volume']):>8,.2f} vol  "
              f"${float(s['pnl'] or 0):>+8.2f} pnl  {wr:5.1f}% WR")

    # Best and worst trades
    best = await pool.fetch("""
        SELECT trade_id, market_title, sport, side, entry_price, exit_price, size, pnl
        FROM paper_trades
        WHERE (DATE(entry_time) = $1 OR DATE(exit_time) = $1) AND pnl IS NOT NULL
        ORDER BY pnl DESC
        LIMIT 5
    """, target_date)

    worst = await pool.fetch("""
        SELECT trade_id, market_title, sport, side, entry_price, exit_price, size, pnl
        FROM paper_trades
        WHERE (DATE(entry_time) = $1 OR DATE(exit_time) = $1) AND pnl IS NOT NULL
        ORDER BY pnl ASC
        LIMIT 5
    """, target_date)

    print()
    print('BEST TRADES:')
    print('-'*60)
    for t in best:
        print(f"  {t['sport']:5} {t['side']:4} ${float(t['pnl']):>+7.2f}  {t['market_title'][:35]}")

    print()
    print('WORST TRADES:')
    print('-'*60)
    for t in worst:
        print(f"  {t['sport']:5} {t['side']:4} ${float(t['pnl']):>+7.2f}  {t['market_title'][:35]}")

    # By game
    games = await pool.fetch("""
        SELECT game_id, market_title, sport,
            COUNT(*) as trades,
            SUM(size) as volume,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        GROUP BY game_id, market_title, sport
        ORDER BY volume DESC
        LIMIT 20
    """, target_date)

    print()
    print('BY GAME/TEAM:')
    print('-'*80)
    for g in games:
        total = (g['wins'] or 0) + (g['losses'] or 0)
        wr = (g['wins'] / total * 100) if total > 0 else 0
        print(f"  {g['sport']:5} {g['trades']:3} trades  ${float(g['volume']):>8,.2f} vol  "
              f"${float(g['pnl'] or 0):>+8.2f} pnl  {wr:5.1f}% WR  {g['market_title'][:30]}")

    # Hourly distribution
    hourly = await pool.fetch("""
        SELECT DATE_TRUNC('hour', entry_time) as hour, COUNT(*) as trades, SUM(pnl) as pnl
        FROM paper_trades
        WHERE DATE(entry_time) = $1
        GROUP BY DATE_TRUNC('hour', entry_time)
        ORDER BY hour
    """, target_date)

    print()
    print('HOURLY DISTRIBUTION:')
    print('-'*60)
    for h in hourly:
        print(f"  {h['hour'].strftime('%H:%M')}: {h['trades']:3} trades  ${float(h['pnl'] or 0):>+8.2f} pnl")

    await close_pool()


if __name__ == "__main__":
    if len(sys.argv) > 1:
        parts = sys.argv[1].split('-')
        if len(parts) == 3:
            target = date(int(parts[0]), int(parts[1]), int(parts[2]))
        else:
            parts = sys.argv[1].split('/')
            target = date(2026, int(parts[0]), int(parts[1]))
    else:
        target = date.today()

    asyncio.run(full_report(target))
