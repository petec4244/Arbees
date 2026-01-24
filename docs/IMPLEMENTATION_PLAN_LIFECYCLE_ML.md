# Implementation Plan: Game Lifecycle & ML Performance Analysis

## Executive Summary

This plan implements two critical features for the Arbees trading system:
1. **Game Lifecycle Management** - Auto-archive completed games, clean live dashboard
2. **ML Performance Analysis** - Nightly reports, trade analysis, parameter optimization

Based on codebase analysis, the system already has solid foundations:
- TimescaleDB with hypertables for time-series data
- `trading_performance_daily` continuous aggregate
- Position settlement in `GameShard._settle_game_positions()`
- 30-day retention policy (data purged, not archived)

**Gap:** No archival to separate historical tables; no ML analysis/reporting.

---

## Phase 1: Architecture Analysis Summary

### Current Game Tracking
| Component | How It Works |
|-----------|--------------|
| Discovery | Orchestrator polls ESPN, assigns games to shards |
| Monitoring | GameShard polls every 1-30s based on game state |
| End Detection | `status in ['final', 'complete']` triggers settlement |
| Settlement | `_settle_game_positions()` closes trades, calculates PnL |
| Cleanup | Orchestrator sends `remove_game` command, publishes `games:ended` |

### Current Database Schema
```
Relational Tables:
├── games (game_id, sport, teams, status, final_scores)
├── market_mappings (game_id → market_id, platform, market_type)
└── bankroll (account tracking)

Time-Series Hypertables:
├── game_states (snapshots every 1-30s)
├── plays (individual events)
├── market_prices (price history)
├── trading_signals (generated signals)
├── paper_trades (executed trades)
├── arbitrage_opportunities
└── latency_metrics

Continuous Aggregates:
├── market_prices_hourly (OHLC)
└── trading_performance_daily (P&L by signal/sport)

Retention: 30 days detailed, then purged
```

### Integration Points Identified
1. **GameShard → Archiver**: Redis pub on game end with final state
2. **Archiver → Database**: Archive data + update status
3. **ML Analyzer → Database**: Read historical, write insights
4. **API → Frontend**: Historical games endpoint, ML insights endpoint

---

## Phase 2: Database Schema

### New Archive Tables

