# Claude Code Prompt: Futures Tracking & Game Lifecycle Management

## Overview

Implement two major features to expand Arbees' trading capabilities:

1. **Futures/Pre-Game Prop Tracking** - Monitor and trade on upcoming games before they start
2. **Game Lifecycle Management** - Properly archive completed games and enable historical analysis

These features will:
- Capture early pricing inefficiencies (futures markets often mispriced)
- Clean up live game clutter
- Enable post-game analysis and ML training data

---

## PART 1: Futures/Pre-Game Prop Tracking

### Business Value

**Why this matters:**
- Markets for upcoming games often list 24-48 hours early
- **Early pricing is less efficient** - fewer bettors, more mistakes
- Opening lines can shift 5-10% before game starts
- Opportunity to capture edge before sharp money arrives
- Can build positions at favorable prices

**Example opportunity:**
```
Tuesday 3pm: Lakers @ Celtics game (Friday 7pm) markets listed
  - Opening line: Lakers 55% ($0.55)
  - Your model: Lakers 62% (based on recent form, injuries)
  - Edge: 7% (huge!)
  - Action: Buy Lakers YES @ $0.55

Friday 6pm (1 hour before game):
  - Line moved to Lakers 60% ($0.60) (sharp money agreed with you)
  - Your position: Bought @ $0.55, now worth $0.60
  - Already +9% profit before game even starts!
  - Option: Close position for guaranteed profit OR hold for game
```

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Futures Monitor Service (NEW)                          │
│  ├─ Polls ESPN for upcoming games (24-48h ahead)        │
│  ├─ Discovers markets as soon as they're listed         │
│  ├─ Subscribes to WebSocket for price updates           │
│  ├─ Generates early signals (model vs opening line)     │
│  └─ Hands off to GameShard when game goes live          │
└─────────────────────────────────────────────────────────┘
          ↓ (stores to DB)
┌─────────────────────────────────────────────────────────┐
│  PostgreSQL: futures_games table                        │
│  ├─ game_id, start_time, status (upcoming/live/ended)  │
│  ├─ discovered_at, markets_found_at                     │
│  ├─ opening_line, current_line, line_movement           │
│  └─ positions opened (if we traded early)               │
└─────────────────────────────────────────────────────────┘
          ↓ (displays in)
┌─────────────────────────────────────────────────────────┐
│  Frontend: Futures Page (NEW)                           │
│  ├─ Shows upcoming games (next 48 hours)                │
│  ├─ Displays line movement charts (same as live games)  │
│  ├─ Highlights early opportunities                       │
│  ├─ Shows time until game starts                        │
│  └─ Toggle to show/hide past futures                    │
└─────────────────────────────────────────────────────────┘
```

### Implementation Details

#### 1. Create Futures Monitor Service

**File:** `services/futures_monitor/monitor.py`

```python
"""
Futures Monitor Service

Discovers and monitors upcoming games before they go live.
Captures early pricing inefficiencies.
"""

import asyncio
from datetime import datetime, timedelta
from typing import List, Optional
from loguru import logger

from data_providers.espn.client import ESPNClient
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from arbees_shared.models.game import Sport, GameState
from arbees_shared.db.connection import DatabaseClient


