# Claude Code Prompt: Futures Tracking + Game Lifecycle Management

## Overview

Implement two new features for the Arbees trading system:
1. **Futures/Pre-Game Prop Tracking** - Monitor markets 24-48 hours before games start to catch early pricing inefficiencies
2. **Game Lifecycle Management** - Automatically archive completed games and create a historical analysis page

---

## Feature 1: Futures/Pre-Game Prop Tracking

### Problem Statement

Currently, we only monitor markets for games that are already in progress. This means we miss opportunities that exist when markets first open (24-48 hours before game time).

**Opportunity:**
- Markets often misprice when first listed (low liquidity, less sharp action)
- Early odds can shift dramatically as game approaches
- Sharp bettors can identify value before public betting action

**Example Scenario:**
```
Monday 10am: Lakers vs Celtics game listed for Wednesday 7pm
  - Initial odds: Lakers -3.5 @ $0.55
  - Low volume, market maker hasn't adjusted yet
  
Monday 2pm: Injury news breaks (LeBron questionable)
  - Odds should move to Lakers -2.5 or even pick'em
  - But market hasn't updated yet → OPPORTUNITY!
  
Wednesday 6pm: Game about to start
  - Odds now accurately reflect all information
  - Too late, opportunity missed
```

### Requirements

#### Core Functionality

1. **Market Discovery for Future Games**
   - Poll ESPN API for games in next 24-48 hours
   - Discover markets on Kalshi/Polymarket as soon as they're listed
   - Store future game info with countdown to start time

2. **Price Tracking**
   - Subscribe to WebSocket feeds for future game markets
   - Track price movements leading up to game
   - Calculate price trends (momentum)
   - Alert on significant price shifts

3. **Early Opportunity Detection**
   - Compare initial market prices to historical patterns
   - Identify obvious mispricings
   - Generate "futures signals" when edge detected
   - Different thresholds than live games (higher edge required)

4. **Frontend Integration**
   - Separate "Futures" tab/page in dashboard
   - Show countdown to game start
   - Display price history charts
   - List of upcoming opportunities
   - Same chart components as live games (reuse)

#### Technical Architecture

```
┌─────────────────────────────────────────────────────────┐
│  FuturesMonitor Service (New)                           │
│  ├─ Poll ESPN for upcoming games (every 15 min)        │
│  ├─ Discover markets on Kalshi/Polymarket              │
│  ├─ Subscribe to WebSocket price feeds                 │
│  ├─ Track price history in TimescaleDB                 │
│  └─ Generate futures signals                           │
└─────────────────────────────────────────────────────────┘
         ↓ (stores in database)
┌─────────────────────────────────────────────────────────┐
│  PostgreSQL / TimescaleDB                               │
│  ├─ future_games (game_id, start_time, discovered_at)  │
│  ├─ future_market_prices (continuous time-series)      │
│  └─ futures_signals (early opportunities)              │
└─────────────────────────────────────────────────────────┘
         ↓ (API endpoints)
┌─────────────────────────────────────────────────────────┐
│  Frontend: Futures Page                                 │
│  ├─ List of upcoming games (24-48h out)                │
│  ├─ Price trend charts (reuse GameTracker components)  │
│  ├─ Countdown timers                                   │
│  └─ Early opportunity alerts                           │
└─────────────────────────────────────────────────────────┘
```

---

## Feature 2: Game Lifecycle Management

### Problem Statement

Games currently remain in the system forever, cluttering the dashboard and database. We need automatic archival and a historical analysis page.

**Current Issues:**
- Live games list grows indefinitely
- No way to review past performance by game
- Database fills with stale data
- Can't easily analyze "what went well/wrong"

### Requirements

#### Core Functionality

1. **Automatic Game Archival**
   - Detect when games finish (status = "final")
   - Wait 30 minutes (for late score corrections)
   - Move game data to historical tables
   - Remove from live GameShard tracking
   - Preserve all trades, signals, price history

2. **Historical Database Schema**
   - Archive game states
   - Archive all trades executed
   - Archive all signals generated
   - Archive complete price history
   - Calculate final P&L per game

3. **Historical Analysis Page**
   - List of past games (filterable by date, sport, outcome)
   - Per-game summary:
     - Final score
     - Total P&L
     - Win rate (successful trades / total)
     - Signals generated vs executed
     - Price chart replay
   - Search and filter
   - Export to CSV

#### Technical Architecture

```
┌─────────────────────────────────────────────────────────┐
│  GameArchiver Service (New)                             │
│  ├─ Poll for completed games (every 5 min)             │
│  ├─ Wait 30 min after game ends                        │
│  ├─ Calculate final P&L                                │
│  ├─ Move to historical tables                          │
│  └─ Notify GameShard to stop tracking                  │
└─────────────────────────────────────────────────────────┘
         ↓ (archives to)
┌─────────────────────────────────────────────────────────┐
│  PostgreSQL Historical Tables                           │
│  ├─ archived_games                                     │
│  │   ├─ game_id, final_score, archived_at             │
│  │   ├─ total_pnl, win_rate                           │
│  │   └─ signals_generated, trades_executed            │
│  ├─ archived_trades                                    │
│  │   └─ (copy of all trades for this game)            │
│  ├─ archived_signals                                   │
│  │   └─ (copy of all signals for this game)           │
│  └─ archived_prices (TimescaleDB hypertable)          │
│      └─ (price history compressed for long-term)      │
└─────────────────────────────────────────────────────────┘
         ↓ (API endpoints)
┌─────────────────────────────────────────────────────────┐
│  Frontend: Historical Games Page                        │
│  ├─ Table of past games (sortable, filterable)         │
│  ├─ Date range picker                                  │
│  ├─ Sport filter (NBA, NFL, etc.)                      │
│  ├─ Outcome filter (Win/Loss/Break-even)               │
│  ├─ Per-game detail view:                              │
│  │   ├─ Game summary card                              │
│  │   ├─ Trade list with P&L                            │
│  │   ├─ Price chart (historical replay)                │
│  │   └─ Signal analysis                                │
│  └─ Export to CSV                                      │
└─────────────────────────────────────────────────────────┘
```