```sql
-- ================================================
-- MIGRATION: 014_archive_tables.sql
-- ================================================

-- Archived games with computed summary statistics
CREATE TABLE archived_games (
    archive_id SERIAL PRIMARY KEY,
    game_id VARCHAR(64) NOT NULL UNIQUE,
    sport VARCHAR(20) NOT NULL,
    home_team VARCHAR(100) NOT NULL,
    away_team VARCHAR(100) NOT NULL,
    final_home_score INTEGER NOT NULL,
    final_away_score INTEGER NOT NULL,
    scheduled_time TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Computed summary stats
    total_trades INTEGER NOT NULL DEFAULT 0,
    winning_trades INTEGER NOT NULL DEFAULT 0,
    losing_trades INTEGER NOT NULL DEFAULT 0,
    push_trades INTEGER NOT NULL DEFAULT 0,
    total_pnl DECIMAL(12,2) NOT NULL DEFAULT 0,
    total_signals_generated INTEGER NOT NULL DEFAULT 0,
    total_signals_executed INTEGER NOT NULL DEFAULT 0,
    avg_edge_pct DECIMAL(6,4),

    -- Performance metrics
    win_rate DECIMAL(5,4) GENERATED ALWAYS AS (
        CASE WHEN total_trades > 0
        THEN winning_trades::DECIMAL / total_trades
        ELSE 0 END
    ) STORED,
    capture_rate DECIMAL(5,4) GENERATED ALWAYS AS (
        CASE WHEN total_signals_generated > 0
        THEN total_signals_executed::DECIMAL / total_signals_generated
        ELSE 0 END
    ) STORED
);

CREATE INDEX idx_archived_games_sport ON archived_games(sport);
CREATE INDEX idx_archived_games_ended_at ON archived_games(ended_at DESC);
CREATE INDEX idx_archived_games_pnl ON archived_games(total_pnl DESC);

-- Archived trades (copy from paper_trades)
CREATE TABLE archived_trades (
    archive_trade_id SERIAL PRIMARY KEY,
    trade_id VARCHAR(64) NOT NULL,
    archive_game_id INTEGER REFERENCES archived_games(archive_id),
    game_id VARCHAR(64) NOT NULL,
    signal_id VARCHAR(64),
    signal_type VARCHAR(50),

    -- Trade details
    platform VARCHAR(20) NOT NULL,
    market_id VARCHAR(100),
    market_type VARCHAR(20),
    side VARCHAR(10) NOT NULL,  -- BUY or SELL
    team VARCHAR(100),

    -- Pricing
    entry_price DECIMAL(8,4) NOT NULL,
    exit_price DECIMAL(8,4),
    quantity DECIMAL(12,4) NOT NULL,

    -- Timing
    opened_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,

    -- Outcome
    status VARCHAR(20) NOT NULL,
    outcome VARCHAR(20),
    pnl DECIMAL(12,2),
    pnl_pct DECIMAL(8,4),

    -- Context
    edge_at_entry DECIMAL(6,4),
    model_prob_at_entry DECIMAL(6,4),
    market_prob_at_entry DECIMAL(6,4),
    game_period_at_entry VARCHAR(20),
    score_diff_at_entry INTEGER,
    time_remaining_at_entry INTEGER  -- seconds
);

CREATE INDEX idx_archived_trades_game ON archived_trades(archive_game_id);
CREATE INDEX idx_archived_trades_outcome ON archived_trades(outcome);
CREATE INDEX idx_archived_trades_signal_type ON archived_trades(signal_type);

-- Archived signals (copy from trading_signals)
CREATE TABLE archived_signals (
    archive_signal_id SERIAL PRIMARY KEY,
    signal_id VARCHAR(64) NOT NULL,
    archive_game_id INTEGER REFERENCES archived_games(archive_id),
    game_id VARCHAR(64) NOT NULL,

    -- Signal details
    signal_type VARCHAR(50) NOT NULL,
    direction VARCHAR(10) NOT NULL,
    team VARCHAR(100),
    market_type VARCHAR(20),

    -- Probabilities
    model_prob DECIMAL(6,4) NOT NULL,
    market_prob DECIMAL(6,4) NOT NULL,
    edge_pct DECIMAL(6,4) NOT NULL,
    confidence VARCHAR(20),

    -- Timing
    generated_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,

    -- Execution
    was_executed BOOLEAN NOT NULL DEFAULT FALSE,
    execution_reason VARCHAR(200),  -- why executed or why not

    -- Context
    game_period VARCHAR(20),
    score_diff INTEGER,
    time_remaining INTEGER
);

CREATE INDEX idx_archived_signals_game ON archived_signals(archive_game_id);
CREATE INDEX idx_archived_signals_executed ON archived_signals(was_executed);

-- ML analysis results (nightly reports)
CREATE TABLE ml_analysis_reports (
    report_id SERIAL PRIMARY KEY,
    report_date DATE NOT NULL UNIQUE,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Summary stats
    total_games INTEGER NOT NULL,
    total_trades INTEGER NOT NULL,
    total_pnl DECIMAL(12,2) NOT NULL,
    win_rate DECIMAL(5,4) NOT NULL,

    -- Top performers
    best_sport VARCHAR(20),
    best_sport_win_rate DECIMAL(5,4),
    best_market_type VARCHAR(20),
    best_market_type_win_rate DECIMAL(5,4),
    best_edge_range VARCHAR(20),  -- e.g., "2.5%-3.5%"

    -- Weaknesses
    worst_sport VARCHAR(20),
    worst_sport_win_rate DECIMAL(5,4),
    worst_market_type VARCHAR(20),
    worst_market_type_win_rate DECIMAL(5,4),

    -- Opportunities
    signals_generated INTEGER NOT NULL,
    signals_executed INTEGER NOT NULL,
    missed_opportunity_reasons JSONB,  -- { "too_slow": 45, "edge_threshold": 22, ... }

    -- Recommendations
    recommendations JSONB NOT NULL,  -- Array of { type, current, recommended, impact }

    -- Model metrics
    model_accuracy DECIMAL(5,4),
    feature_importance JSONB,

    -- Full report content
    report_markdown TEXT NOT NULL,
    report_html TEXT
);

CREATE INDEX idx_ml_reports_date ON ml_analysis_reports(report_date DESC);

-- Parameter history (track changes over time)
CREATE TABLE parameter_history (
    history_id SERIAL PRIMARY KEY,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    parameter_name VARCHAR(100) NOT NULL,
    old_value DECIMAL(12,4),
    new_value DECIMAL(12,4) NOT NULL,
    change_reason VARCHAR(200),
    recommended_by VARCHAR(50),  -- 'ML' or 'manual'
    approved_by VARCHAR(100),
    applied BOOLEAN NOT NULL DEFAULT FALSE
);

-- Update games table to track archive status
ALTER TABLE games ADD COLUMN IF NOT EXISTS archived BOOLEAN DEFAULT FALSE;
ALTER TABLE games ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
```

### Indexes for Performance

```sql
-- Query patterns for historical games page
CREATE INDEX idx_archived_games_filter ON archived_games(sport, ended_at DESC)
    WHERE total_pnl > 0;  -- Partial index for profitable games
CREATE INDEX idx_archived_games_losing ON archived_games(sport, ended_at DESC)
    WHERE total_pnl < 0;  -- Partial index for losing games

-- Query patterns for ML analysis
CREATE INDEX idx_archived_trades_ml ON archived_trades(
    signal_type, outcome, game_period_at_entry, edge_at_entry
);
```

---

## Phase 3: Service Architecture

### 3.1 Game Archiver Service

**Location:** `services/archiver/`

```
services/archiver/
├── __init__.py
├── archiver.py          # Main service
├── archive_worker.py    # Archive logic
└── config.py            # Configuration
```

**Architecture Diagram:**
```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  GameShard  │────▶│    Redis    │────▶│   Archiver  │
│             │     │  games:ended│     │   Service   │
└─────────────┘     └─────────────┘     └─────────────┘
                                               │
                                               ▼
                                        ┌─────────────┐
                                        │ TimescaleDB │
                                        │  Archive    │
                                        │  Tables     │
                                        └─────────────┘
```

**Implementation:**