class FuturesMonitor:
    """
    Monitor upcoming games and discover markets early.
    
    Workflow:
    1. Poll ESPN for games in next 48 hours
    2. For each game, check if markets exist yet
    3. When markets found, subscribe to price updates
    4. Generate signals on opening lines
    5. Hand off to GameShard when game goes live
    """
    
    def __init__(
        self,
        sports: List[Sport],
        poll_interval_seconds: int = 300,  # 5 minutes
        lookback_hours: int = 48,
    ):
        self.sports = sports
        self.poll_interval = poll_interval_seconds
        self.lookback_hours = lookback_hours
        
        # Clients
        self.espn_clients = {
            sport: ESPNClient(sport) for sport in sports
        }
        self.kalshi = KalshiClient()
        self.polymarket = PolymarketClient()
        self.db: Optional[DatabaseClient] = None
        
        # State tracking
        self.tracked_games: dict[str, FuturesGame] = {}
        self.markets_discovered: set[str] = set()
    
    async def connect(self):
        """Initialize connections."""
        logger.info("Connecting Futures Monitor...")
        
        # Connect ESPN clients
        for sport, client in self.espn_clients.items():
            await client.connect()
            logger.info(f"✅ ESPN {sport.value} connected")
        
        # Connect market clients
        await self.kalshi.connect()
        logger.info("✅ Kalshi connected")
        
        await self.polymarket.connect()
        logger.info("✅ Polymarket connected")
        
        # Connect database
        from arbees_shared.db.connection import get_pool
        pool = await get_pool()
        self.db = DatabaseClient(pool)
        logger.info("✅ Database connected")
    
    async def run(self):
        """Main monitoring loop."""
        logger.info(
            f"Starting Futures Monitor (poll every {self.poll_interval}s, "
            f"lookback {self.lookback_hours}h)"
        )
        
        while True:
            try:
                await self._poll_upcoming_games()
                await self._discover_markets()
                await self._check_handoff_to_live()
                
            except Exception as e:
                logger.error(f"Error in futures monitor loop: {e}")
            
            await asyncio.sleep(self.poll_interval)
    
    async def _poll_upcoming_games(self):
        """Get upcoming games from ESPN."""
        logger.debug("Polling for upcoming games...")
        
        cutoff = datetime.utcnow() + timedelta(hours=self.lookback_hours)
        
        for sport, espn in self.espn_clients.items():
            # Get schedule for next N days
            games = await espn.get_upcoming_games(
                start_date=datetime.utcnow(),
                end_date=cutoff
            )
            
            for game_id, game_info in games.items():
                if game_id not in self.tracked_games:
                    # New upcoming game
                    futures_game = FuturesGame(
                        game_id=game_id,
                        sport=sport,
                        home_team=game_info["home_team"],
                        away_team=game_info["away_team"],
                        start_time=game_info["start_time"],
                        discovered_at=datetime.utcnow(),
                        status="upcoming",
                    )
                    
                    self.tracked_games[game_id] = futures_game
                    
                    # Store in database
                    await self._store_futures_game(futures_game)
                    
                    logger.info(
                        f"📅 New futures game: {futures_game.away_team} @ "
                        f"{futures_game.home_team} "
                        f"(starts in {self._format_time_until(futures_game.start_time)})"
                    )
        
        logger.debug(f"Tracking {len(self.tracked_games)} upcoming games")
    
    async def _discover_markets(self):
        """
        Try to discover markets for upcoming games.
        
        Markets may not exist yet - keep trying until found.
        """
        for game_id, game in self.tracked_games.items():
            if game.markets_discovered:
                continue  # Already found markets
            
            # Try Kalshi
            kalshi_market = await self._find_kalshi_market(game)
            if kalshi_market:
                game.kalshi_market_id = kalshi_market.ticker
                game.kalshi_opening_line = kalshi_market.yes_ask
                game.markets_found_at = datetime.utcnow()
                
                logger.info(
                    f"🎯 Kalshi market found: {game.home_team} vs {game.away_team} "
                    f"(opening line: {kalshi_market.yes_ask:.2%})"
                )
            
            # Try Polymarket
            poly_market = await self._find_polymarket_market(game)
            if poly_market:
                game.polymarket_market_id = poly_market.condition_id
                game.polymarket_opening_line = poly_market.yes_ask
                game.markets_found_at = datetime.utcnow()
                
                logger.info(
                    f"🎯 Polymarket market found: {game.home_team} vs {game.away_team} "
                    f"(opening line: {poly_market.yes_ask:.2%})"
                )
            
            # Update database if markets found
            if game.markets_discovered:
                await self._update_futures_game(game)
                
                # Subscribe to price updates
                await self._subscribe_to_futures_prices(game)
                
                # Generate opening line signal
                await self._generate_opening_signal(game)
    
    async def _generate_opening_signal(self, game: FuturesGame):
        """
        Generate signal based on opening line vs model.
        
        This is the key value-add: catching mispriced opening lines.
        """
        # Calculate model probability
        # (You'd use your actual model here)
        model_prob = await self._calculate_futures_model_prob(game)
        
        if model_prob is None:
            return
        
        # Compare to opening line
        if game.kalshi_opening_line:
            edge = model_prob - game.kalshi_opening_line
            
            if abs(edge) >= 0.05:  # 5% edge threshold for futures
                logger.info(
                    f"💰 FUTURES OPPORTUNITY: {game.home_team} "
                    f"(Model: {model_prob:.1%}, Market: {game.kalshi_opening_line:.1%}, "
                    f"Edge: {edge:+.1%})"
                )
                
                # Generate and store signal
                signal = FuturesSignal(
                    game_id=game.game_id,
                    team=game.home_team if edge > 0 else game.away_team,
                    action="BUY",
                    edge=edge,
                    model_prob=model_prob,
                    market_prob=game.kalshi_opening_line,
                    generated_at=datetime.utcnow(),
                    hours_before_game=self._hours_until(game.start_time),
                )
                
                await self._store_futures_signal(signal)
    
    async def _check_handoff_to_live(self):
        """
        Check if any futures games are now live.
        Hand them off to GameShard.
        """
        now = datetime.utcnow()
        
        for game_id, game in list(self.tracked_games.items()):
            time_until = (game.start_time - now).total_seconds()
            
            # Handoff when game starts (or 5 minutes before)
            if time_until <= 300:  # 5 minutes
                logger.info(
                    f"🔄 Handing off to live: {game.home_team} vs {game.away_team}"
                )
                
                # Update status
                game.status = "live"
                await self._update_futures_game(game)
                
                # Notify orchestrator to pick up this game
                await self._notify_orchestrator(game_id)
                
                # Remove from futures tracking
                del self.tracked_games[game_id]
    
    def _hours_until(self, start_time: datetime) -> float:
        """Calculate hours until game starts."""
        delta = start_time - datetime.utcnow()
        return delta.total_seconds() / 3600
    
    def _format_time_until(self, start_time: datetime) -> str:
        """Format time until game starts."""
        hours = self._hours_until(start_time)
        
        if hours < 1:
            return f"{int(hours * 60)}m"
        elif hours < 24:
            return f"{hours:.1f}h"
        else:
            days = hours / 24
            return f"{days:.1f}d"
    
    # ... additional helper methods ...