---

## Implementation Guide

### Part 1: Futures Monitoring Service

#### File: `services/futures_monitor/monitor.py` (NEW)

```python
"""
Futures Monitor Service

Tracks upcoming games and their markets 24-48 hours before start time.
Detects early pricing inefficiencies and generates futures signals.
"""

import asyncio
import os
from datetime import datetime, timedelta
from typing import List, Optional
from loguru import logger

from data_providers.espn.client import ESPNClient
from arbees_shared.models.game import Sport, GameState
from arbees_shared.models.market import Platform
from services.market_discovery.discovery import MarketDiscoveryService
from markets.kalshi.websocket.ws_client import KalshiWebSocketClient
from markets.polymarket.websocket.ws_client import PolymarketWebSocketClient
from arbees_shared.db.connection import DatabaseClient


class FuturesMonitor:
    """
    Monitor upcoming games and their markets.
    
    Workflow:
    1. Poll ESPN for games in next 24-48 hours
    2. Discover markets on Kalshi/Polymarket
    3. Subscribe to WebSocket price feeds
    4. Track price movements
    5. Generate futures signals on mispricings
    """
    
    def __init__(self):
        self.espn = ESPNClient(Sport.NBA)  # Can add more sports
        self.discovery = MarketDiscoveryService()
        self.kalshi_ws = KalshiWebSocketClient()
        self.poly_ws = PolymarketWebSocketClient()
        self.db: Optional[DatabaseClient] = None
        
        # Tracking
        self.future_games: dict[str, FutureGame] = {}
        self.monitored_markets: set[str] = set()
        
        # Config
        self.lookback_hours = int(os.getenv("FUTURES_LOOKBACK_HOURS", "48"))
        self.poll_interval = int(os.getenv("FUTURES_POLL_INTERVAL", "900"))  # 15 min
        self.min_edge_futures = float(os.getenv("FUTURES_MIN_EDGE", "0.05"))  # 5%
    
    async def connect(self):
        """Initialize connections."""
        logger.info("Connecting Futures Monitor...")
        
        await self.espn.connect()
        await self.discovery.connect()
        await self.kalshi_ws.connect()
        await self.poly_ws.connect()
        
        # Database
        from arbees_shared.db.connection import get_pool
        pool = await get_pool()
        self.db = DatabaseClient(pool)
        
        logger.info("✅ Futures Monitor connected")
    
    async def run(self):
        """Main monitoring loop."""
        await self.connect()
        
        # Start background tasks
        asyncio.create_task(self._poll_upcoming_games())
        asyncio.create_task(self._stream_prices())
        asyncio.create_task(self._generate_futures_signals())
        
        # Keep running
        await asyncio.Event().wait()
    
    async def _poll_upcoming_games(self):
        """Poll ESPN for upcoming games every 15 minutes."""
        while True:
            try:
                await self._discover_future_games()
                await asyncio.sleep(self.poll_interval)
            except Exception as e:
                logger.error(f"Error polling upcoming games: {e}")
                await asyncio.sleep(60)
    
    async def _discover_future_games(self):
        """Discover games in next 24-48 hours."""
        logger.info("Discovering upcoming games...")
        
        # Get schedule from ESPN
        now = datetime.utcnow()
        start_time = now
        end_time = now + timedelta(hours=self.lookback_hours)
        
        games = await self.espn.get_schedule(start_time, end_time)
        
        logger.info(f"Found {len(games)} upcoming games")
        
        for game_id in games:
            if game_id in self.future_games:
                continue  # Already tracking
            
            # Get game details
            state, _ = await self.espn.poll_game(game_id, None)
            if not state:
                continue
            
            # Only track pre-game
            if state.status != "scheduled":
                continue
            
            # Discover markets
            await self._discover_markets_for_game(game_id, state)
    
    async def _discover_markets_for_game(self, game_id: str, state: GameState):
        """Discover markets for upcoming game."""
        logger.info(
            f"Discovering markets for {state.away_team} @ {state.home_team} "
            f"(starts in {self._time_until_start(state)})"
        )
        
        # Find markets
        markets = await self.discovery.find_markets_for_game(
            game_state=state,
            platforms=[Platform.KALSHI, Platform.POLYMARKET]
        )
        
        if not markets:
            logger.warning(f"No markets found for game {game_id}")
            return
        
        # Create FutureGame tracking
        future_game = FutureGame(
            game_id=game_id,
            game_state=state,
            markets=markets,
            discovered_at=datetime.utcnow()
        )
        
        self.future_games[game_id] = future_game
        
        # Subscribe to price feeds
        for market in markets:
            if market.market_id not in self.monitored_markets:
                await self._subscribe_to_market(market)
                self.monitored_markets.add(market.market_id)
        
        # Store in database
        await self._store_future_game(future_game)
        
        logger.info(
            f"✅ Now tracking {len(markets)} markets for game {game_id}"
        )
    
    async def _subscribe_to_market(self, market):
        """Subscribe to price updates for market."""
        if market.platform == Platform.KALSHI:
            await self.kalshi_ws.subscribe([market.market_id])
        elif market.platform == Platform.POLYMARKET:
            await self.poly_ws.subscribe([market.token_id])
    
    async def _stream_prices(self):
        """Stream price updates and store in database."""
        # Merge both WebSocket streams
        async def handle_kalshi():
            async for price in self.kalshi_ws.stream_prices():
                await self._handle_price_update(price)
        
        async def handle_poly():
            async for price in self.poly_ws.stream_prices():
                await self._handle_price_update(price)
        
        await asyncio.gather(
            handle_kalshi(),
            handle_poly()
        )
    
    async def _handle_price_update(self, price):
        """Handle price update for future game market."""
        # Find which game this belongs to
        game_id = self._find_game_for_market(price.market_id)
        if not game_id:
            return
        
        # Store price in database
        await self.db.insert_future_market_price(
            game_id=game_id,
            market_id=price.market_id,
            platform=price.platform.value,
            price=price.mid_price,
            timestamp=price.timestamp
        )
        
        # Update in-memory tracking
        future_game = self.future_games.get(game_id)
        if future_game:
            future_game.latest_prices[price.market_id] = price
    
    async def _generate_futures_signals(self):
        """Generate signals for futures opportunities."""
        while True:
            try:
                for game_id, future_game in list(self.future_games.items()):
                    # Check if game started (move to live tracking)
                    if self._is_game_starting_soon(future_game, minutes=30):
                        await self._transition_to_live(future_game)
                        continue
                    
                    # Check for opportunities
                    await self._check_futures_opportunities(future_game)
                
                await asyncio.sleep(60)  # Check every minute
                
            except Exception as e:
                logger.error(f"Error generating futures signals: {e}")
                await asyncio.sleep(60)
    
    async def _check_futures_opportunities(self, future_game: FutureGame):
        """Check for early pricing opportunities."""
        # Calculate expected price based on historical data
        # Compare to current market price
        # If edge > threshold, generate signal
        
        # This is simplified - you'd want more sophisticated analysis
        for market_id, price in future_game.latest_prices.items():
            # Example: Check if price has moved significantly
            historical_prices = await self._get_historical_prices(market_id)
            if not historical_prices:
                continue
            
            avg_price = sum(historical_prices) / len(historical_prices)
            current_price = price.mid_price
            
            edge = abs(current_price - avg_price)
            
            if edge >= self.min_edge_futures:
                # Generate futures signal
                signal = self._create_futures_signal(
                    future_game=future_game,
                    market_id=market_id,
                    edge=edge,
                    direction="BUY" if current_price < avg_price else "SELL"
                )
                
                await self._store_futures_signal(signal)
                
                logger.info(
                    f"🎯 Futures opportunity: {future_game.game_state.away_team} @ "
                    f"{future_game.game_state.home_team} - Edge: {edge:.2%}"
                )
    
    def _time_until_start(self, state: GameState) -> str:
        """Calculate time until game starts."""
        # This is simplified - you'd get actual start time from state
        return "24h"  # Placeholder
    
    def _is_game_starting_soon(self, future_game: FutureGame, minutes: int) -> bool:
        """Check if game is starting within N minutes."""
        # Check actual game start time
        # Return True if < minutes away
        return False  # Placeholder
    
    async def _transition_to_live(self, future_game: FutureGame):
        """Transition future game to live monitoring."""
        logger.info(
            f"Game starting soon: {future_game.game_id} - "
            f"Transitioning to live tracking"
        )
        
        # Remove from futures tracking
        del self.future_games[future_game.game_id]
        
        # Signal orchestrator to start live monitoring
        # (This would integrate with your existing orchestrator)
    
    async def _store_future_game(self, future_game):
        """Store future game in database."""
        await self.db.execute("""
            INSERT INTO future_games 
            (game_id, home_team, away_team, sport, start_time, discovered_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (game_id) DO NOTHING
        """,
            future_game.game_id,
            future_game.game_state.home_team,
            future_game.game_state.away_team,
            future_game.game_state.sport.value,
            None,  # TODO: Get actual start time
            future_game.discovered_at
        )
    
    async def _store_futures_signal(self, signal):
        """Store futures signal in database."""
        # Store in futures_signals table
        pass


@dataclass
class FutureGame:
    """Tracking for an upcoming game."""
    game_id: str
    game_state: GameState
    markets: List
    discovered_at: datetime
    latest_prices: dict = field(default_factory=dict)
```