```python
# services/archiver/archiver.py

from dataclasses import dataclass
from datetime import datetime, timedelta
import asyncio
import json
from typing import Optional

from arbees_shared.db import DatabaseClient
from arbees_shared.messaging import RedisClient
from arbees_shared.logging import get_logger

logger = get_logger(__name__)

@dataclass
class ArchiverConfig:
    grace_period_minutes: int = 60  # Wait after game ends
    archive_batch_size: int = 10
    poll_interval_seconds: int = 300  # 5 minutes

class GameArchiver:
    """
    Archives completed games to historical tables.

    Workflow:
    1. Listen for 'games:ended' events from Redis
    2. Wait grace period (for score corrections)
    3. Copy data to archive tables
    4. Mark original as archived
    5. Compute summary statistics
    """

    def __init__(
        self,
        db: DatabaseClient,
        redis: RedisClient,
        config: Optional[ArchiverConfig] = None
    ):
        self.db = db
        self.redis = redis
        self.config = config or ArchiverConfig()
        self._running = False
        self._pending_archives: dict[str, datetime] = {}  # game_id -> end_time

    async def start(self):
        """Start the archiver service."""
        self._running = True
        logger.info("Starting GameArchiver service")

        # Subscribe to game ended events
        await self.redis.subscribe("games:ended", self._on_game_ended)

        # Start polling loop for grace period check
        asyncio.create_task(self._archive_loop())

    async def stop(self):
        """Stop the archiver service."""
        self._running = False
        await self.redis.unsubscribe("games:ended")
        logger.info("GameArchiver stopped")

    async def _on_game_ended(self, message: dict):
        """Handle game ended event from Redis."""
        game_id = message.get("game_id")
        if not game_id:
            return

        logger.info(f"Game ended: {game_id}, queuing for archive")
        self._pending_archives[game_id] = datetime.utcnow()

    async def _archive_loop(self):
        """Poll for games ready to archive (past grace period)."""
        while self._running:
            try:
                await self._process_ready_games()
            except Exception as e:
                logger.error(f"Archive loop error: {e}")

            await asyncio.sleep(self.config.poll_interval_seconds)

    async def _process_ready_games(self):
        """Archive games that have passed the grace period."""
        grace_cutoff = datetime.utcnow() - timedelta(
            minutes=self.config.grace_period_minutes
        )

        ready_games = [
            game_id for game_id, end_time in self._pending_archives.items()
            if end_time < grace_cutoff
        ]

        for game_id in ready_games[:self.config.archive_batch_size]:
            try:
                await self._archive_game(game_id)
                del self._pending_archives[game_id]
                logger.info(f"Archived game: {game_id}")
            except Exception as e:
                logger.error(f"Failed to archive {game_id}: {e}")

    async def _archive_game(self, game_id: str):
        """
        Archive a single game with all its data.
        Uses a transaction to ensure atomicity.
        """
        async with self.db.transaction():
            # 1. Get game details
            game = await self._get_game(game_id)
            if not game or game.get("archived"):
                return

            # 2. Get all trades for this game
            trades = await self._get_game_trades(game_id)

            # 3. Get all signals for this game
            signals = await self._get_game_signals(game_id)

            # 4. Compute summary statistics
            stats = self._compute_stats(trades, signals)

            # 5. Insert into archived_games
            archive_id = await self._insert_archived_game(game, stats)

            # 6. Insert archived trades
            await self._insert_archived_trades(archive_id, trades)

            # 7. Insert archived signals
            await self._insert_archived_signals(archive_id, signals)

            # 8. Mark original game as archived
            await self._mark_game_archived(game_id)

    def _compute_stats(self, trades: list, signals: list) -> dict:
        """Compute summary statistics for archived game."""
        winning = sum(1 for t in trades if t.get("outcome") == "WIN")
        losing = sum(1 for t in trades if t.get("outcome") == "LOSS")
        push = sum(1 for t in trades if t.get("outcome") == "PUSH")
        total_pnl = sum(t.get("pnl", 0) or 0 for t in trades)
        executed = sum(1 for s in signals if s.get("was_executed"))

        edges = [t.get("edge_at_entry") for t in trades if t.get("edge_at_entry")]
        avg_edge = sum(edges) / len(edges) if edges else None

        return {
            "total_trades": len(trades),
            "winning_trades": winning,
            "losing_trades": losing,
            "push_trades": push,
            "total_pnl": total_pnl,
            "total_signals_generated": len(signals),
            "total_signals_executed": executed,
            "avg_edge_pct": avg_edge,
        }

    # Database methods (implementations abbreviated)
    async def _get_game(self, game_id: str) -> Optional[dict]: ...
    async def _get_game_trades(self, game_id: str) -> list: ...
    async def _get_game_signals(self, game_id: str) -> list: ...
    async def _insert_archived_game(self, game: dict, stats: dict) -> int: ...
    async def _insert_archived_trades(self, archive_id: int, trades: list): ...
    async def _insert_archived_signals(self, archive_id: int, signals: list): ...
    async def _mark_game_archived(self, game_id: str): ...
```

### 3.2 ML Analyzer Service

**Location:** `services/ml_analyzer/`

```
services/ml_analyzer/
├── __init__.py
├── analyzer.py           # Main service / scheduler
├── feature_extractor.py  # Extract ML features from trades
├── models/
│   ├── __init__.py
│   ├── trade_success.py  # Trade success classifier
│   ├── pnl_predictor.py  # P&L regression
│   └── optimizer.py      # Parameter optimizer
├── report_generator.py   # Hot wash report generation
├── insights.py           # Insight extraction
└── config.py
```

**Architecture Diagram:**
```
                    ┌─────────────────┐
                    │  Scheduler      │
                    │  (11pm daily)   │
                    └────────┬────────┘
                             │
                             ▼
┌─────────────┐     ┌─────────────────┐     ┌─────────────┐
│ TimescaleDB │────▶│   ML Analyzer   │────▶│   Reports   │
│  Archives   │     │                 │     │  (MD/HTML)  │
└─────────────┘     └─────────────────┘     └─────────────┘
       ▲                     │
       │                     ▼
       │            ┌─────────────────┐
       │            │   ML Models     │
       │            │  (scikit-learn) │
       │            └─────────────────┘
       │                     │
       └─────────────────────┘
              (Save insights)
```