```

#### 2. Database Schema for Futures

**File:** `services/futures_monitor/schema.sql`

```sql
-- Futures games table
CREATE TABLE IF NOT EXISTS futures_games (
    game_id VARCHAR(50) PRIMARY KEY,
    sport VARCHAR(20) NOT NULL,
    home_team VARCHAR(100) NOT NULL,
    away_team VARCHAR(100) NOT NULL,
    start_time TIMESTAMP NOT NULL,
    
    -- Discovery tracking
    discovered_at TIMESTAMP NOT NULL,
    markets_found_at TIMESTAMP,
    
    -- Market info
    kalshi_market_id VARCHAR(100),
    kalshi_opening_line FLOAT,
    kalshi_current_line FLOAT,
    
    polymarket_market_id VARCHAR(100),
    polymarket_opening_line FLOAT,
    polymarket_current_line FLOAT,
    
    -- Status
    status VARCHAR(20) DEFAULT 'upcoming',  -- upcoming, live, ended
    
    -- Metadata
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX idx_futures_games_start_time ON futures_games(start_time);
CREATE INDEX idx_futures_games_status ON futures_games(status);

-- Futures price history (for line movement charts)
CREATE TABLE IF NOT EXISTS futures_price_history (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(50) REFERENCES futures_games(game_id),
    platform VARCHAR(20) NOT NULL,
    
    yes_bid FLOAT NOT NULL,
    yes_ask FLOAT NOT NULL,
    mid_price FLOAT NOT NULL,
    
    timestamp TIMESTAMP NOT NULL,
    hours_before_game FLOAT NOT NULL,
    
    CONSTRAINT fk_futures_game FOREIGN KEY (game_id) 
        REFERENCES futures_games(game_id) ON DELETE CASCADE
);

CREATE INDEX idx_futures_price_history_game ON futures_price_history(game_id);
CREATE INDEX idx_futures_price_history_timestamp ON futures_price_history(timestamp);

-- Futures signals (opportunities detected)
CREATE TABLE IF NOT EXISTS futures_signals (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(50) REFERENCES futures_games(game_id),
    
    team VARCHAR(100) NOT NULL,
    action VARCHAR(10) NOT NULL,
    
    edge FLOAT NOT NULL,
    model_prob FLOAT NOT NULL,
    market_prob FLOAT NOT NULL,
    
    generated_at TIMESTAMP NOT NULL,
    hours_before_game FLOAT NOT NULL,
    
    -- Execution tracking
    executed BOOLEAN DEFAULT FALSE,
    executed_at TIMESTAMP,
    execution_price FLOAT,
    
    CONSTRAINT fk_futures_signal_game FOREIGN KEY (game_id)
        REFERENCES futures_games(game_id) ON DELETE CASCADE
);

CREATE INDEX idx_futures_signals_game ON futures_signals(game_id);
CREATE INDEX idx_futures_signals_generated ON futures_signals(generated_at);
```

#### 3. Frontend: Futures Page

**File:** `frontend/src/pages/FuturesPage.tsx`

```typescript
/**
 * Futures Trading Page
 * 
 * Shows upcoming games with:
 * - Time until game starts
 * - Opening line vs current line (movement)
 * - Model probability vs market
 * - Line movement charts (same as live games)
 * - Active positions on futures
 */

import React, { useState, useEffect } from 'react';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { PropChart } from '@/components/PropChart';
import { formatDistanceToNow } from 'date-fns';

interface FuturesGame {
  game_id: string;
  sport: string;
  home_team: string;
  away_team: string;
  start_time: string;
  
  kalshi_opening_line?: number;
  kalshi_current_line?: number;
  polymarket_opening_line?: number;
  polymarket_current_line?: number;
  
  model_prob?: number;
  edge?: number;
  
  status: 'upcoming' | 'live' | 'ended';
}

export function FuturesPage() {
  const [games, setGames] = useState<FuturesGame[]>([]);
  const [selectedGame, setSelectedGame] = useState<string | null>(null);
  const [timeRange, setTimeRange] = useState<'24h' | '48h'>('48h');
  
  useEffect(() => {
    // Fetch futures games
    const fetchGames = async () => {
      const response = await fetch('/api/futures/games?range=' + timeRange);
      const data = await response.json();
      setGames(data.games);
    };
    
    fetchGames();
    const interval = setInterval(fetchGames, 30000); // Update every 30s
    
    return () => clearInterval(interval);
  }, [timeRange]);
  
  const getLineMovement = (game: FuturesGame) => {
    if (!game.kalshi_opening_line || !game.kalshi_current_line) {
      return null;
    }
    
    const movement = game.kalshi_current_line - game.kalshi_opening_line;
    return {
      value: movement,
      percentage: (movement / game.kalshi_opening_line) * 100,
      direction: movement > 0 ? 'up' : movement < 0 ? 'down' : 'flat',
    };
  };
  
  const getTimeUntilStart = (startTime: string) => {
    return formatDistanceToNow(new Date(startTime), { addSuffix: true });
  };
  
  return (
    <div className="p-6 space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-3xl font-bold">Futures Markets</h1>
        
        <Tabs value={timeRange} onValueChange={(v) => setTimeRange(v as any)}>
          <TabsList>
            <TabsTrigger value="24h">Next 24h</TabsTrigger>
            <TabsTrigger value="48h">Next 48h</TabsTrigger>
          </TabsList>
        </Tabs>
      </div>
      
      {/* Games Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {games.map((game) => {
          const movement = getLineMovement(game);
          const hasOpportunity = game.edge && Math.abs(game.edge) >= 0.05;
          
          return (
            <Card
              key={game.game_id}
              className={`cursor-pointer hover:shadow-lg transition-shadow ${
                hasOpportunity ? 'border-green-500 border-2' : ''
              }`}
              onClick={() => setSelectedGame(game.game_id)}
            >
              <CardHeader>
                <div className="flex justify-between items-start">
                  <CardTitle className="text-lg">
                    {game.away_team} @ {game.home_team}
                  </CardTitle>
                  <Badge variant={game.sport === 'NBA' ? 'default' : 'secondary'}>
                    {game.sport}
                  </Badge>
                </div>
                
                <p className="text-sm text-muted-foreground">
                  🕐 Starts {getTimeUntilStart(game.start_time)}
                </p>
              </CardHeader>
              
              <CardContent className="space-y-3">
                {/* Line Movement */}
                {movement && (
                  <div className="flex justify-between items-center p-2 bg-muted rounded">
                    <span className="text-sm">Line Movement:</span>
                    <span className={`text-sm font-mono font-bold ${
                      movement.direction === 'up' ? 'text-green-600' :
                      movement.direction === 'down' ? 'text-red-600' :
                      'text-gray-600'
                    }`}>
                      {movement.value > 0 ? '+' : ''}{(movement.value * 100).toFixed(1)}%
                    </span>
                  </div>
                )}
                
                {/* Current Line */}
                {game.kalshi_current_line && (
                  <div className="flex justify-between items-center">
                    <span className="text-sm text-muted-foreground">Current Line:</span>
                    <span className="text-sm font-mono">
                      {(game.kalshi_current_line * 100).toFixed(1)}%
                    </span>
                  </div>
                )}
                
                {/* Model vs Market */}
                {game.model_prob && (
                  <div className="flex justify-between items-center">
                    <span className="text-sm text-muted-foreground">Model:</span>
                    <span className="text-sm font-mono">
                      {(game.model_prob * 100).toFixed(1)}%
                    </span>
                  </div>
                )}
                
                {/* Edge */}
                {game.edge && (
                  <div className="flex justify-between items-center p-2 bg-muted rounded">
                    <span className="text-sm font-bold">Edge:</span>
                    <span className={`text-sm font-mono font-bold ${
                      Math.abs(game.edge) >= 0.05 ? 'text-green-600' : 'text-gray-600'
                    }`}>
                      {game.edge > 0 ? '+' : ''}{(game.edge * 100).toFixed(1)}%
                    </span>
                  </div>
                )}
                
                {hasOpportunity && (
                  <Badge variant="default" className="w-full justify-center">
                    💰 Opportunity Detected
                  </Badge>
                )}
              </CardContent>
            </Card>
          );
        })}
      </div>
      
      {/* Selected Game Detail View */}
      {selectedGame && (
        <Card className="mt-6">
          <CardHeader>
            <CardTitle>Line Movement Chart</CardTitle>
          </CardHeader>
          <CardContent>
            <PropChart
              gameId={selectedGame}
              showBothTeams={true}
              chartType="line"
              isFutures={true}  // New prop to fetch futures data
            />
          </CardContent>
        </Card>
      )}
    </div>
  );
}
```

#### 4. Docker Service Configuration

**File:** `docker-compose.yml` (add this service)

```yaml
services:
  # ... existing services ...
  
  futures_monitor:
    build:
      context: .
      dockerfile: services/futures_monitor/Dockerfile
    container_name: arbees_futures_monitor
    environment:
      - POSTGRES_URL=${POSTGRES_URL}
      - REDIS_URL=redis://redis:6379
      - KALSHI_API_KEY=${KALSHI_API_KEY}
      - POLYMARKET_PRIVATE_KEY=${POLYMARKET_PRIVATE_KEY}
      - ESPN_SPORTS=NBA,NFL,NHL  # Which sports to monitor
      - POLL_INTERVAL=300  # 5 minutes
      - LOOKBACK_HOURS=48  # How far ahead to look
    depends_on:
      - postgres
      - redis
    restart: unless-stopped