#### File: `services/futures_monitor/Dockerfile` (NEW)

```dockerfile
FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . /app

CMD ["python", "monitor.py"]
```

#### Database Migrations

```sql
-- migrations/003_futures_tables.sql

-- Future games tracking
CREATE TABLE IF NOT EXISTS future_games (
    game_id VARCHAR(50) PRIMARY KEY,
    home_team VARCHAR(100) NOT NULL,
    away_team VARCHAR(100) NOT NULL,
    sport VARCHAR(20) NOT NULL,
    start_time TIMESTAMP,
    discovered_at TIMESTAMP NOT NULL DEFAULT NOW(),
    transitioned_at TIMESTAMP,  -- When moved to live
    status VARCHAR(20) DEFAULT 'tracking'
);

-- Future market prices (TimescaleDB hypertable)
CREATE TABLE IF NOT EXISTS future_market_prices (
    time TIMESTAMP NOT NULL,
    game_id VARCHAR(50) NOT NULL,
    market_id VARCHAR(200) NOT NULL,
    platform VARCHAR(20) NOT NULL,
    price DECIMAL(10, 8) NOT NULL,
    volume DECIMAL(20, 2),
    PRIMARY KEY (time, game_id, market_id)
);

-- Convert to hypertable for time-series efficiency
SELECT create_hypertable('future_market_prices', 'time', if_not_exists => TRUE);

-- Futures signals
CREATE TABLE IF NOT EXISTS futures_signals (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(50) NOT NULL,
    market_id VARCHAR(200) NOT NULL,
    platform VARCHAR(20) NOT NULL,
    signal_type VARCHAR(10) NOT NULL,  -- BUY, SELL
    edge DECIMAL(10, 6) NOT NULL,
    price DECIMAL(10, 8) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    executed BOOLEAN DEFAULT FALSE,
    FOREIGN KEY (game_id) REFERENCES future_games(game_id)
);

-- Indexes
CREATE INDEX idx_future_games_start_time ON future_games(start_time);
CREATE INDEX idx_future_games_status ON future_games(status);
CREATE INDEX idx_futures_signals_game_id ON futures_signals(game_id);
CREATE INDEX idx_futures_signals_created_at ON futures_signals(created_at);
```

