#!/usr/bin/env python3
"""Run anomaly detection on today's data (non-destructive)."""

import asyncio
from datetime import date
from dotenv import load_dotenv
load_dotenv()

from arbees_shared.db.connection import get_pool, close_pool
from services.ml_analyzer.anomaly_detector import AnomalyDetector


async def main():
    pool = await get_pool()
    today = date.today()

    print(f"ANOMALY DETECTION FOR {today}")
    print("="*80)

    # Get all trades for today
    trades = await pool.fetch("""
        SELECT * FROM paper_trades
        WHERE DATE(entry_time) = $1 OR DATE(exit_time) = $1
        ORDER BY entry_time
    """, today)

    if not trades:
        print("No trades found for today.")
        await close_pool()
        return

    # Convert to list of dicts
    trade_list = [dict(t) for t in trades]
    print(f"Analyzing {len(trade_list)} trades...")

    # Run anomaly detection
    detector = AnomalyDetector()
    report = detector.analyze(trade_list, today)

    # Print results
    print(detector.format_report(report))

    # Show summary
    print("\n" + "="*80)
    print("SUMMARY")
    print("="*80)
    print(f"Total anomalies: {len(report.anomalies)}")
    print(f"Critical: {report.critical_count}")
    print(f"Warning: {report.warning_count}")

    if report.anomalies:
        print("\nDetails of each anomaly:")
        for i, a in enumerate(report.anomalies, 1):
            print(f"\n{i}. [{a.severity.upper()}] {a.title}")
            print(f"   Type: {a.anomaly_type}")
            for k, v in a.details.items():
                print(f"   {k}: {v}")

    await close_pool()


if __name__ == "__main__":
    asyncio.run(main())