```

---

## PART 2: Game Lifecycle Management

### Business Value

**Why this matters:**
- Live games page gets cluttered with 20+ finished games
- Need historical data for ML model training
- Post-game analysis requires easy access to completed games
- Performance: Don't process games that are already over

**User experience:**
```
Before: Live games page shows 50 games (45 are finished!)
After:  Live games page shows 5 active games
        Historical page shows finished games with analysis
```

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Game Lifecycle States                                  │
│                                                          │
│  UPCOMING → LIVE → ENDED → ARCHIVED                     │
│     ↓        ↓      ↓         ↓                         │
│  Futures  GameShard  →  Historical DB                   │
└─────────────────────────────────────────────────────────┘

Flow:
1. Upcoming games: Monitored by FuturesMonitor
2. Live games: Picked up by GameShard when they start
3. Ended games: GameShard detects final and marks "ended"
4. Archived games: Archiver moves to historical tables after 1 hour
```

### Implementation Details

#### 1. Game Archiver Service

**File:** `services/archiver/archiver.py`

```python
"""
Game Archiver Service

Moves completed games to historical tables for analysis.
Keeps live games table clean.
"""

import asyncio
from datetime import datetime, timedelta
from loguru import logger

from arbees_shared.db.connection import DatabaseClient, get_pool


class GameArchiver:
    """
    Archive completed games to historical tables.
    
    Workflow:
    1. Find games with status='ended' that are >1 hour old
    2. Copy game data to historical tables
    3. Copy all related data (signals, trades, price history)
    4. Update game status to 'archived'
    5. Optionally: Delete from live tables (or keep with archived status)
    """
    
    def __init__(
        self,
        archive_delay_hours: int = 1,
        poll_interval_seconds: int = 300,  # 5 minutes
    ):
        self.archive_delay_hours = archive_delay_hours
        self.poll_interval = poll_interval_seconds
        self.db: Optional[DatabaseClient] = None
    
    async def connect(self):
        """Initialize database connection."""
        logger.info("Connecting Game Archiver...")
        pool = await get_pool()
        self.db = DatabaseClient(pool)
        logger.info("✅ Database connected")
    
    async def run(self):
        """Main archiving loop."""
        logger.info(
            f"Starting Game Archiver (check every {self.poll_interval}s, "
            f"archive after {self.archive_delay_hours}h)"
        )
        
        while True:
            try:
                archived_count = await self._archive_completed_games()
                
                if archived_count > 0:
                    logger.info(f"📦 Archived {archived_count} games")
                
            except Exception as e:
                logger.error(f"Error in archiver loop: {e}")
            
            await asyncio.sleep(self.poll_interval)
    
    async def _archive_completed_games(self) -> int:
        """
        Find and archive completed games.
        
        Returns number of games archived.
        """
        cutoff = datetime.utcnow() - timedelta(hours=self.archive_delay_hours)
        
        # Find games to archive
        query = """
            SELECT game_id, sport, home_team, away_team, 
                   home_score, away_score, status, ended_at
            FROM games
            WHERE status = 'ended'
              AND ended_at < $1
              AND archived_at IS NULL
        """
        
        games = await self.db.pool.fetch(query, cutoff)
        
        if not games:
            return 0
        
        logger.info(f"Found {len(games)} games to archive")
        
        for game in games:
            try:
                await self._archive_game(game)
            except Exception as e:
                logger.error(f"Error archiving game {game['game_id']}: {e}")
        
        return len(games)
    
    async def _archive_game(self, game: dict):
        """
        Archive a single game.
        
        Steps:
        1. Copy game record to historical_games
        2. Copy signals to historical_signals
        3. Copy trades to historical_trades
        4. Copy price history to historical_price_history
        5. Mark original game as archived
        """
        game_id = game['game_id']
        
        logger.info(
            f"Archiving: {game['away_team']} @ {game['home_team']} "
            f"({game['home_score']}-{game['away_score']})"
        )
        
        async with self.db.pool.acquire() as conn:
            async with conn.transaction():
                # 1. Copy game to historical_games
                await conn.execute("""
                    INSERT INTO historical_games 
                    SELECT * FROM games WHERE game_id = $1
                    ON CONFLICT (game_id) DO NOTHING
                """, game_id)
                
                # 2. Copy signals
                await conn.execute("""
                    INSERT INTO historical_signals
                    SELECT * FROM trading_signals WHERE game_id = $1
                    ON CONFLICT DO NOTHING
                """, game_id)
                
                # 3. Copy trades
                await conn.execute("""
                    INSERT INTO historical_trades
                    SELECT * FROM trades WHERE game_id = $1
                    ON CONFLICT DO NOTHING
                """, game_id)
                
                # 4. Copy price history
                await conn.execute("""
                    INSERT INTO historical_price_history
                    SELECT * FROM price_history WHERE game_id = $1
                    ON CONFLICT DO NOTHING
                """, game_id)
                
                # 5. Mark as archived
                await conn.execute("""
                    UPDATE games 
                    SET archived_at = NOW()
                    WHERE game_id = $1
                """, game_id)
        
        logger.info(f"✅ Archived game {game_id}")
```