---

### Part 2: Game Archiver Service

#### File: `services/archiver/archiver.py` (NEW)

```python
"""
Game Archiver Service

Automatically archives completed games after a 30-minute grace period.
Moves all data to historical tables for analysis.
"""

import asyncio
import os
from datetime import datetime, timedelta
from typing import Optional
from loguru import logger

from data_providers.espn.client import ESPNClient
from arbees_shared.models.game import Sport
from arbees_shared.db.connection import DatabaseClient


class GameArchiver:
    """
    Archive completed games to historical tables.
    
    Workflow:
    1. Poll for games with status='final' (every 5 min)
    2. Wait 30 minutes after game ends (for score corrections)
    3. Calculate final P&L and stats
    4. Copy all data to archived_* tables
    5. Remove from active tracking
    6. Notify orchestrator to stop GameShard
    """
    
    def __init__(self):
        self.espn = ESPNClient(Sport.NBA)
        self.db: Optional[DatabaseClient] = None
        
        # Config
        self.grace_period_minutes = int(os.getenv("ARCHIVE_GRACE_PERIOD", "30"))
        self.poll_interval = int(os.getenv("ARCHIVE_POLL_INTERVAL", "300"))  # 5 min
    
    async def connect(self):
        """Initialize connections."""
        logger.info("Connecting Game Archiver...")
        
        await self.espn.connect()
        
        # Database
        from arbees_shared.db.connection import get_pool
        pool = await get_pool()
        self.db = DatabaseClient(pool)
        
        logger.info("✅ Game Archiver connected")
    
    async def run(self):
        """Main archival loop."""
        await self.connect()
        
        while True:
            try:
                await self._archive_completed_games()
                await asyncio.sleep(self.poll_interval)
            except Exception as e:
                logger.error(f"Error in archiver loop: {e}")
                await asyncio.sleep(60)
    
    async def _archive_completed_games(self):
        """Find and archive completed games."""
        logger.debug("Checking for completed games to archive...")
        
        # Find games that ended > grace_period minutes ago
        cutoff_time = datetime.utcnow() - timedelta(minutes=self.grace_period_minutes)
        
        completed_games = await self.db.fetch("""
            SELECT game_id, home_team, away_team, 
                   home_score, away_score, status, 
                   updated_at
            FROM game_states
            WHERE status = 'final'
              AND updated_at < $1
              AND game_id NOT IN (
                  SELECT game_id FROM archived_games
              )
        """, cutoff_time)
        
        if not completed_games:
            logger.debug("No games ready to archive")
            return
        
        logger.info(f"Found {len(completed_games)} games to archive")
        
        for game in completed_games:
            try:
                await self._archive_game(game)
            except Exception as e:
                logger.error(f"Error archiving game {game['game_id']}: {e}")
    
    async def _archive_game(self, game: dict):
        """Archive a single game."""
        game_id = game['game_id']
        
        logger.info(
            f"Archiving game: {game['away_team']} @ {game['home_team']} "
            f"(Final: {game['away_score']}-{game['home_score']})"
        )
        
        # Calculate final statistics
        stats = await self._calculate_game_stats(game_id)
        
        # Begin transaction
        async with self.db.pool.acquire() as conn:
            async with conn.transaction():
                # 1. Archive game record
                await conn.execute("""
                    INSERT INTO archived_games 
                    (game_id, home_team, away_team, home_score, away_score,
                     sport, final_status, ended_at, archived_at,
                     total_pnl, win_rate, total_trades, signals_generated)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW(), $9, $10, $11, $12)
                """,
                    game_id,
                    game['home_team'],
                    game['away_team'],
                    game['home_score'],
                    game['away_score'],
                    'NBA',  # TODO: Get from game record
                    game['status'],
                    game['updated_at'],
                    stats['total_pnl'],
                    stats['win_rate'],
                    stats['total_trades'],
                    stats['signals_generated']
                )
                
                # 2. Archive all trades
                await conn.execute("""
                    INSERT INTO archived_trades
                    SELECT * FROM trades WHERE game_id = $1
                """, game_id)
                
                # 3. Archive all signals
                await conn.execute("""
                    INSERT INTO archived_signals
                    SELECT * FROM trading_signals WHERE game_id = $1
                """, game_id)
                
                # 4. Archive price history
                # (TimescaleDB automatically compresses old data)
                await conn.execute("""
                    INSERT INTO archived_market_prices
                    SELECT * FROM market_prices WHERE game_id = $1
                """, game_id)
                
                # 5. Delete from active tables
                await conn.execute("DELETE FROM trades WHERE game_id = $1", game_id)
                await conn.execute("DELETE FROM trading_signals WHERE game_id = $1", game_id)
                await conn.execute("DELETE FROM market_prices WHERE game_id = $1", game_id)
                await conn.execute("DELETE FROM game_states WHERE game_id = $1", game_id)
        
        logger.info(
            f"✅ Archived game {game_id}: "
            f"P&L=${stats['total_pnl']:.2f}, "
            f"Win Rate={stats['win_rate']:.1%}, "
            f"Trades={stats['total_trades']}"
        )
        
        # Notify orchestrator to stop tracking
        await self._notify_orchestrator(game_id)
    
    async def _calculate_game_stats(self, game_id: str) -> dict:
        """Calculate final statistics for game."""
        # Total P&L
        pnl_result = await self.db.fetchrow("""
            SELECT COALESCE(SUM(pnl), 0) as total_pnl,
                   COUNT(*) as total_trades,
                   COUNT(*) FILTER (WHERE pnl > 0) as winning_trades
            FROM trades
            WHERE game_id = $1
        """, game_id)
        
        # Signals generated
        signals_result = await self.db.fetchrow("""
            SELECT COUNT(*) as signals_generated
            FROM trading_signals
            WHERE game_id = $1
        """, game_id)
        
        total_trades = pnl_result['total_trades'] or 0
        winning_trades = pnl_result['winning_trades'] or 0
        
        return {
            'total_pnl': float(pnl_result['total_pnl'] or 0),
            'total_trades': total_trades,
            'signals_generated': signals_result['signals_generated'] or 0,
            'win_rate': winning_trades / total_trades if total_trades > 0 else 0.0
        }
    
    async def _notify_orchestrator(self, game_id: str):
        """Notify orchestrator that game is archived."""
        # Send Redis message or API call to orchestrator
        # to stop GameShard tracking for this game
        logger.info(f"Notified orchestrator to stop tracking {game_id}")
```