**Implementation:**

```python
# services/ml_analyzer/analyzer.py

from dataclasses import dataclass
from datetime import datetime, date, timedelta
from typing import Optional
import asyncio
import schedule

from arbees_shared.db import DatabaseClient
from arbees_shared.logging import get_logger
from .feature_extractor import FeatureExtractor
from .models.trade_success import TradeSuccessModel
from .models.optimizer import ParameterOptimizer
from .report_generator import HotWashReportGenerator
from .insights import InsightExtractor

logger = get_logger(__name__)

@dataclass
class AnalyzerConfig:
    run_time: str = "23:00"  # 11pm daily
    min_trades_for_ml: int = 100  # Minimum trades to train models
    lookback_days: int = 30
    model_retrain_interval_days: int = 7

class MLAnalyzer:
    """
    Analyzes trading performance and generates insights.

    Runs nightly at 11pm (after markets close).
    """

    def __init__(
        self,
        db: DatabaseClient,
        config: Optional[AnalyzerConfig] = None
    ):
        self.db = db
        self.config = config or AnalyzerConfig()
        self.feature_extractor = FeatureExtractor()
        self.success_model = TradeSuccessModel()
        self.optimizer = ParameterOptimizer()
        self.report_generator = HotWashReportGenerator()
        self.insight_extractor = InsightExtractor()
        self._running = False

    async def start(self):
        """Start the ML analyzer with scheduled runs."""
        self._running = True
        logger.info(f"ML Analyzer starting, scheduled for {self.config.run_time}")

        # Schedule nightly run
        schedule.every().day.at(self.config.run_time).do(
            lambda: asyncio.create_task(self.run_nightly_analysis())
        )

        # Run scheduler loop
        while self._running:
            schedule.run_pending()
            await asyncio.sleep(60)

    async def run_nightly_analysis(self, for_date: Optional[date] = None):
        """
        Full analysis pipeline:
        1. Load today's trades + signals
        2. Extract features
        3. Update ML models (if enough data)
        4. Generate insights
        5. Create hot wash report
        6. Save to database
        """
        analysis_date = for_date or date.today()
        logger.info(f"Starting nightly analysis for {analysis_date}")

        try:
            # 1. Load data
            trades = await self._load_trades(analysis_date)
            signals = await self._load_signals(analysis_date)
            historical_trades = await self._load_historical_trades()

            if not trades:
                logger.info(f"No trades for {analysis_date}, skipping")
                return

            # 2. Extract features
            features_df = self.feature_extractor.extract(trades, signals)

            # 3. Update ML models if enough data
            if len(historical_trades) >= self.config.min_trades_for_ml:
                await self._update_models(historical_trades)

            # 4. Generate insights
            insights = self.insight_extractor.analyze(
                trades=trades,
                signals=signals,
                features=features_df,
                success_model=self.success_model,
            )

            # 5. Generate recommendations
            recommendations = self.optimizer.generate_recommendations(
                historical_trades=historical_trades,
                current_params=await self._get_current_params(),
            )
            insights.recommendations = recommendations

            # 6. Create report
            report_md = self.report_generator.generate_markdown(
                date=analysis_date,
                insights=insights,
            )
            report_html = self.report_generator.generate_html(
                date=analysis_date,
                insights=insights,
            )

            # 7. Save to database
            await self._save_report(analysis_date, insights, report_md, report_html)

            # 8. Deliver report
            await self._deliver_report(report_md)

            logger.info(f"Nightly analysis complete for {analysis_date}")

        except Exception as e:
            logger.error(f"Nightly analysis failed: {e}", exc_info=True)
            raise

    async def run_on_demand(self, for_date: date) -> str:
        """Run analysis on-demand for a specific date."""
        await self.run_nightly_analysis(for_date)
        report = await self._get_report(for_date)
        return report.get("report_markdown", "")

    async def _update_models(self, historical_trades: list):
        """Retrain ML models on historical data."""
        features_df = self.feature_extractor.extract_historical(historical_trades)

        # Train trade success model
        self.success_model.train(features_df)

        # Log feature importance
        importance = self.success_model.feature_importance
        logger.info(f"Model updated. Top features: {list(importance.items())[:5]}")

    async def _deliver_report(self, report_md: str):
        """Deliver report via configured channels."""
        # TODO: Implement email/Slack delivery
        # For now, just save to file
        filename = f"reports/hot_wash_{date.today()}.md"
        with open(filename, "w") as f:
            f.write(report_md)
        logger.info(f"Report saved to {filename}")

    # Database methods
    async def _load_trades(self, for_date: date) -> list: ...
    async def _load_signals(self, for_date: date) -> list: ...
    async def _load_historical_trades(self) -> list: ...
    async def _get_current_params(self) -> dict: ...
    async def _save_report(self, for_date: date, insights, md: str, html: str): ...
    async def _get_report(self, for_date: date) -> dict: ...
```

### 3.3 Feature Extractor