#### 2. Database Schema for Historical Tables

**File:** `services/archiver/schema.sql`

```sql
-- Historical games (completed games)
CREATE TABLE IF NOT EXISTS historical_games (
    LIKE games INCLUDING ALL
);

-- Add archiving metadata
ALTER TABLE historical_games 
    ADD COLUMN IF NOT EXISTS archived_at TIMESTAMP DEFAULT NOW();

CREATE INDEX idx_historical_games_ended ON historical_games(ended_at);
CREATE INDEX idx_historical_games_sport ON historical_games(sport);
CREATE INDEX idx_historical_games_archived ON historical_games(archived_at);

-- Historical signals
CREATE TABLE IF NOT EXISTS historical_signals (
    LIKE trading_signals INCLUDING ALL
);

CREATE INDEX idx_historical_signals_game ON historical_signals(game_id);
CREATE INDEX idx_historical_signals_generated ON historical_signals(generated_at);

-- Historical trades
CREATE TABLE IF NOT EXISTS historical_trades (
    LIKE trades INCLUDING ALL
);

CREATE INDEX idx_historical_trades_game ON historical_trades(game_id);
CREATE INDEX idx_historical_trades_executed ON historical_trades(executed_at);

-- Historical price history
CREATE TABLE IF NOT EXISTS historical_price_history (
    LIKE price_history INCLUDING ALL
);

CREATE INDEX idx_historical_price_history_game ON historical_price_history(game_id);
CREATE INDEX idx_historical_price_history_timestamp ON historical_price_history(timestamp);

-- Add archived_at column to live games table
ALTER TABLE games 
    ADD COLUMN IF NOT EXISTS archived_at TIMESTAMP;

CREATE INDEX idx_games_archived ON games(archived_at) 
    WHERE archived_at IS NOT NULL;
```