#### Database Migrations

```sql
-- migrations/004_archival_tables.sql

-- Archived games
CREATE TABLE IF NOT EXISTS archived_games (
    game_id VARCHAR(50) PRIMARY KEY,
    home_team VARCHAR(100) NOT NULL,
    away_team VARCHAR(100) NOT NULL,
    home_score INT,
    away_score INT,
    sport VARCHAR(20) NOT NULL,
    final_status VARCHAR(20) NOT NULL,
    ended_at TIMESTAMP NOT NULL,
    archived_at TIMESTAMP NOT NULL DEFAULT NOW(),
    
    -- Statistics
    total_pnl DECIMAL(12, 2),
    win_rate DECIMAL(5, 4),
    total_trades INT,
    signals_generated INT
);

-- Archived trades (copy of trades table structure)
CREATE TABLE IF NOT EXISTS archived_trades (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(50) NOT NULL,
    team VARCHAR(100) NOT NULL,
    action VARCHAR(10) NOT NULL,
    platform VARCHAR(20) NOT NULL,
    entry_price DECIMAL(10, 8) NOT NULL,
    exit_price DECIMAL(10, 8),
    size DECIMAL(12, 2) NOT NULL,
    pnl DECIMAL(12, 2),
    executed_at TIMESTAMP NOT NULL,
    closed_at TIMESTAMP
);

-- Archived signals
CREATE TABLE IF NOT EXISTS archived_signals (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(50) NOT NULL,
    team VARCHAR(100) NOT NULL,
    action VARCHAR(10) NOT NULL,
    edge DECIMAL(10, 6) NOT NULL,
    model_prob DECIMAL(10, 8) NOT NULL,
    market_prob DECIMAL(10, 8) NOT NULL,
    created_at TIMESTAMP NOT NULL,
    executed BOOLEAN DEFAULT FALSE
);

-- Archived market prices (TimescaleDB hypertable)
CREATE TABLE IF NOT EXISTS archived_market_prices (
    time TIMESTAMP NOT NULL,
    game_id VARCHAR(50) NOT NULL,
    market_id VARCHAR(200) NOT NULL,
    platform VARCHAR(20) NOT NULL,
    price DECIMAL(10, 8) NOT NULL,
    PRIMARY KEY (time, game_id, market_id)
);

-- Convert to hypertable
SELECT create_hypertable('archived_market_prices', 'time', if_not_exists => TRUE);

-- Indexes for fast queries
CREATE INDEX idx_archived_games_sport ON archived_games(sport);
CREATE INDEX idx_archived_games_ended_at ON archived_games(ended_at);
CREATE INDEX idx_archived_games_pnl ON archived_games(total_pnl);
CREATE INDEX idx_archived_trades_game_id ON archived_trades(game_id);
CREATE INDEX idx_archived_signals_game_id ON archived_signals(game_id);
```

