#!/usr/bin/env python3
"""Run the hot wash report for a specific date."""

import asyncio
import sys
from datetime import date, timedelta
from dotenv import load_dotenv
load_dotenv()

import logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

from services.ml_analyzer.analyzer import MLAnalyzer


async def run_hotwash(target_date: date):
    """Run the hot wash analysis for a specific date."""
    analyzer = MLAnalyzer()
    await analyzer.run_nightly_analysis(target_date)


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

    print(f"Running hot wash report for {target}...")
    asyncio.run(run_hotwash(target))
