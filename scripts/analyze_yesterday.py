#!/usr/bin/env python3
"""Quick script to run ML analysis on yesterday's trading data."""

import asyncio
import logging
import os
from datetime import date, datetime, timedelta
from decimal import Decimal

# Setup logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)

# Load env from .env if available
from dotenv import load_dotenv
load_dotenv()


async def query_raw_trades():
    """Query raw trade data from database."""
    from arbees_shared.db.connection import get_pool, close_pool

    pool = await get_pool()
    yesterday = date.today() - timedelta(days=1)

    print("\n" + "="*80)
    print(f"RAW TRADE DATA ANALYSIS FOR {yesterday}")
    print("="*80)

    # Get all paper trades from yesterday
    trades = await pool.fetch("""
        SELECT
            trade_id, game_id, sport, side,
            entry_price, exit_price, size,
            entry_time, exit_time,
            status, outcome, pnl, pnl_pct,
            model_prob, edge_at_entry, market_title
        FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        ORDER BY entry_time ASC
    """, yesterday)

    if not trades:
        print(f"\nNo trades found for {yesterday}")

        # Check what dates have trades
        dates = await pool.fetch("""
            SELECT DATE(entry_time) as d, COUNT(*) as cnt, SUM(size) as total_volume
            FROM paper_trades
            GROUP BY DATE(entry_time)
            ORDER BY d DESC
            LIMIT 10
        """)
        print("\nRecent trading dates:")
        for row in dates:
            print(f"  {row['d']}: {row['cnt']} trades, ${row['total_volume']:.2f} volume")
        return

    print(f"\nTotal trades found: {len(trades)}")

    # Calculate summary stats
    total_volume = sum(float(t['size']) for t in trades)
    total_pnl = sum(float(t['pnl'] or 0) for t in trades if t['pnl'] is not None)

    closed_trades = [t for t in trades if t['status'] == 'closed']
    wins = sum(1 for t in closed_trades if t['outcome'] == 'win')
    losses = sum(1 for t in closed_trades if t['outcome'] == 'loss')
    pushes = sum(1 for t in closed_trades if t['outcome'] == 'push')

    print(f"\nSUMMARY:")
    print(f"  Total Volume: ${total_volume:,.2f}")
    print(f"  Total PnL: ${total_pnl:,.2f}")
    print(f"  Closed Trades: {len(closed_trades)}")
    print(f"  Wins/Losses/Pushes: {wins}/{losses}/{pushes}")
    if closed_trades:
        print(f"  Win Rate: {wins/len(closed_trades)*100:.1f}%")

    # Check for suspicious patterns
    print("\n" + "-"*80)
    print("CHECKING FOR ANOMALIES...")
    print("-"*80)

    # 1. Check for very large position sizes
    large_positions = [t for t in trades if float(t['size']) > 100]
    if large_positions:
        print(f"\n[!] Large positions (>$100):")
        for t in large_positions[:10]:
            print(f"  {t['trade_id'][:8]}... size=${t['size']:.2f} "
                  f"entry={t['entry_price']:.3f} exit={t['exit_price']} "
                  f"pnl=${t['pnl'] or 0:.2f}")

    # 2. Check for weird entry/exit prices
    weird_prices = []
    for t in trades:
        entry = float(t['entry_price'])
        exit_p = float(t['exit_price']) if t['exit_price'] is not None else None
        if entry <= 0 or entry >= 1:
            weird_prices.append(('entry', t, entry))
        if exit_p is not None and (exit_p < 0 or exit_p > 1):
            weird_prices.append(('exit', t, exit_p))

    if weird_prices:
        print(f"\n[!] Weird prices found:")
        for price_type, t, price in weird_prices[:10]:
            print(f"  {t['trade_id'][:8]}... {price_type}={price:.3f}")

    # 3. Check PnL calculations
    print("\n" + "-"*80)
    print("PNL CALCULATION CHECK (sample of closed trades):")
    print("-"*80)

    for t in closed_trades[:20]:
        entry = float(t['entry_price'])
        exit_p = float(t['exit_price']) if t['exit_price'] else 0
        size = float(t['size'])
        recorded_pnl = float(t['pnl'] or 0)
        side = t['side']

        # Expected PnL calculation
        if side == 'buy':
            expected_pnl = (exit_p - entry) * size
        else:  # sell
            expected_pnl = (entry - exit_p) * size

        diff = abs(recorded_pnl - expected_pnl)
        status = "[OK]" if diff < 0.01 else f"[!] DIFF={diff:.2f}"

        print(f"  {t['trade_id'][:8]}... {side:4} entry={entry:.3f} exit={exit_p:.3f} "
              f"size=${size:.2f} pnl=${recorded_pnl:+.2f} expected=${expected_pnl:+.2f} {status}")

    # 4. Group by sport
    print("\n" + "-"*80)
    print("BY SPORT:")
    print("-"*80)

    sports = await pool.fetch("""
        SELECT
            sport,
            COUNT(*) as trades,
            SUM(size) as volume,
            SUM(pnl) as total_pnl,
            SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) as wins,
            SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) as losses
        FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        GROUP BY sport
        ORDER BY volume DESC
    """, yesterday)

    for s in sports:
        total = s['wins'] + s['losses']
        wr = (s['wins'] / total * 100) if total > 0 else 0
        print(f"  {s['sport']:10} trades={s['trades']:3} "
              f"volume=${s['volume']:,.2f} pnl=${s['total_pnl'] or 0:+,.2f} "
              f"wins={s['wins']} losses={s['losses']} wr={wr:.0f}%")

    # 5. Check position sizes vs expected Kelly sizing
    print("\n" + "-"*80)
    print("POSITION SIZE ANALYSIS:")
    print("-"*80)

    sizes = [float(t['size']) for t in trades]
    edges = [float(t['edge_at_entry'] or 0) for t in trades if t['edge_at_entry']]

    print(f"  Min size: ${min(sizes):.2f}")
    print(f"  Max size: ${max(sizes):.2f}")
    print(f"  Avg size: ${sum(sizes)/len(sizes):.2f}")
    print(f"  Median size: ${sorted(sizes)[len(sizes)//2]:.2f}")
    if edges:
        print(f"  Avg edge: {sum(edges)/len(edges):.2f}%")

    # Bankroll check
    bankroll = float(os.environ.get('INITIAL_BANKROLL', 1000))
    max_pct = float(os.environ.get('MAX_POSITION_PCT', 10))
    expected_max = bankroll * max_pct / 100

    print(f"\n  Bankroll: ${bankroll:.2f}")
    print(f"  Max position %: {max_pct}%")
    print(f"  Expected max size: ${expected_max:.2f}")

    oversized = [t for t in trades if float(t['size']) > expected_max * 1.1]
    if oversized:
        print(f"\n  [!] {len(oversized)} trades exceeded max position size!")

    await close_pool()


