#!/usr/bin/env python3
"""Analyze the Hawaii game trades specifically."""

import asyncio
from datetime import date
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def main():
    pool = await get_pool()
    today = date.today()

    # Get all Hawaii trades
    trades = await pool.fetch("""
        SELECT * FROM paper_trades
        WHERE market_title LIKE '%Hawaii%' OR market_title LIKE '%Hawai%'
        ORDER BY entry_time
    """)

    print(f'Hawaii trades: {len(trades)}')
    print('='*150)
    print(f'{"Time":<20} | {"Side":<4} | {"Entry":>6} {"Exit":>6} | {"Size":>12} | '
          f'{"Edge":>6} {"Kelly":>5} | {"PnL":>10} | {"Outcome":<6}')
    print('-'*150)

    running_pnl = 0
    for t in trades:
        entry = float(t['entry_price'])
        exit_p = float(t['exit_price']) if t['exit_price'] else 0
        size = float(t['size'])
        pnl = float(t['pnl'] or 0)
        edge = float(t['edge_at_entry'] or 0)
        kelly = float(t['kelly_fraction'] or 0)
        running_pnl += pnl

        time_str = t['entry_time'].strftime('%H:%M:%S') if t['entry_time'] else ''
        print(f'{time_str:<20} | {t["side"]:4} | {entry:>6.3f} {exit_p:>6.3f} | '
              f'${size:>10,.2f} | {edge:>5.1f}% {kelly:>5.2f} | '
              f'${pnl:>9,.2f} | {t["outcome"] or "open":<6} (running: ${running_pnl:,.2f})')

    print('-'*150)
    print(f'Total PnL: ${running_pnl:,.2f}')

    # Check the model_prob vs market price
    print()
    print('MODEL PROB VS MARKET PRICE:')
    print('='*100)
    for t in trades[:5]:
        model_prob = float(t['model_prob'] or 0)
        entry = float(t['entry_price'])
        edge = float(t['edge_at_entry'] or 0)
        print(f'  Model prob: {model_prob:.3f}, Entry price: {entry:.3f}, Edge: {edge:.1f}%')

    await close_pool()


if __name__ == "__main__":
    asyncio.run(main())