```python
# services/ml_analyzer/feature_extractor.py

import pandas as pd
from typing import List
from arbees_shared.models import PaperTrade, TradingSignal

class FeatureExtractor:
    """Extracts ML features from trades and signals."""

    SPORT_MAP = {"NBA": 0, "NFL": 1, "NHL": 2, "MLB": 3, "NCAAF": 4, "NCAAB": 5}
    MARKET_TYPE_MAP = {"moneyline": 0, "spread": 1, "total": 2}
    PERIOD_MAP = {"Q1": 1, "Q2": 2, "Q3": 3, "Q4": 4, "OT": 5, "1": 1, "2": 2, "3": 3}

    def extract(self, trades: list, signals: list) -> pd.DataFrame:
        """Extract features from today's trades."""
        features = []

        for trade in trades:
            signal = self._find_signal(trade, signals)
            features.append(self._extract_trade_features(trade, signal))

        return pd.DataFrame(features)

    def _extract_trade_features(self, trade: dict, signal: dict = None) -> dict:
        """Extract features for a single trade."""
        return {
            # Trade context
            "sport": self.SPORT_MAP.get(trade.get("sport"), -1),
            "market_type": self.MARKET_TYPE_MAP.get(trade.get("market_type"), -1),
            "edge_size": trade.get("edge_at_entry", 0),
            "time_of_game": self.PERIOD_MAP.get(trade.get("game_period_at_entry"), 0),
            "score_differential": trade.get("score_diff_at_entry", 0),

            # Timing features
            "hour_of_day": pd.to_datetime(trade.get("opened_at")).hour,
            "day_of_week": pd.to_datetime(trade.get("opened_at")).dayofweek,

            # Position features
            "position_size": trade.get("quantity", 0),
            "is_home_team": 1 if "home" in trade.get("team", "").lower() else 0,

            # Signal features
            "model_prob": signal.get("model_prob", 0) if signal else 0,
            "market_prob": signal.get("market_prob", 0) if signal else 0,
            "confidence": self._encode_confidence(signal.get("confidence")) if signal else 0,

            # Target (for training)
            "trade_success": 1 if trade.get("outcome") == "WIN" else 0,
            "pnl": trade.get("pnl", 0),
        }

    def _encode_confidence(self, conf: str) -> int:
        return {"LOW": 1, "MEDIUM": 2, "HIGH": 3, "VERY_HIGH": 4}.get(conf, 0)

    def _find_signal(self, trade: dict, signals: list) -> dict:
        signal_id = trade.get("signal_id")
        for s in signals:
            if s.get("signal_id") == signal_id:
                return s
        return None
```

### 3.4 Report Generator

```python
# services/ml_analyzer/report_generator.py

from dataclasses import dataclass
from datetime import date
from typing import List, Optional

@dataclass
class PerformanceInsights:
    """Container for daily performance insights."""
    date: date
    total_trades: int
    winning_trades: int
    losing_trades: int
    total_pnl: float
    win_rate: float
    avg_edge: float

    # By category
    by_sport: dict  # sport -> {trades, wins, pnl, win_rate}
    by_market_type: dict
    by_edge_range: dict
    by_period: dict

    # Opportunities
    signals_generated: int
    signals_executed: int
    missed_reasons: dict  # reason -> count

    # Top/worst
    best_trades: list
    worst_trades: list

    # Recommendations
    recommendations: list = None

    # Model metrics
    model_accuracy: float = None
    feature_importance: dict = None

class HotWashReportGenerator:
    """Generates nightly hot wash reports."""

    def generate_markdown(self, date: date, insights: PerformanceInsights) -> str:
        """Create markdown report with analysis and recommendations."""

        # Determine best/worst performers
        best_sport = max(insights.by_sport.items(),
                        key=lambda x: x[1]["win_rate"], default=(None, {}))
        worst_sport = min(insights.by_sport.items(),
                         key=lambda x: x[1]["win_rate"] if x[1]["trades"] >= 5 else 1,
                         default=(None, {}))

        # Build report
        report = f"""# Arbees Trading Report - {date.strftime('%B %d, %Y')}

## Executive Summary
- **Daily P&L:** ${insights.total_pnl:,.2f} {"🔥" if insights.total_pnl > 1500 else ""}
- **Win Rate:** {insights.win_rate:.1%} ({insights.winning_trades} wins / {insights.total_trades} trades)
- **Avg Edge:** {insights.avg_edge:.2%}
- **Opportunities:** {insights.signals_generated} detected, {insights.signals_executed} executed ({insights.signals_executed/max(insights.signals_generated,1):.0%} capture rate)

## What Went Well ✅
{self._format_successes(insights, best_sport)}

## What Needs Improvement ⚠️
{self._format_improvements(insights, worst_sport)}

## Losing Trades Analysis 💸
**Total Losses:** ${abs(sum(t['pnl'] for t in insights.worst_trades if t['pnl'] < 0)):,.2f} ({insights.losing_trades} trades)

{self._format_worst_trades(insights.worst_trades[:5])}

## Recommended Changes 🎯
{self._format_recommendations(insights.recommendations)}

## Performance by Category

### By Sport
{self._format_category_table(insights.by_sport)}

### By Market Type
{self._format_category_table(insights.by_market_type)}

### By Edge Range
{self._format_category_table(insights.by_edge_range)}

## 7-Day Performance Trend 📈
{self._format_trend_placeholder()}

---
*Generated automatically by Arbees ML Analyzer*
"""
        return report

    def _format_successes(self, insights, best_sport) -> str:
        lines = []
        if best_sport[0]:
            lines.append(f"1. **{best_sport[0]} performed well:** "
                        f"{best_sport[1]['win_rate']:.0%} win rate "
                        f"({best_sport[1]['wins']}/{best_sport[1]['trades']} trades)")

        # Find best edge range
        best_edge = max(insights.by_edge_range.items(),
                       key=lambda x: x[1]["win_rate"] if x[1]["trades"] >= 3 else 0,
                       default=(None, {}))
        if best_edge[0]:
            lines.append(f"2. **Edge {best_edge[0]} had {best_edge[1]['win_rate']:.0%} success rate**")

        return "\n".join(lines) if lines else "- Consistent performance across all categories"

    def _format_improvements(self, insights, worst_sport) -> str:
        lines = []
        if worst_sport[0] and worst_sport[1].get("win_rate", 1) < 0.5:
            lines.append(f"1. **{worst_sport[0]} struggled:** "
                        f"{worst_sport[1]['win_rate']:.0%} win rate - consider reducing exposure")

        if insights.missed_reasons:
            top_reason = max(insights.missed_reasons.items(), key=lambda x: x[1])
            lines.append(f"2. **Missed {insights.signals_generated - insights.signals_executed} opportunities** - "
                        f"top reason: {top_reason[0]} ({top_reason[1]} signals)")

        return "\n".join(lines) if lines else "- No major issues identified"

    def _format_worst_trades(self, trades: list) -> str:
        if not trades:
            return "No losing trades today!"

        lines = ["**Top Losses:**"]
        for i, t in enumerate(trades[:3], 1):
            lines.append(f"{i}. {t.get('game_id', 'Unknown')} {t.get('game_period', '')} "
                        f"(${t.get('pnl', 0):,.2f}) → Edge was {t.get('edge_at_entry', 0):.1%}")

        return "\n".join(lines)

    def _format_recommendations(self, recommendations: list) -> str:
        if not recommendations:
            return "- Continue with current parameters"

        lines = []
        for i, rec in enumerate(recommendations[:4], 1):
            lines.append(f"{i}. **{rec['title']}:**\n"
                        f"   - Current: {rec['current']}\n"
                        f"   - Recommended: {rec['recommended']}\n"
                        f"   - Expected impact: {rec['impact']}")

        return "\n\n".join(lines)

    def _format_category_table(self, data: dict) -> str:
        lines = ["| Category | Trades | Wins | Win Rate | P&L |",
                 "|----------|--------|------|----------|-----|"]
        for cat, stats in sorted(data.items(), key=lambda x: -x[1].get("pnl", 0)):
            lines.append(f"| {cat} | {stats['trades']} | {stats['wins']} | "
                        f"{stats['win_rate']:.0%} | ${stats['pnl']:,.2f} |")
        return "\n".join(lines)

    def _format_trend_placeholder(self) -> str:
        return """```