---

### Part 3: Frontend Components

#### File: `frontend/src/pages/FuturesPage.tsx` (NEW)

```typescript
/**
 * Futures Page
 * 
 * Shows upcoming games (24-48h out) with price tracking and opportunities.
 * Reuses GameTracker components for charts.
 */

import React, { useEffect, useState } from 'react';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { PropChart } from '@/components/PropChart';

interface FutureGame {
  game_id: string;
  home_team: string;
  away_team: string;
  sport: string;
  start_time: string;
  hours_until_start: number;
  latest_price: number;
  initial_price: number;
  price_change: number;
  opportunities: number;
}

export function FuturesPage() {
  const [futureGames, setFutureGames] = useState<FutureGame[]>([]);
  const [selectedGame, setSelectedGame] = useState<string | null>(null);
  
  useEffect(() => {
    // Poll for future games every 30 seconds
    const fetchFutureGames = async () => {
      const response = await fetch('/api/futures/games');
      const data = await response.json();
      setFutureGames(data.games);
    };
    
    fetchFutureGames();
    const interval = setInterval(fetchFutureGames, 30000);
    
    return () => clearInterval(interval);
  }, []);
  
  return (
    <div className="p-6 space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-3xl font-bold">Futures Tracking</h1>
        <Badge variant="outline">
          {futureGames.length} upcoming games
        </Badge>
      </div>
      
      {/* Game List */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {futureGames.map((game) => (
          <FutureGameCard
            key={game.game_id}
            game={game}
            onClick={() => setSelectedGame(game.game_id)}
            isSelected={selectedGame === game.game_id}
          />
        ))}
      </div>
      
      {/* Selected Game Detail */}
      {selectedGame && (
        <FutureGameDetail gameId={selectedGame} />
      )}
    </div>
  );
}

function FutureGameCard({ game, onClick, isSelected }: {
  game: FutureGame;
  onClick: () => void;
  isSelected: boolean;
}) {
  return (
    <Card
      className={`cursor-pointer transition-all ${
        isSelected ? 'ring-2 ring-blue-500' : ''
      }`}
      onClick={onClick}
    >
      <CardHeader>
        <CardTitle className="text-lg">
          {game.away_team} @ {game.home_team}
        </CardTitle>
        <div className="text-sm text-gray-500">
          {game.sport} • Starts in {game.hours_until_start}h
        </div>
      </CardHeader>
      <CardContent>
        <div className="space-y-2">
          <div className="flex justify-between">
            <span className="text-sm">Current Price:</span>
            <span className="font-mono">${game.latest_price.toFixed(3)}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-sm">Price Change:</span>
            <Badge variant={game.price_change > 0 ? 'success' : 'destructive'}>
              {game.price_change > 0 ? '+' : ''}
              {(game.price_change * 100).toFixed(1)}%
            </Badge>
          </div>
          {game.opportunities > 0 && (
            <div className="flex justify-between">
              <span className="text-sm">Opportunities:</span>
              <Badge variant="warning">{game.opportunities}</Badge>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function FutureGameDetail({ gameId }: { gameId: string }) {
  const [priceHistory, setPriceHistory] = useState([]);
  
  useEffect(() => {
    // Fetch price history for this game
    const fetchHistory = async () => {
      const response = await fetch(`/api/futures/games/${gameId}/prices`);
      const data = await response.json();
      setPriceHistory(data.prices);
    };
    
    fetchHistory();
  }, [gameId]);
  
  return (
    <Card>
      <CardHeader>
        <CardTitle>Price History</CardTitle>
      </CardHeader>
      <CardContent>
        {/* Reuse PropChart component */}
        <PropChart
          data={priceHistory}
          title="Market Price Over Time"
        />
      </CardContent>
    </Card>
  );
}
```

#### File: `frontend/src/pages/HistoricalGamesPage.tsx` (NEW)

