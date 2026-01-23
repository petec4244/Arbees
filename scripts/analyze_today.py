#!/usr/bin/env python3
"""Analyze today's suspicious trading data."""

import asyncio
from datetime import date
from collections import Counter
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def analyze_today():
    pool = await get_pool()
    today = date.today()

    print(f'DETAILED ANALYSIS FOR {today}')
    print('='*100)

    # Get all trades
    trades = await pool.fetch("""
        SELECT * FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        ORDER BY entry_time
    """, today)

    print(f'Total trades: {len(trades)}')

    # Check PnL calculation
    print()
    print('SAMPLE TRADES WITH PNL CHECK:')
    print('-'*100)

    wrong_count = 0
    for t in list(trades)[:50]:
        entry = float(t['entry_price'])
        exit_p = float(t['exit_price']) if t['exit_price'] else 0
        size = float(t['size'])
        recorded_pnl = float(t['pnl'] or 0)
        side = t['side']

        # Expected PnL
        if side == 'buy':
            expected_pnl = (exit_p - entry) * size
        else:  # sell
            expected_pnl = (entry - exit_p) * size

        diff = abs(recorded_pnl - expected_pnl)
        if diff > 0.01:
            wrong_count += 1
            print(f'{t["trade_id"][:8]} {str(side):4} entry={entry:.3f} exit={exit_p:.3f} '
                  f'size=${size:.2f} recorded=${recorded_pnl:+.2f} expected=${expected_pnl:+.2f} '
                  f'DIFF=${diff:.2f}')

    print(f'\nWrong PnL calculations: {wrong_count} out of {min(len(trades), 50)} checked')

    # Check position sizes
    print()
    print('POSITION SIZE DISTRIBUTION:')
    print('-'*100)

    sizes = [float(t['size']) for t in trades]
    print(f'Min: ${min(sizes):.2f}')
    print(f'Max: ${max(sizes):.2f}')
    print(f'Avg: ${sum(sizes)/len(sizes):.2f}')

    # Look at the biggest positions
    print('\nLargest positions:')
    sorted_by_size = sorted(trades, key=lambda x: float(x['size']), reverse=True)
    for t in sorted_by_size[:10]:
        print(f'  ${float(t["size"]):.2f} - {t["market_title"][:60]}')

    # Check if there are duplicate trades
    print()
    print('CHECKING FOR DUPLICATES:')
    print('-'*100)

    trade_ids = [t['trade_id'] for t in trades]
    unique_ids = set(trade_ids)
    print(f'Total trades: {len(trade_ids)}, Unique IDs: {len(unique_ids)}')
    if len(trade_ids) != len(unique_ids):
        print('[!] DUPLICATE TRADE IDS FOUND!')
        dupes = [tid for tid, cnt in Counter(trade_ids).items() if cnt > 1]
        for d in dupes[:10]:
            print(f'  Duplicate: {d}')

    # Check by game
    print()
    print('TRADES PER GAME:')
    print('-'*100)

    games = await pool.fetch("""
        SELECT
            game_id,
            market_title,
            COUNT(*) as trades,
            SUM(size) as volume,
            SUM(pnl) as pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        GROUP BY game_id, market_title
        ORDER BY volume DESC
        LIMIT 20
    """, today)

    for g in games:
        total = (g['wins'] or 0) + (g['losses'] or 0)
        wr = (g['wins'] / total * 100) if total > 0 else 0
        title = (g['market_title'] or 'Unknown')[:50]
        print(f'{g["game_id"][:8]}... {g["trades"]:3} trades ${float(g["volume"]):>10,.2f} vol '
              f'${float(g["pnl"] or 0):>8,.2f} pnl {wr:5.1f}% WR | {title}')

    # Check the entry prices
    print()
    print('ENTRY PRICE DISTRIBUTION:')
    print('-'*100)

    entry_prices = [float(t['entry_price']) for t in trades]
    exit_prices = [float(t['exit_price']) for t in trades if t['exit_price']]

    print(f'Entry prices - Min: {min(entry_prices):.3f}, Max: {max(entry_prices):.3f}, Avg: {sum(entry_prices)/len(entry_prices):.3f}')
    if exit_prices:
        print(f'Exit prices  - Min: {min(exit_prices):.3f}, Max: {max(exit_prices):.3f}, Avg: {sum(exit_prices)/len(exit_prices):.3f}')

    # Check for 1.0 or 0.0 settlements
    perfect_wins = [t for t in trades if t['exit_price'] and float(t['exit_price']) == 1.0]
    perfect_losses = [t for t in trades if t['exit_price'] and float(t['exit_price']) == 0.0]
    print(f'\nPerfect settlement wins (exit=1.0): {len(perfect_wins)}')
    print(f'Perfect settlement losses (exit=0.0): {len(perfect_losses)}')

    # Show some perfect wins
    if perfect_wins:
        print('\nSample perfect wins:')
        for t in perfect_wins[:5]:
            entry = float(t['entry_price'])
            pnl = float(t['pnl'] or 0)
            size = float(t['size'])
            print(f'  {t["trade_id"][:8]} {t["side"]:4} entry={entry:.3f} size=${size:.2f} pnl=${pnl:+.2f}')

    await close_pool()


if __name__ == "__main__":
    asyncio.run(analyze_today())