(7-day trend will appear after 7 days of data)
```"""

    def generate_html(self, date: date, insights: PerformanceInsights) -> str:
        """Create HTML report with charts."""
        # TODO: Implement HTML version with embedded charts
        md = self.generate_markdown(date, insights)
        # Convert markdown to HTML
        import markdown
        return markdown.markdown(md, extensions=['tables'])
```

---

## Phase 4: API Endpoints

### Historical Games API

```python
# services/api/routes/historical.py

from fastapi import APIRouter, Query, Depends
from typing import Optional, List
from datetime import date
from pydantic import BaseModel

router = APIRouter(prefix="/api/v1/historical", tags=["historical"])

class ArchivedGameSummary(BaseModel):
    archive_id: int
    game_id: str
    sport: str
    home_team: str
    away_team: str
    final_home_score: int
    final_away_score: int
    ended_at: str
    total_trades: int
    win_rate: float
    total_pnl: float

class ArchivedGameDetail(ArchivedGameSummary):
    trades: List[dict]
    signals: List[dict]
    price_history: List[dict]

class HistoricalGamesResponse(BaseModel):
    games: List[ArchivedGameSummary]
    total: int
    page: int
    page_size: int

@router.get("/games", response_model=HistoricalGamesResponse)
async def list_historical_games(
    sport: Optional[str] = Query(None, description="Filter by sport"),
    from_date: Optional[date] = Query(None, description="Start date"),
    to_date: Optional[date] = Query(None, description="End date"),
    outcome: Optional[str] = Query(None, description="profitable|loss|breakeven"),
    sort_by: str = Query("ended_at", description="ended_at|total_pnl|win_rate"),
    sort_order: str = Query("desc", description="asc|desc"),
    page: int = Query(1, ge=1),
    page_size: int = Query(20, ge=1, le=100),
    db = Depends(get_db),
):
    """List archived games with filters and pagination."""
    # Build query
    query = """
        SELECT archive_id, game_id, sport, home_team, away_team,
               final_home_score, final_away_score, ended_at,
               total_trades, win_rate, total_pnl
        FROM archived_games
        WHERE 1=1
    """
    params = []

    if sport:
        query += " AND sport = $1"
        params.append(sport)

    if from_date:
        query += f" AND ended_at >= ${len(params)+1}"
        params.append(from_date)

    if to_date:
        query += f" AND ended_at <= ${len(params)+1}"
        params.append(to_date)

    if outcome == "profitable":
        query += " AND total_pnl > 0"
    elif outcome == "loss":
        query += " AND total_pnl < 0"
    elif outcome == "breakeven":
        query += " AND total_pnl = 0"

    # Count total
    count_query = f"SELECT COUNT(*) FROM ({query}) sq"
    total = await db.fetchval(count_query, *params)

    # Add sorting and pagination
    query += f" ORDER BY {sort_by} {sort_order}"
    query += f" LIMIT ${len(params)+1} OFFSET ${len(params)+2}"
    params.extend([page_size, (page - 1) * page_size])

    rows = await db.fetch(query, *params)

    return HistoricalGamesResponse(
        games=[ArchivedGameSummary(**dict(r)) for r in rows],
        total=total,
        page=page,
        page_size=page_size,
    )

@router.get("/games/{game_id}", response_model=ArchivedGameDetail)
async def get_historical_game(
    game_id: str,
    db = Depends(get_db),
):
    """Get detailed view of an archived game."""
    game = await db.fetchrow(
        "SELECT * FROM archived_games WHERE game_id = $1", game_id
    )
    if not game:
        raise HTTPException(404, "Game not found")

    trades = await db.fetch(
        "SELECT * FROM archived_trades WHERE game_id = $1 ORDER BY opened_at",
        game_id
    )

    signals = await db.fetch(
        "SELECT * FROM archived_signals WHERE game_id = $1 ORDER BY generated_at",
        game_id
    )

    return ArchivedGameDetail(
        **dict(game),
        trades=[dict(t) for t in trades],
        signals=[dict(s) for s in signals],
        price_history=[],  # TODO: Include if stored
    )

@router.get("/summary")
async def get_historical_summary(
    from_date: Optional[date] = Query(None),
    to_date: Optional[date] = Query(None),
    db = Depends(get_db),
):
    """Get aggregate statistics for historical games."""
    query = """
        SELECT
            COUNT(*) as total_games,
            SUM(total_pnl) as total_pnl,
            AVG(win_rate) as avg_win_rate,
            SUM(total_trades) as total_trades,
            SUM(winning_trades) as total_wins,
            SUM(losing_trades) as total_losses
        FROM archived_games
        WHERE 1=1
    """
    params = []

    if from_date:
        query += " AND ended_at >= $1"
        params.append(from_date)
    if to_date:
        query += f" AND ended_at <= ${len(params)+1}"
        params.append(to_date)

    row = await db.fetchrow(query, *params)

    return {
        "total_games": row["total_games"],
        "total_pnl": float(row["total_pnl"] or 0),
        "avg_win_rate": float(row["avg_win_rate"] or 0),
        "total_trades": row["total_trades"],
        "win_rate": row["total_wins"] / max(row["total_trades"], 1),
    }
```