#### 3. GameShard: Detect Game End

**File:** `services/game_shard/shard.py` (modifications)

```python
class GameShard:
    # ... existing code ...
    
    async def _handle_game_state_update(
        self,
        game_id: str,
        new_state: GameState,
        new_plays: List[Play]
    ):
        """Handle game state update."""
        
        # ... existing logic ...
        
        # NEW: Check if game ended
        if new_state.status in ["final", "closed", "completed"]:
            await self._handle_game_end(game_id, new_state)
    
    async def _handle_game_end(self, game_id: str, final_state: GameState):
        """
        Handle game ending.
        
        Actions:
        1. Mark game as ended in database
        2. Close any open positions
        3. Calculate final P&L
        4. Stop monitoring this game
        """
        logger.info(
            f"🏁 Game ended: {final_state.away_team} @ {final_state.home_team} "
            f"({final_state.away_score}-{final_state.home_score})"
        )
        
        # Update database
        if self.db:
            await self.db.pool.execute("""
                UPDATE games
                SET status = 'ended',
                    ended_at = NOW(),
                    final_score_home = $2,
                    final_score_away = $3
                WHERE game_id = $1
            """, game_id, final_state.home_score, final_state.away_score)
        
        # Close open positions
        if game_id in self.positions:
            await self._close_all_positions(game_id, final_state)
        
        # Calculate final P&L
        pnl = await self._calculate_final_pnl(game_id, final_state)
        logger.info(f"💰 Final P&L for {game_id}: ${pnl:.2f}")
        
        # Remove from active contexts
        if game_id in self._contexts:
            del self._contexts[game_id]
        
        # Unsubscribe from WebSocket
        await self._unsubscribe_markets(game_id)
    
    async def _close_all_positions(self, game_id: str, final_state: GameState):
        """Close all positions for a game at final score."""
        # Determine winner
        winner = (
            final_state.home_team 
            if final_state.home_score > final_state.away_score 
            else final_state.away_team
        )
        
        # Close each position
        for position in self.positions.get(game_id, []):
            final_value = 1.0 if position.team == winner else 0.0
            pnl = (final_value - position.entry_price) * position.size
            
            logger.info(
                f"Closed position: {position.team} "
                f"(entry: ${position.entry_price:.2f}, "
                f"final: ${final_value:.2f}, "
                f"P&L: ${pnl:+.2f})"
            )
            
            # Store in database
            if self.db:
                await self.db.pool.execute("""
                    UPDATE positions
                    SET closed_at = NOW(),
                        exit_price = $2,
                        pnl = $3
                    WHERE position_id = $1
                """, position.id, final_value, pnl)
```

#### 4. Frontend: Historical Games Page

**File:** `frontend/src/pages/HistoricalGamesPage.tsx`

