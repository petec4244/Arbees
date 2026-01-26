#!/usr/bin/env python3
"""Investigate missing games in trade reports."""

import asyncio
from datetime import date
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool


async def investigate(target_date: date):
    pool = await get_pool()

    print('='*80)
    print(f'INVESTIGATION: Missing games for {target_date}')
    print('='*80)

    # 1. Check all trades for that day
    trades = await pool.fetch("""
        SELECT DISTINCT game_id, market_title, sport, COUNT(*) as trade_count, SUM(pnl) as total_pnl
        FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        GROUP BY game_id, market_title, sport
        ORDER BY sport, game_id
    """, target_date)

    print(f'\n1. TRADES BY GAME (from paper_trades):')
    print('-'*80)
    games_with_trades = set()
    for t in trades:
        games_with_trades.add(t['game_id'])
        print(f"  {t['sport']:5} {t['game_id'][:12]}... {t['trade_count']:3} trades  ${float(t['total_pnl'] or 0):>8.2f} pnl  {t['market_title'][:40]}")
    print(f'Total unique games with trades: {len(games_with_trades)}')

    # 2. Check game_states for that day - what games did we monitor?
    game_states = await pool.fetch("""
        SELECT DISTINCT game_id, sport,
            MIN(time) as first_seen,
            MAX(time) as last_seen,
            COUNT(*) as state_count
        FROM game_states
        WHERE DATE(time) = $1
        GROUP BY game_id, sport
        ORDER BY sport, game_id
    """, target_date)

    print(f'\n2. GAMES MONITORED (from game_states):')
    print('-'*80)
    games_monitored = set()
    for g in game_states:
        games_monitored.add(g['game_id'])
        had_trades = '[HAS TRADES]' if g['game_id'] in games_with_trades else '[NO TRADES]'
        print(f"  {g['sport']:5} {g['game_id'][:12]}... {g['state_count']:5} states  {had_trades}")
    print(f'Total games monitored: {len(games_monitored)}')

    # 3. Games monitored but no trades
    missing = games_monitored - games_with_trades
    print(f'\n3. GAMES MONITORED BUT NO TRADES: {len(missing)}')
    print('-'*80)
    for gid in list(missing)[:20]:
        print(f'  {gid}')

    # 4. Check signals for that day
    signals = await pool.fetch("""
        SELECT DISTINCT game_id, team, direction, COUNT(*) as signal_count
        FROM trading_signals
        WHERE DATE(time) = $1
        GROUP BY game_id, team, direction
        ORDER BY game_id
    """, target_date)

    print(f'\n4. SIGNALS GENERATED (from trading_signals):')
    print('-'*80)
    games_with_signals = set()
    for s in signals:
        games_with_signals.add(s['game_id'])
        had_trades = '[Y]' if s['game_id'] in games_with_trades else '[N]'
        print(f"  {s['game_id'][:12]}... {s['signal_count']:3} signals  {s['direction']:4}  {had_trades}  {s['team'][:30]}")
    print(f'Total games with signals: {len(games_with_signals)}')

    # 5. Check market_prices for that day
    prices = await pool.fetch("""
        SELECT DISTINCT market_id,
            COUNT(*) as price_count,
            MIN(time) as first_price,
            MAX(time) as last_price
        FROM market_prices
        WHERE DATE(time) = $1
        GROUP BY market_id
        ORDER BY price_count DESC
        LIMIT 30
    """, target_date)

    print(f'\n5. MARKET PRICES RECORDED (from market_prices):')
    print('-'*80)
    print(f'Total markets with prices: {len(prices)}')
    for p in prices[:15]:
        print(f"  {p['market_id'][:50]}... {p['price_count']:5} prices")
    if len(prices) > 15:
        print(f'  ... and {len(prices) - 15} more markets')

    # 6. Check for signals that didn't result in trades
    print(f'\n6. SIGNALS WITHOUT TRADES:')
    print('-'*80)
    signals_no_trades = await pool.fetch("""
        SELECT s.game_id, s.team, s.direction, s.edge_pct, s.model_prob, s.market_prob,
               s.time, s.signal_type
        FROM trading_signals s
        LEFT JOIN paper_trades t ON s.game_id = t.game_id
            AND (s.team = t.market_title OR s.team LIKE '%' || t.market_title || '%')
        WHERE DATE(s.time) = $1
        AND t.trade_id IS NULL
        ORDER BY s.time
        LIMIT 30
    """, target_date)

    print(f'Signals with NO corresponding trades: {len(signals_no_trades)}')
    for s in signals_no_trades[:15]:
        print(f"  {s['game_id'][:12]}... {s['direction']:4} edge={float(s['edge_pct'] or 0):5.1f}% "
              f"model={float(s['model_prob'] or 0):.3f} market={float(s['market_prob'] or 0):.3f} "
              f"{s['team'][:25]}")

    # 7. Check what sports/leagues had games that day
    print(f'\n7. SPORTS BREAKDOWN:')
    print('-'*80)
    sports = await pool.fetch("""
        SELECT sport, COUNT(DISTINCT game_id) as game_count
        FROM game_states
        WHERE DATE(time) = $1
        GROUP BY sport
        ORDER BY game_count DESC
    """, target_date)

    for sp in sports:
        print(f"  {sp['sport']:10} {sp['game_count']:3} games monitored")

    # 8. Check execution service logs / errors
    print(f'\n8. CHECKING EXECUTION METRICS:')
    print('-'*80)

    # Count trades per hour to see if there were outages
    hourly = await pool.fetch("""
        SELECT DATE_TRUNC('hour', entry_time) as hour, COUNT(*) as trades
        FROM paper_trades
        WHERE DATE(entry_time) = $1
        GROUP BY DATE_TRUNC('hour', entry_time)
        ORDER BY hour
    """, target_date)

    print('Trades per hour:')
    for h in hourly:
        print(f"  {h['hour'].strftime('%H:%M')}: {h['trades']} trades")

    # Check for markets that had prices but no signals
    print(f'\n9. MARKETS WITH PRICES BUT NO SIGNALS:')
    print('-'*80)

    markets_no_signals = await pool.fetch("""
        SELECT DISTINCT mp.market_id, COUNT(*) as price_count
        FROM market_prices mp
        LEFT JOIN trading_signals ts ON mp.market_id LIKE '%' || ts.game_id || '%'
        WHERE DATE(mp.time) = $1
        AND ts.signal_id IS NULL
        GROUP BY mp.market_id
        ORDER BY price_count DESC
        LIMIT 20
    """, target_date)

    print(f'Markets with prices but no signals: {len(markets_no_signals)}')
    for m in markets_no_signals[:10]:
        print(f"  {m['market_id'][:60]}... {m['price_count']} prices")

    # 10. Compare with surrounding days
    print(f'\n10. COMPARISON WITH SURROUNDING DAYS:')
    print('-'*80)

    from datetime import timedelta
    for delta in [-2, -1, 0, 1, 2]:
        check_date = target_date + timedelta(days=delta)
        counts = await pool.fetchrow("""
            SELECT
                (SELECT COUNT(DISTINCT game_id) FROM game_states WHERE DATE(time) = $1) as games,
                (SELECT COUNT(*) FROM paper_trades WHERE DATE(entry_time) = $1) as trades,
                (SELECT COUNT(DISTINCT game_id) FROM trading_signals WHERE DATE(time) = $1) as signal_games,
                (SELECT COUNT(DISTINCT market_id) FROM market_prices WHERE DATE(time) = $1) as markets
        """, check_date)
        marker = " <-- TARGET" if delta == 0 else ""
        print(f"  {check_date}: {counts['games']:3} games, {counts['trades']:4} trades, "
              f"{counts['signal_games']:3} signal_games, {counts['markets']:3} markets{marker}")

    await close_pool()


if __name__ == "__main__":
    import sys
    if len(sys.argv) > 1:
        # Parse date from argument
        parts = sys.argv[1].split('-')
        if len(parts) == 3:
            target = date(int(parts[0]), int(parts[1]), int(parts[2]))
        else:
            # Try MM/DD format
            parts = sys.argv[1].split('/')
            target = date(2026, int(parts[0]), int(parts[1]))
    else:
        target = date(2026, 1, 25)

    asyncio.run(investigate(target))