### ML Insights API

```python
# services/api/routes/ml.py

from fastapi import APIRouter, Query, Depends
from typing import Optional
from datetime import date

router = APIRouter(prefix="/api/v1/ml", tags=["ml"])

@router.get("/reports/{report_date}")
async def get_report(
    report_date: date,
    format: str = Query("markdown", description="markdown|html"),
    db = Depends(get_db),
):
    """Get ML analysis report for a specific date."""
    row = await db.fetchrow(
        "SELECT * FROM ml_analysis_reports WHERE report_date = $1",
        report_date
    )
    if not row:
        raise HTTPException(404, "Report not found for this date")

    if format == "html":
        return {"content": row["report_html"], "format": "html"}
    return {"content": row["report_markdown"], "format": "markdown"}

@router.get("/reports/latest")
async def get_latest_report(
    format: str = Query("markdown"),
    db = Depends(get_db),
):
    """Get the most recent ML analysis report."""
    row = await db.fetchrow(
        "SELECT * FROM ml_analysis_reports ORDER BY report_date DESC LIMIT 1"
    )
    if not row:
        raise HTTPException(404, "No reports available")

    if format == "html":
        return {"content": row["report_html"], "format": "html"}
    return {"content": row["report_markdown"], "format": "markdown"}

@router.get("/recommendations")
async def get_recommendations(
    status: Optional[str] = Query(None, description="pending|applied|rejected"),
    db = Depends(get_db),
):
    """Get parameter change recommendations."""
    query = """
        SELECT r.*, h.applied, h.approved_by
        FROM ml_analysis_reports r
        CROSS JOIN LATERAL jsonb_array_elements(r.recommendations) rec
        LEFT JOIN parameter_history h ON h.recommended_by = 'ML'
            AND h.parameter_name = rec->>'parameter'
        ORDER BY r.report_date DESC
        LIMIT 20
    """
    rows = await db.fetch(query)
    return [dict(r) for r in rows]

@router.post("/reports/generate")
async def generate_report_on_demand(
    for_date: date = Query(date.today()),
    ml_analyzer = Depends(get_ml_analyzer),
):
    """Generate an ML report on-demand."""
    report = await ml_analyzer.run_on_demand(for_date)
    return {"status": "generated", "date": for_date, "report": report}

@router.get("/insights/sports")
async def get_sport_insights(
    days: int = Query(30, ge=1, le=90),
    db = Depends(get_db),
):
    """Get performance breakdown by sport."""
    query = """
        SELECT
            sport,
            COUNT(*) as games,
            SUM(total_trades) as trades,
            SUM(winning_trades) as wins,
            SUM(total_pnl) as pnl,
            AVG(win_rate) as avg_win_rate
        FROM archived_games
        WHERE ended_at > NOW() - INTERVAL '%s days'
        GROUP BY sport
        ORDER BY pnl DESC
    """
    rows = await db.fetch(query, days)
    return [dict(r) for r in rows]
```

---

## Phase 5: Critical Questions Answered

### Game Lifecycle Questions

| Question | Answer |
|----------|--------|
| When to mark "ended"? | When `status in ['final', 'complete', 'closed']` |
| Grace period? | **60 minutes** - allows for score corrections, late settlements |
| Delete or mark archived? | **Mark archived** - keep original for debugging, use flag `games.archived=true` |
| Suspended/postponed games? | Status becomes `suspended`/`postponed` - don't archive, keep monitoring or manual cleanup |
| Open positions on unexpected end? | Close at last known price, log warning, flag for review |