```typescript
/**
 * Historical Games Page
 * 
 * Shows completed games with:
 * - Final scores
 * - Trades executed
 * - P&L summary
 * - Filtering by date, sport, outcome
 * - Line movement replay
 */

import React, { useState, useEffect } from 'react';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { format } from 'date-fns';

interface HistoricalGame {
  game_id: string;
  sport: string;
  home_team: string;
  away_team: string;
  home_score: number;
  away_score: number;
  ended_at: string;
  
  // Trading stats
  total_trades: number;
  total_pnl: number;
  win_rate: number;
}

export function HistoricalGamesPage() {
  const [games, setGames] = useState<HistoricalGame[]>([]);
  const [filters, setFilters] = useState({
    sport: 'all',
    dateFrom: '',
    dateTo: '',
    outcome: 'all',  // won, lost, all
  });
  
  useEffect(() => {
    fetchHistoricalGames();
  }, [filters]);
  
  const fetchHistoricalGames = async () => {
    const params = new URLSearchParams(filters);
    const response = await fetch(`/api/historical/games?${params}`);
    const data = await response.json();
    setGames(data.games);
  };
  
  return (
    <div className="p-6 space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-3xl font-bold">Historical Games</h1>
        
        {/* Filters */}
        <div className="flex gap-4">
          <Select
            value={filters.sport}
            onValueChange={(sport) => setFilters({ ...filters, sport })}
          >
            <option value="all">All Sports</option>
            <option value="NBA">NBA</option>
            <option value="NFL">NFL</option>
            <option value="NHL">NHL</option>
          </Select>
          
          <Input
            type="date"
            value={filters.dateFrom}
            onChange={(e) => setFilters({ ...filters, dateFrom: e.target.value })}
            placeholder="From"
          />
          
          <Input
            type="date"
            value={filters.dateTo}
            onChange={(e) => setFilters({ ...filters, dateTo: e.target.value })}
            placeholder="To"
          />
          
          <Select
            value={filters.outcome}
            onValueChange={(outcome) => setFilters({ ...filters, outcome })}
          >
            <option value="all">All Outcomes</option>
            <option value="won">Profitable</option>
            <option value="lost">Lost Money</option>
          </Select>
        </div>
      </div>
      
      {/* Games Table */}
      <Card>
        <CardContent className="p-0">
          <table className="w-full">
            <thead className="bg-muted">
              <tr>
                <th className="p-3 text-left">Game</th>
                <th className="p-3 text-left">Sport</th>
                <th className="p-3 text-left">Final Score</th>
                <th className="p-3 text-left">Date</th>
                <th className="p-3 text-right">Trades</th>
                <th className="p-3 text-right">P&L</th>
                <th className="p-3 text-right">Win Rate</th>
                <th className="p-3"></th>
              </tr>
            </thead>
            <tbody>
              {games.map((game) => (
                <tr key={game.game_id} className="border-b hover:bg-muted/50">
                  <td className="p-3">
                    {game.away_team} @ {game.home_team}
                  </td>
                  <td className="p-3">
                    <Badge>{game.sport}</Badge>
                  </td>
                  <td className="p-3 font-mono">
                    {game.away_score} - {game.home_score}
                  </td>
                  <td className="p-3 text-sm text-muted-foreground">
                    {format(new Date(game.ended_at), 'MMM d, yyyy h:mm a')}
                  </td>
                  <td className="p-3 text-right font-mono">
                    {game.total_trades}
                  </td>
                  <td className={`p-3 text-right font-mono font-bold ${
                    game.total_pnl > 0 ? 'text-green-600' :
                    game.total_pnl < 0 ? 'text-red-600' :
                    'text-gray-600'
                  }`}>
                    ${game.total_pnl.toFixed(2)}
                  </td>
                  <td className="p-3 text-right font-mono">
                    {(game.win_rate * 100).toFixed(0)}%
                  </td>
                  <td className="p-3">
                    <button
                      onClick={() => window.location.href = `/historical/${game.game_id}`}
                      className="text-sm text-blue-600 hover:underline"
                    >
                      View Details →
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </CardContent>
      </Card>
      
      {/* Summary Stats */}
      <div className="grid grid-cols-4 gap-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total Games</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">{games.length}</p>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total P&L</CardTitle>
          </CardHeader>
          <CardContent>
            <p className={`text-3xl font-bold ${
              games.reduce((sum, g) => sum + g.total_pnl, 0) > 0
                ? 'text-green-600' : 'text-red-600'
            }`}>
              ${games.reduce((sum, g) => sum + g.total_pnl, 0).toFixed(2)}
            </p>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Avg Win Rate</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {(games.reduce((sum, g) => sum + g.win_rate, 0) / games.length * 100).toFixed(0)}%
            </p>
          </CardContent>
        </Card>
        
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Total Trades</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {games.reduce((sum, g) => sum + g.total_trades, 0)}
            </p>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
```

#### 5. API Endpoints

**File:** `services/api/historical_routes.py`

```python
from fastapi import APIRouter, Query
from datetime import datetime
from typing import Optional

router = APIRouter(prefix="/api/historical", tags=["historical"])

@router.get("/games")
async def get_historical_games(
    sport: Optional[str] = Query(None),
    date_from: Optional[datetime] = Query(None),
    date_to: Optional[datetime] = Query(None),
    outcome: Optional[str] = Query(None),  # won, lost, all
    limit: int = Query(100, le=1000),
):
    """Get historical games with filters."""
    
    query = """
        SELECT 
            g.game_id,
            g.sport,
            g.home_team,
            g.away_team,
            g.home_score,
            g.away_score,
            g.ended_at,
            COUNT(DISTINCT t.trade_id) as total_trades,
            COALESCE(SUM(t.pnl), 0) as total_pnl,
            CASE 
                WHEN COUNT(t.trade_id) > 0 
                THEN COUNT(CASE WHEN t.pnl > 0 THEN 1 END)::FLOAT / COUNT(t.trade_id)
                ELSE 0
            END as win_rate
        FROM historical_games g
        LEFT JOIN historical_trades t ON g.game_id = t.game_id
        WHERE 1=1
    """
    
    params = []
    
    if sport and sport != "all":
        query += " AND g.sport = $" + str(len(params) + 1)
        params.append(sport)
    
    if date_from:
        query += " AND g.ended_at >= $" + str(len(params) + 1)
        params.append(date_from)
    
    if date_to:
        query += " AND g.ended_at <= $" + str(len(params) + 1)
        params.append(date_to)
    
    query += """
        GROUP BY g.game_id, g.sport, g.home_team, g.away_team, 
                 g.home_score, g.away_score, g.ended_at
    """
    
    if outcome == "won":
        query += " HAVING SUM(t.pnl) > 0"
    elif outcome == "lost":
        query += " HAVING SUM(t.pnl) < 0"
    
    query += " ORDER BY g.ended_at DESC"
    query += " LIMIT $" + str(len(params) + 1)
    params.append(limit)
    
    games = await db.fetch(query, *params)
    
    return {"games": games}

@router.get("/games/{game_id}")
async def get_historical_game_detail(game_id: str):
    """Get detailed analysis for a specific game."""
    
    # Game info
    game = await db.fetchrow("""
        SELECT * FROM historical_games WHERE game_id = $1
    """, game_id)
    
    # Trades
    trades = await db.fetch("""
        SELECT * FROM historical_trades 
        WHERE game_id = $1 
        ORDER BY executed_at
    """, game_id)
    
    # Signals
    signals = await db.fetch("""
        SELECT * FROM historical_signals 
        WHERE game_id = $1 
        ORDER BY generated_at
    """, game_id)
    
    # Price history
    price_history = await db.fetch("""
        SELECT * FROM historical_price_history 
        WHERE game_id = $1 
        ORDER BY timestamp
    """, game_id)
    
    return {
        "game": game,
        "trades": trades,
        "signals": signals,
        "price_history": price_history,
    }
```