async def run_ml_analysis():
    """Run the ML analyzer on yesterday's data."""
    from services.ml_analyzer import MLAnalyzer
    from arbees_shared.db.connection import close_pool

    yesterday = date.today() - timedelta(days=1)

    print("\n" + "="*80)
    print(f"ML ANALYZER OUTPUT FOR {yesterday}")
    print("="*80)

    analyzer = MLAnalyzer()

    try:
        insights = await analyzer.run_nightly_analysis(yesterday)

        # Print summary
        print(f"\nTotal Trades: {insights.total_trades}")
        print(f"Win Rate: {insights.win_rate:.1%}")
        print(f"Total P&L: ${insights.total_pnl:.2f}")
        print(f"Avg Edge: {insights.avg_edge:.2%}")

        print("\nBy Sport:")
        for sport, perf in insights.by_sport.items():
            print(f"  {sport}: {perf.trades} trades, {perf.win_rate:.0%} wr, ${perf.pnl:.2f} pnl")

        print("\nBest Trades:")
        for t in insights.best_trades[:5]:
            print(f"  {t.trade_id[:8]}... {t.sport} ${t.pnl:+.2f}")

        print("\nWorst Trades:")
        for t in insights.worst_trades[:5]:
            print(f"  {t.trade_id[:8]}... {t.sport} ${t.pnl:+.2f}")

        if insights.recommendations:
            print("\nRecommendations:")
            for rec in insights.recommendations:
                print(f"  - {rec.title}: {rec.rationale}")

    except Exception as e:
        logger.error(f"ML Analysis failed: {e}", exc_info=True)
    finally:
        await close_pool()


async def main():
    """Run both analyses."""
    await query_raw_trades()
    print("\n\n")
    await run_ml_analysis()


if __name__ == "__main__":
    asyncio.run(main())