### ML Analysis Questions

| Question | Answer |
|----------|--------|
| Minimum data for ML? | **100 trades** - below this, use simple statistics only |
| Model retrain frequency? | **Weekly** - daily retrain risks overfitting to recent noise |
| ML libraries? | **scikit-learn** for classifiers, **XGBoost** optional for advanced models |
| Overfitting prevention? | 80/20 train/test split, cross-validation, simple models first (Random Forest depth=5) |
| Report format? | **Markdown primary**, HTML for dashboard, PDF optional for email |
| Report delivery? | **File save** initially, then add Slack webhook, email optional |
| Auto-apply recommendations? | **NO** - always require manual approval via `parameter_history.approved_by` |

---

## Phase 6: Risk Analysis & Mitigation

### Game Lifecycle Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Data loss during archival | Low | High | Database transaction wraps all operations |
| Position not closed properly | Medium | High | Reconciliation check after settlement, alert on mismatch |
| Historical queries slow live trading | Medium | Medium | Separate tables + dedicated read replicas if needed |
| Archive backlog grows | Low | Low | Batch processing, monitor queue size, alert if > 50 pending |
| Score correction after archive | Low | Medium | Store original in archive, grace period handles most |

### ML Analysis Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Overfitting | High | High | Simple models, cross-validation, min 100 trades |
| Cold start (not enough data) | High initially | Medium | Start with statistics, add ML after 1 week |
| Auto-apply bad recommendations | N/A | High | **Disabled** - all changes require manual approval |
| Model degrades over time | Medium | Medium | Weekly retrain, track accuracy over time |
| Report generation failure | Low | Low | Catch exceptions, send alert, retry next day |

---

## Phase 7: Implementation Timeline

### Week 1: Database & Archive Backend

| Day | Task |
|-----|------|
| 1 | Create migration `014_archive_tables.sql` with all new tables |
| 2 | Implement `GameArchiver` service with Redis subscription |
| 3 | Implement archive worker with transaction logic |
| 4 | Add `games:ended` event to GameShard when game completes |
| 5 | Test archival flow end-to-end with real game data |

### Week 2: API & Frontend

| Day | Task |
|-----|------|
| 1 | Implement historical games API endpoints |
| 2 | Create `HistoricalGames.tsx` page with table view |
| 3 | Add filters (sport, date, outcome) and sorting |
| 4 | Create game detail modal with trade breakdown |
| 5 | Add summary dashboard component |

### Week 3: ML Data Pipeline

| Day | Task |
|-----|------|
| 1 | Implement `FeatureExtractor` class |
| 2 | Create database queries for historical trade loading |
| 3 | Build feature extraction for all trade/signal dimensions |
| 4 | Create `InsightExtractor` for basic statistics |
| 5 | Test feature extraction with real archived data |

### Week 4: ML Models & Optimization

| Day | Task |
|-----|------|
| 1 | Implement `TradeSuccessModel` (Random Forest classifier) |
| 2 | Implement model training and persistence |
| 3 | Implement `ParameterOptimizer` for edge threshold tuning |
| 4 | Build recommendation generation logic |
| 5 | Test models on historical data, validate accuracy |

### Week 5: Reporting & Delivery

| Day | Task |
|-----|------|
| 1 | Implement `HotWashReportGenerator` markdown output |
| 2 | Add HTML report generation |
| 3 | Implement scheduled nightly run with `schedule` library |
| 4 | Add report storage to `ml_analysis_reports` table |
| 5 | Add Slack webhook delivery (optional), test full pipeline |

---

## Phase 8: Success Metrics

### Game Lifecycle

| Metric | Target | Measurement |
|--------|--------|-------------|
| Live games page accurate | 100% only active games | Manual spot check |
| Archive latency | < 2 hours from game end | `archived_at - ended_at` |
| Data integrity | 0 lost trades/signals | Compare counts before/after |
| Historical page load time | < 2 seconds | API response time monitoring |
| Filter accuracy | 100% correct results | Automated test suite |

### ML Analysis

| Metric | Target | Measurement |
|--------|--------|-------------|
| First report generated | Week 5 | Manual check |
| Report delivery success | 99% | Monitor delivery logs |
| Model accuracy | > 55% (better than random) | Holdout set evaluation |
| Actionable recommendations | >= 3 per report | Count recommendations |
| Win rate improvement | +5% after applying recommendations | Compare before/after periods |

---

## Appendix: File Structure

```
services/
├── archiver/
│   ├── __init__.py
│   ├── archiver.py
│   ├── archive_worker.py
│   └── config.py
├── ml_analyzer/
│   ├── __init__.py
│   ├── analyzer.py
│   ├── feature_extractor.py
│   ├── insights.py
│   ├── report_generator.py
│   ├── config.py
│   └── models/
│       ├── __init__.py
│       ├── trade_success.py
│       ├── pnl_predictor.py
│       └── optimizer.py
├── api/
│   └── routes/
│       ├── historical.py
│       └── ml.py
└── ...

shared/arbees_shared/db/migrations/
└── 014_archive_tables.sql

frontend/src/
├── pages/
│   └── HistoricalGames.tsx
└── components/
    ├── GameDetailModal.tsx
    └── PerformanceSummary.tsx
```

---

## Next Steps

1. **Review this plan** - Confirm architecture decisions
2. **Start Week 1** - Database migration and archiver service
3. **Parallel frontend work** - Can start historical page UI while backend develops
4. **Data collection** - Continue paper trading to build historical dataset for ML

Ready to begin implementation?