---

## Implementation Checklist

### Part 1: Futures Tracking

- [ ] Create `services/futures_monitor/` directory
- [ ] Implement `monitor.py` with FuturesMonitor class
- [ ] Create futures database tables (futures_games, futures_price_history, futures_signals)
- [ ] Add Dockerfile for futures_monitor service
- [ ] Update docker-compose.yml with futures_monitor service
- [ ] Create `frontend/src/pages/FuturesPage.tsx`
- [ ] Add API routes for futures data (`/api/futures/games`, etc.)
- [ ] Update PropChart component to support `isFutures` prop
- [ ] Test with upcoming games (verify markets discovered early)
- [ ] Verify handoff to GameShard when games go live

### Part 2: Game Lifecycle Management

- [ ] Create `services/archiver/` directory
- [ ] Implement `archiver.py` with GameArchiver class
- [ ] Create historical database tables (historical_games, historical_signals, etc.)
- [ ] Add Dockerfile for archiver service
- [ ] Update docker-compose.yml with archiver service
- [ ] Modify GameShard to detect game end and mark status='ended'
- [ ] Implement position closing logic on game end
- [ ] Create `frontend/src/pages/HistoricalGamesPage.tsx`
- [ ] Add API routes for historical data (`/api/historical/games`, etc.)
- [ ] Add filters (sport, date range, outcome)
- [ ] Test archiving process (verify data copied correctly)
- [ ] Verify live games page only shows active games

---

## Testing Strategy

### Futures Testing

1. **Market Discovery Test**
   - Wait for upcoming game (24-48h ahead)
   - Verify FuturesMonitor discovers it
   - Check markets found on Kalshi/Polymarket
   - Verify opening line recorded

2. **Price Tracking Test**
   - Monitor line movement over time
   - Verify price history stored
   - Check charts display correctly

3. **Handoff Test**
   - Wait for game to start
   - Verify handoff to GameShard
   - Check game removed from futures
   - Verify GameShard picks it up

### Archiving Testing

1. **Game End Detection Test**
   - Wait for live game to end
   - Verify GameShard marks status='ended'
   - Check positions closed
   - Verify final P&L calculated

2. **Archiving Test**
   - Wait 1 hour after game ends
   - Verify archiver copies data
   - Check historical tables populated
   - Verify game marked archived

3. **Historical Page Test**
   - Navigate to historical page
   - Verify completed games displayed
   - Test filters (sport, date, outcome)
   - Check P&L calculations correct

---

## Success Criteria

### Futures Feature

- [ ] Upcoming games discovered 24-48h early
- [ ] Markets found as soon as they list
- [ ] Line movement tracked continuously
- [ ] Early signals generated on opening lines
- [ ] Futures page displays upcoming games
- [ ] Handoff to live games works smoothly

### Archiving Feature

- [ ] Games automatically marked 'ended' when finished
- [ ] Positions closed at final score
- [ ] Data archived 1 hour after game ends
- [ ] Live games page only shows active games
- [ ] Historical page shows completed games
- [ ] Filters work correctly
- [ ] P&L calculations accurate

---

## Performance Considerations

1. **Futures Monitor**: Poll every 5 minutes (not too aggressive)
2. **Archiver**: Run every 5 minutes (low overhead)
3. **Historical Queries**: Add indexes on ended_at, sport, archived_at
4. **Price History**: Use TimescaleDB for efficient time-series queries
5. **Frontend**: Paginate historical games (100 per page)

---

## Security & Data Integrity

1. **Use transactions** when archiving (all-or-nothing)
2. **Add foreign keys** to maintain referential integrity
3. **Soft delete** - Keep original data even after archiving
4. **Validate** game status transitions (upcoming → live → ended → archived)
5. **Log** all archiving operations for audit trail

---

## Begin Implementation

Start by creating the directory structure and implementing the FuturesMonitor service. Test with upcoming games to verify market discovery works before moving on to the archiving feature.

The two features complement each other well:
- Futures expands trading to earlier opportunities
- Archiving keeps the system clean and enables analysis

Both are essential for a production trading system!
