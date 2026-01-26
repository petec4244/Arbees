#!/usr/bin/env python3
"""Check trading volume by day."""

import asyncio
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def check_volume():
    pool = await get_pool()
    target_date = date(2026, 1, 25)

    print('='*80)
    print('TRADE TIMESTAMPS ON 2026-01-25:')
    print('='*80)

    times = await pool.fetch("""
        SELECT entry_time, market_title, side, size, pnl
        FROM paper_trades
        WHERE DATE(entry_time) = $1
        ORDER BY entry_time
    """, target_date)

    print(f'Total trades: {len(times)}')
    for t in times:
        print(f"  {t['entry_time'].strftime('%H:%M:%S')} {t['side']:4} "
              f"size=${float(t['size']):>6.2f} pnl=${float(t['pnl'] or 0):>+6.2f} "
              f"{t['market_title'][:40]}")

    print('\n' + '='*80)
    print('VOLUME COMPARISON BY DAY (last 7 days):')
    print('='*80)

    for i in range(-7, 1):
        d = date.today() + timedelta(days=i)
        row = await pool.fetchrow("""
            SELECT COUNT(*) as trades, COUNT(DISTINCT game_id) as games
            FROM paper_trades
            WHERE DATE(entry_time) = $1
        """, d)
        day_name = d.strftime('%A')[:3]
        marker = ' <-- TARGET' if d == target_date else ''
        print(f"  {d} ({day_name}): {row['games']:3} games, {row['trades']:4} trades{marker}")

    await close_pool()


if __name__ == "__main__":
    asyncio.run(check_volume())