```typescript
/**
 * Historical Games Page
 * 
 * Shows archived games with P&L analysis and trade details.
 */

import React, { useEffect, useState } from 'react';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';

interface ArchivedGame {
  game_id: string;
  home_team: string;
  away_team: string;
  home_score: number;
  away_score: number;
  sport: string;
  ended_at: string;
  total_pnl: number;
  win_rate: number;
  total_trades: number;
  signals_generated: number;
}

export function HistoricalGamesPage() {
  const [games, setGames] = useState<ArchivedGame[]>([]);
  const [filters, setFilters] = useState({
    sport: 'all',
    outcome: 'all',  // profit, loss, break-even
    dateFrom: '',
    dateTo: ''
  });
  
  useEffect(() => {
    fetchGames();
  }, [filters]);
  
  const fetchGames = async () => {
    const params = new URLSearchParams(filters as any);
    const response = await fetch(`/api/historical/games?${params}`);
    const data = await response.json();
    setGames(data.games);
  };
  
  return (
    <div className="p-6 space-y-6">
      <h1 className="text-3xl font-bold">Historical Games</h1>
      
      {/* Filters */}
      <Card>
        <CardHeader>
          <CardTitle>Filters</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-4 gap-4">
            <Select
              value={filters.sport}
              onValueChange={(value) => setFilters({ ...filters, sport: value })}
            >
              <option value="all">All Sports</option>
              <option value="NBA">NBA</option>
              <option value="NFL">NFL</option>
              <option value="NHL">NHL</option>
            </Select>
            
            <Select
              value={filters.outcome}
              onValueChange={(value) => setFilters({ ...filters, outcome: value })}
            >
              <option value="all">All Outcomes</option>
              <option value="profit">Profitable</option>
              <option value="loss">Loss</option>
              <option value="break-even">Break-even</option>
            </Select>
            
            <Input
              type="date"
              placeholder="From"
              value={filters.dateFrom}
              onChange={(e) => setFilters({ ...filters, dateFrom: e.target.value })}
            />
            
            <Input
              type="date"
              placeholder="To"
              value={filters.dateTo}
              onChange={(e) => setFilters({ ...filters, dateTo: e.target.value })}
            />
          </div>
        </CardContent>
      </Card>
      
      {/* Summary Stats */}
      <div className="grid grid-cols-4 gap-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total Games</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{games.length}</div>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total P&L</CardTitle>
          </CardHeader>
          <CardContent>
            <div className={`text-2xl font-bold ${
              games.reduce((sum, g) => sum + g.total_pnl, 0) > 0 
                ? 'text-green-600' 
                : 'text-red-600'
            }`}>
              ${games.reduce((sum, g) => sum + g.total_pnl, 0).toFixed(2)}
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Avg Win Rate</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {(games.reduce((sum, g) => sum + g.win_rate, 0) / games.length * 100 || 0).toFixed(1)}%
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total Trades</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {games.reduce((sum, g) => sum + g.total_trades, 0)}
            </div>
          </CardContent>
        </Card>
      </div>
      
      {/* Games Table */}
      <Card>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Game</TableHead>
                <TableHead>Score</TableHead>
                <TableHead>Sport</TableHead>
                <TableHead>Date</TableHead>
                <TableHead>P&L</TableHead>
                <TableHead>Win Rate</TableHead>
                <TableHead>Trades</TableHead>
                <TableHead>Signals</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {games.map((game) => (
                <TableRow
                  key={game.game_id}
                  className="cursor-pointer hover:bg-gray-50"
                  onClick={() => window.location.href = `/historical/${game.game_id}`}
                >
                  <TableCell>
                    {game.away_team} @ {game.home_team}
                  </TableCell>
                  <TableCell>
                    {game.away_score} - {game.home_score}
                  </TableCell>
                  <TableCell>
                    <Badge>{game.sport}</Badge>
                  </TableCell>
                  <TableCell>
                    {new Date(game.ended_at).toLocaleDateString()}
                  </TableCell>
                  <TableCell>
                    <span className={game.total_pnl > 0 ? 'text-green-600' : 'text-red-600'}>
                      ${game.total_pnl.toFixed(2)}
                    </span>
                  </TableCell>
                  <TableCell>
                    {(game.win_rate * 100).toFixed(1)}%
                  </TableCell>
                  <TableCell>{game.total_trades}</TableCell>
                  <TableCell>{game.signals_generated}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
```

---

### Part 4: API Endpoints

#### File: `services/api/futures_endpoints.py` (NEW)

```python
"""API endpoints for futures tracking."""

from fastapi import APIRouter, Query
from datetime import datetime, timedelta
from typing import Optional

router = APIRouter(prefix="/api/futures", tags=["futures"])

@router.get("/games")
async def get_future_games(
    hours_ahead: int = Query(48, description="Hours to look ahead"),
    sport: Optional[str] = None
):
    """Get upcoming games being tracked for futures."""
    # Query database for future_games
    # Return list with latest prices and opportunities
    pass

@router.get("/games/{game_id}/prices")
async def get_future_game_prices(game_id: str):
    """Get price history for a future game."""
    # Query future_market_prices table
    # Return time-series data
    pass

@router.get("/games/{game_id}/signals")
async def get_futures_signals(game_id: str):
    """Get futures signals generated for a game."""
    # Query futures_signals table
    pass
```

#### File: `services/api/historical_endpoints.py` (NEW)

```python
"""API endpoints for historical games."""

from fastapi import APIRouter, Query
from datetime import datetime
from typing import Optional

router = APIRouter(prefix="/api/historical", tags=["historical"])

@router.get("/games")
async def get_historical_games(
    sport: Optional[str] = Query(None),
    outcome: Optional[str] = Query(None),  # profit, loss, break-even
    date_from: Optional[datetime] = Query(None),
    date_to: Optional[datetime] = Query(None),
    limit: int = Query(100, le=500)
):
    """Get archived games with filters."""
    # Query archived_games table with filters
    pass

@router.get("/games/{game_id}")
async def get_historical_game_detail(game_id: str):
    """Get detailed information for an archived game."""
    # Query archived_games
    # Include trades, signals, price history
    pass

@router.get("/games/{game_id}/trades")
async def get_game_trades(game_id: str):
    """Get all trades for an archived game."""
    # Query archived_trades
    pass

@router.get("/games/{game_id}/chart")
async def get_game_price_chart(game_id: str):
    """Get price history chart data for archived game."""
    # Query archived_market_prices
    # Return in format suitable for PropChart component
    pass
```

---

### Part 5: Docker Compose Updates

```yaml
# docker-compose.yml (additions)

services:
  # Existing services...
  
  # Futures Monitor
  futures_monitor:
    build:
      context: .
      dockerfile: services/futures_monitor/Dockerfile
    container_name: arbees_futures_monitor
    environment:
      - REDIS_URL=redis://redis:6379
      - POSTGRES_URL=postgresql://user:pass@postgres:5432/arbees
      - FUTURES_LOOKBACK_HOURS=48
      - FUTURES_POLL_INTERVAL=900  # 15 minutes
      - FUTURES_MIN_EDGE=0.05  # 5% minimum edge
      - LOG_LEVEL=INFO
    depends_on:
      - redis
      - postgres
    restart: unless-stopped
  
  # Game Archiver
  archiver:
    build:
      context: .
      dockerfile: services/archiver/Dockerfile
    container_name: arbees_archiver
    environment:
      - POSTGRES_URL=postgresql://user:pass@postgres:5432/arbees
      - ARCHIVE_GRACE_PERIOD=30  # Minutes after game ends
      - ARCHIVE_POLL_INTERVAL=300  # 5 minutes
      - LOG_LEVEL=INFO
    depends_on:
      - postgres
    restart: unless-stopped
```

---

## Implementation Checklist

### Phase 1: Futures Monitoring (Week 1)

- [ ] Create database tables (`future_games`, `future_market_prices`, `futures_signals`)
- [ ] Implement `FuturesMonitor` service
  - [ ] ESPN schedule polling
  - [ ] Market discovery for upcoming games
  - [ ] WebSocket price tracking
  - [ ] Futures signal generation
- [ ] Add API endpoints for futures
- [ ] Create `FuturesPage.tsx` frontend
- [ ] Test with upcoming games
- [ ] Add to Docker Compose
- [ ] Deploy and monitor

### Phase 2: Game Archival (Week 2)

- [ ] Create archival tables (`archived_games`, `archived_trades`, etc.)
- [ ] Implement `GameArchiver` service
  - [ ] Detect completed games
  - [ ] Calculate statistics
  - [ ] Archive to historical tables
  - [ ] Clean up active tables
- [ ] Add API endpoints for historical data
- [ ] Create `HistoricalGamesPage.tsx` frontend
  - [ ] Filters (sport, outcome, date range)
  - [ ] Summary statistics
  - [ ] Games table
  - [ ] Detail view with charts
- [ ] Test archival flow
- [ ] Add to Docker Compose
- [ ] Deploy and monitor

### Phase 3: Integration & Testing (Week 3)

- [ ] Test futures → live transition
- [ ] Test live → archival flow
- [ ] End-to-end test (futures to archival)
- [ ] Performance testing (database queries)
- [ ] UI/UX refinement
- [ ] Documentation

---

## Success Criteria

### Futures Tracking

- ✅ Discovers markets 24-48h before game start
- ✅ Tracks price movements with < 1 second latency
- ✅ Generates futures signals when edge > 5%
- ✅ Frontend shows upcoming games with countdown
- ✅ Price charts work (reuse existing components)
- ✅ Transitions to live monitoring when game starts

### Game Archival

- ✅ Automatically archives games 30 min after completion
- ✅ Calculates accurate P&L and win rate
- ✅ Historical page shows all past games
- ✅ Filters work (sport, outcome, date range)
- ✅ Detail view shows complete game analysis
- ✅ Database queries are fast (< 500ms)
- ✅ No memory leaks from stale data

---

## Testing Strategy

### Futures Monitor Testing

```python
# Test discovering upcoming games
games = await futures_monitor._discover_future_games()
assert len(games) > 0

# Test price tracking
await futures_monitor._subscribe_to_market(test_market)
price = await futures_monitor._handle_price_update(test_price)
assert price.market_id in futures_monitor.monitored_markets

# Test signal generation
signal = await futures_monitor._check_futures_opportunities(test_game)
assert signal.edge >= 0.05  # 5% minimum
```

### Game Archiver Testing

```python
# Test game archival
await archiver._archive_game(completed_game)

# Verify data moved
archived = await db.fetchrow("SELECT * FROM archived_games WHERE game_id = $1", game_id)
assert archived is not None

# Verify cleanup
active = await db.fetchrow("SELECT * FROM game_states WHERE game_id = $1", game_id)
assert active is None
```

---

## Performance Considerations

### Database Optimization

- Use TimescaleDB hypertables for price history (automatic compression)
- Index on frequently queried fields (game_id, ended_at, sport)
- Partition archived tables by month if volume is high
- Regular VACUUM and ANALYZE on PostgreSQL

### Memory Management

- Futures monitor should limit to 100 tracked games max
- Archiver should batch operations (archive 10 games at a time)
- Use connection pooling for database

---

## Begin Implementation

Start with:
1. Create database migrations (run SQL scripts)
2. Implement FuturesMonitor service (skeleton first)
3. Test with one upcoming game
4. Build frontend incrementally
5. Move to archiver after futures working

This gives you early price tracking (capture opportunities before public) and clean historical analysis (learn from past performance).
