       Comprehensive Implementation Plan: IDBGfSportsBetS to Arbees Migration

       Executive Summary

       This migration is not merely porting code - it is a complete architectural redesign with the following key improvements:

       1. GameShard Architecture - Replace container-per-game (expensive) with asyncio-based multi-game handling in single processes
       2. TimescaleDB - Replace InfluxDB with PostgreSQL + TimescaleDB for relational + time-series queries
       3. Enhanced Rust Core - Expand PyO3 bindings to include win probability models and real-time signal generation
       4. Proper Async - Modern Python with structured concurrency, dependency injection, and Pydantic v2
       5. Multi-Region AWS - Terraform-based deployment with US/EU regions for Polymarket compliance

       ---
       Phase 1: Project Scaffold and Rust Core (Week 1-2)

       1.1 Project Structure

       Create the foundational directory structure with proper Python packaging:

       Arbees/
       ├── pyproject.toml           # UV/Poetry project with workspace
       ├── Cargo.toml               # Rust workspace
       ├── docker-compose.yml       # Local dev environment
       ├── docker-compose.prod.yml  # Production config
       │
       ├── rust_core/
       │   ├── Cargo.toml
       │   ├── pyproject.toml       # Maturin config
       │   └── src/
       │       ├── lib.rs           # PyO3 module entry
       │       ├── arb.rs           # Arbitrage detection (port existing)
       │       ├── win_prob.rs      # Win probability models (NEW - port from Python)
       │       ├── signal.rs        # Signal generation (NEW)
       │       └── types.rs         # Shared Rust types
       │
       ├── shared/
       │   ├── pyproject.toml       # Installable package
       │   └── arbees_shared/
       │       ├── __init__.py
       │       ├── core.pyi         # Type stubs for rust_core
       │       ├── models/          # Pydantic v2 models
       │       ├── messaging/       # Redis pub/sub
       │       ├── db/              # TimescaleDB + Redis
       │       └── config/          # Settings with pydantic-settings

       1.2 Rust Core Improvements

       Current state (old project):
       - lib.rs with 366 lines - basic arbitrage detection, model edges, lagging market detection
       - Uses serde_json for Python/Rust data exchange (inefficient)

       Improvements:
       1. Type-safe bindings - Define proper Rust structs with PyO3 #[pyclass] instead of JSON conversion
       2. Win probability in Rust - Port NFL/NBA/NHL models for 10-100x speedup
       3. SIMD optimization - Use Rust SIMD for batch probability calculations
       4. Streaming support - Support incremental data updates, not just batch processing

       Key Files to Create:

       // rust_core/src/types.rs
       #[pyclass]
       pub struct GameState {
           pub game_id: String,
           pub sport: Sport,
           pub home_score: i32,
           pub away_score: i32,
           pub period: i32,
           pub time_remaining_secs: i32,
           // ... efficient native types
       }

       #[pyclass]
       pub struct ArbitrageOpportunity {
           pub edge_pct: f64,
           pub platform_buy: String,
           pub platform_sell: String,
           pub buy_price: f64,
           pub sell_price: f64,
           pub confidence: f64,
       }

       // rust_core/src/win_prob.rs
       pub fn calculate_win_probability(state: &GameState, team: &str) -> f64 {
           match state.sport {
               Sport::NFL => nfl_win_prob(state, team),
               Sport::NBA => nba_win_prob(state, team),
               Sport::NHL => nhl_win_prob(state, team),
               // ...
           }
       }

       1.3 Verification Steps

       # Build and test Rust core
       cd rust_core
       maturin develop --release
       python -c "import arbees_core; print(arbees_core.calculate_win_probability(...))"

       # Run Rust tests
       cargo test

       # Benchmark vs Python implementation
       python benchmarks/rust_vs_python.py

       ---
       Phase 2: Shared Library and Database (Week 2-3)

       2.1 Pydantic v2 Models

       Improvements over old models:
       - Use Pydantic v2's model_validator for complex validation
       - Leverage ConfigDict for serialization settings
       - Use discriminated unions for polymorphic types

       # shared/arbees_shared/models/game.py
       from datetime import datetime
       from enum import Enum
       from typing import Optional, Literal
       from pydantic import BaseModel, Field, ConfigDict, computed_field

       class Sport(str, Enum):
           NFL = "nfl"
           NBA = "nba"
           NHL = "nhl"
           MLB = "mlb"
           NCAAF = "ncaaf"
           NCAAB = "ncaab"

       class GameState(BaseModel):
           model_config = ConfigDict(
               frozen=True,  # Immutable for caching
               str_strip_whitespace=True,
               use_enum_values=True,
           )

           game_id: str
           sport: Sport
           home_team: str
           away_team: str
           home_score: int = 0
           away_score: int = 0
           period: int = 1
           time_remaining_secs: int
           possession: Optional[str] = None
           game_status: Literal["scheduled", "in_progress", "halftime", "final"] = "scheduled"
           timestamp: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))

           @computed_field
           @property
           def score_differential(self) -> int:
               return self.home_score - self.away_score

           @computed_field
           @property
           def game_progress_pct(self) -> float:
               total_secs = self._total_game_seconds()
               elapsed = total_secs - self.time_remaining_secs
               return min(1.0, max(0.0, elapsed / total_secs))

       2.2 TimescaleDB Schema

       Key improvement: Single unified schema instead of separate InfluxDB measurements.

       -- shared/arbees_shared/db/migrations/001_initial.sql

       -- Enable TimescaleDB extension
       CREATE EXTENSION IF NOT EXISTS timescaledb;

       -- Core tables
       CREATE TABLE sports (
           id SERIAL PRIMARY KEY,
           code VARCHAR(10) UNIQUE NOT NULL,  -- nfl, nba, etc.
           name VARCHAR(100) NOT NULL
       );

       CREATE TABLE teams (
           id SERIAL PRIMARY KEY,
           sport_id INTEGER REFERENCES sports(id),
           abbreviation VARCHAR(10) NOT NULL,
           name VARCHAR(100) NOT NULL,
           UNIQUE(sport_id, abbreviation)
       );

       CREATE TABLE games (
           id SERIAL PRIMARY KEY,
           external_id VARCHAR(50) UNIQUE NOT NULL,  -- ESPN game_id
           sport_id INTEGER REFERENCES sports(id),
           home_team_id INTEGER REFERENCES teams(id),
           away_team_id INTEGER REFERENCES teams(id),
           scheduled_time TIMESTAMPTZ NOT NULL,
           status VARCHAR(20) DEFAULT 'scheduled',
           venue VARCHAR(200),
           created_at TIMESTAMPTZ DEFAULT NOW()
       );

       -- Time-series tables (hypertables)
       CREATE TABLE game_states (
           time TIMESTAMPTZ NOT NULL,
           game_id INTEGER REFERENCES games(id),
           home_score INTEGER NOT NULL,
           away_score INTEGER NOT NULL,
           period INTEGER NOT NULL,
           time_remaining_secs INTEGER NOT NULL,
           possession VARCHAR(10),
           -- NFL specific
           down INTEGER,
           yards_to_go INTEGER,
           yard_line INTEGER,
           -- NBA specific
           shot_clock INTEGER,
           home_fouls INTEGER,
           away_fouls INTEGER,
           -- Computed columns
           win_probability_home FLOAT,
           win_probability_away FLOAT,
           PRIMARY KEY (time, game_id)
       );
       SELECT create_hypertable('game_states', 'time');

       CREATE TABLE plays (
           time TIMESTAMPTZ NOT NULL,
           game_id INTEGER REFERENCES games(id),
           play_id VARCHAR(100) NOT NULL,
           play_type VARCHAR(50) NOT NULL,
           description TEXT,
           team VARCHAR(10),
           scoring_play BOOLEAN DEFAULT FALSE,
           points_scored INTEGER DEFAULT 0,
           yards_gained INTEGER,
           PRIMARY KEY (time, game_id, play_id)
       );
       SELECT create_hypertable('plays', 'time');

       CREATE TABLE market_prices (
           time TIMESTAMPTZ NOT NULL,
           game_id INTEGER REFERENCES games(id),
           platform VARCHAR(20) NOT NULL,  -- kalshi, polymarket
           market_id VARCHAR(100) NOT NULL,
           team VARCHAR(10) NOT NULL,
           yes_bid FLOAT,
           yes_ask FLOAT,
           no_bid FLOAT,
           no_ask FLOAT,
           volume FLOAT,
           PRIMARY KEY (time, game_id, platform, team)
       );
       SELECT create_hypertable('market_prices', 'time');

       CREATE TABLE arbitrage_opportunities (
           time TIMESTAMPTZ NOT NULL,
           game_id INTEGER REFERENCES games(id),
           opportunity_type VARCHAR(50) NOT NULL,
           platform_buy VARCHAR(20) NOT NULL,
           platform_sell VARCHAR(20) NOT NULL,
           edge_pct FLOAT NOT NULL,
           buy_price FLOAT NOT NULL,
           sell_price FLOAT NOT NULL,
           recommended_size FLOAT,
           confidence FLOAT,
           status VARCHAR(20) DEFAULT 'active',
           PRIMARY KEY (time, game_id, opportunity_type, platform_buy, platform_sell)
       );
       SELECT create_hypertable('arbitrage_opportunities', 'time');

       CREATE TABLE paper_trades (
           id SERIAL PRIMARY KEY,
           created_at TIMESTAMPTZ DEFAULT NOW(),
           opportunity_id VARCHAR(100),
           game_id INTEGER REFERENCES games(id),
           platform VARCHAR(20) NOT NULL,
           side VARCHAR(10) NOT NULL,  -- buy/sell
           stake FLOAT NOT NULL,
           entry_price FLOAT NOT NULL,
           exit_price FLOAT,
           edge_at_entry FLOAT,
           pnl FLOAT,
           status VARCHAR(20) DEFAULT 'open',  -- open, won, lost, push
           closed_at TIMESTAMPTZ
       );

       -- Indexes for common queries
       CREATE INDEX idx_game_states_game ON game_states (game_id, time DESC);
       CREATE INDEX idx_market_prices_platform ON market_prices (platform, time DESC);
       CREATE INDEX idx_opportunities_edge ON arbitrage_opportunities (edge_pct DESC) WHERE status = 'active';

       -- Continuous aggregates for dashboards
       CREATE MATERIALIZED VIEW daily_trading_stats
       WITH (timescaledb.continuous) AS
       SELECT
           time_bucket('1 day', created_at) AS day,
           platform,
           COUNT(*) AS total_trades,
           SUM(CASE WHEN status = 'won' THEN 1 ELSE 0 END) AS wins,
           SUM(CASE WHEN status = 'lost' THEN 1 ELSE 0 END) AS losses,
           SUM(pnl) AS total_pnl,
           AVG(edge_at_entry) AS avg_edge
       FROM paper_trades
       WHERE status IN ('won', 'lost', 'push')
       GROUP BY day, platform;

       2.3 Redis Messaging

       Improvement: Proper pub/sub with typed messages instead of ad-hoc communication.

       # shared/arbees_shared/messaging/redis_bus.py
       from typing import TypeVar, Generic, Callable, Awaitable
       from pydantic import BaseModel
       import redis.asyncio as redis
       import msgpack

       T = TypeVar('T', bound=BaseModel)

       class MessageBus:
           """Type-safe Redis pub/sub messaging."""

           def __init__(self, redis_url: str):
               self.redis = redis.from_url(redis_url)
               self._handlers: dict[str, list[Callable]] = {}

           async def publish(self, channel: str, message: BaseModel) -> None:
               """Publish a Pydantic model to a channel."""
               payload = msgpack.packb(message.model_dump(mode='json'))
               await self.redis.publish(channel, payload)

           async def subscribe(
               self,
               channel: str,
               message_type: type[T],
               handler: Callable[[T], Awaitable[None]]
           ) -> None:
               """Subscribe to a channel with typed message handling."""
               pubsub = self.redis.pubsub()
               await pubsub.subscribe(channel)

               async for msg in pubsub.listen():
                   if msg['type'] == 'message':
                       data = msgpack.unpackb(msg['data'])
                       parsed = message_type.model_validate(data)
                       await handler(parsed)

       # Channel definitions
       class Channels:
           GAME_STATE_UPDATE = "game:{game_id}:state"
           PLAY_DETECTED = "game:{game_id}:play"
           MARKET_PRICE = "market:{platform}:price"
           SIGNAL_GENERATED = "signals:new"
           TRADE_EXECUTED = "trades:executed"

       2.4 Verification Steps

       # Run migrations
       alembic upgrade head

       # Test database connection
       python -c "from arbees_shared.db import get_db; ..."

       # Test Redis messaging
       python -c "from arbees_shared.messaging import MessageBus; ..."

       # Run integration tests
       pytest tests/integration/test_database.py
       pytest tests/integration/test_messaging.py

       ---
       Phase 3: Market Clients (Week 3-4)

       3.1 Async Market Client Base

       Improvement: Proper async/await with connection pooling and retry logic.

       # markets/base.py
       from abc import ABC, abstractmethod
       from typing import AsyncIterator, Optional
       from contextlib import asynccontextmanager
       import aiohttp
       from tenacity import retry, stop_after_attempt, wait_exponential

       from arbees_shared.models import MarketPrice, OrderBook

       class BaseMarketClient(ABC):
           """Base class for all market integrations."""

           def __init__(self, api_key: str, base_url: str):
               self.api_key = api_key
               self.base_url = base_url
               self._session: Optional[aiohttp.ClientSession] = None
               self._rate_limiter = AsyncRateLimiter(max_rate=10, per_second=1)

           @asynccontextmanager
           async def session(self):
               """Managed HTTP session with connection pooling."""
               if self._session is None:
                   connector = aiohttp.TCPConnector(
                       limit=100,
                       ttl_dns_cache=300,
                       enable_cleanup_closed=True
                   )
                   self._session = aiohttp.ClientSession(
                       connector=connector,
                       headers=self._auth_headers(),
                       timeout=aiohttp.ClientTimeout(total=30)
                   )
               try:
                   yield self._session
               except Exception:
                   raise

           @abstractmethod
           def _auth_headers(self) -> dict:
               """Return authentication headers."""
               pass

           @abstractmethod
           async def get_markets(self, **filters) -> list[dict]:
               """Fetch available markets."""
               pass

           @abstractmethod
           async def get_orderbook(self, market_id: str) -> OrderBook:
               """Fetch order book for a market."""
               pass

           @abstractmethod
           async def stream_prices(self, market_ids: list[str]) -> AsyncIterator[MarketPrice]:
               """Stream real-time price updates."""
               pass

           @retry(stop=stop_after_attempt(3), wait=wait_exponential(multiplier=1, max=10))
           async def _request(self, method: str, endpoint: str, **kwargs) -> dict:
               """Make rate-limited, retrying HTTP request."""
               async with self._rate_limiter:
                   async with self.session() as session:
                       async with session.request(method, f"{self.base_url}{endpoint}", **kwargs) as resp:
                           resp.raise_for_status()
                           return await resp.json()

       3.2 Kalshi Client (Improved)

       Improvements over old client:
       - Proper RSA signature caching
       - WebSocket support for real-time prices
       - Better error handling and retry logic

       # markets/kalshi/client.py
       from datetime import datetime
       import time
       import base64
       from cryptography.hazmat.primitives import hashes, serialization
       from cryptography.hazmat.primitives.asymmetric import padding

       from ..base import BaseMarketClient
       from arbees_shared.models import MarketPrice, OrderBook

       class KalshiClient(BaseMarketClient):
           """Kalshi API client with RSA authentication."""

           BASE_URL = "https://api.elections.kalshi.com/trade-api/v2"
           WS_URL = "wss://api.elections.kalshi.com/trade-api/ws/v2"

           def __init__(self, api_key: str, private_key: str):
               super().__init__(api_key, self.BASE_URL)
               self._private_key = serialization.load_pem_private_key(
                   private_key.encode() if isinstance(private_key, str) else private_key,
                   password=None
               )
               self._signature_cache: dict[str, tuple[str, float]] = {}

           def _auth_headers(self) -> dict:
               return {"Content-Type": "application/json"}

           def _generate_signature(self, timestamp: str, method: str, path: str) -> str:
               """Generate RSA-PSS signature with caching."""
               cache_key = f"{timestamp}:{method}:{path}"

               # Check cache (valid for 100ms)
               if cache_key in self._signature_cache:
                   sig, cached_time = self._signature_cache[cache_key]
                   if time.time() - cached_time < 0.1:
                       return sig

               msg = f"{timestamp}{method}{path}".encode()
               signature = self._private_key.sign(
                   msg,
                   padding.PSS(
                       mgf=padding.MGF1(hashes.SHA256()),
                       salt_length=padding.PSS.DIGEST_LENGTH
                   ),
                   hashes.SHA256()
               )
               sig = base64.b64encode(signature).decode()
               self._signature_cache[cache_key] = (sig, time.time())
               return sig

           async def _authenticated_request(self, method: str, endpoint: str, **kwargs) -> dict:
               """Make authenticated request to Kalshi."""
               timestamp = str(int(time.time() * 1000))
               path = f"/trade-api/v2{endpoint.split('?')[0]}"
               signature = self._generate_signature(timestamp, method.upper(), path)

               headers = {
                   "KALSHI-ACCESS-KEY": self.api_key,
                   "KALSHI-ACCESS-TIMESTAMP": timestamp,
                   "KALSHI-ACCESS-SIGNATURE": signature,
                   **kwargs.pop('headers', {})
               }

               return await self._request(method, endpoint, headers=headers, **kwargs)

           async def get_markets(self, series_ticker: str = None, status: str = "open") -> list[dict]:
               """Get markets with optional filtering."""
               params = {"status": status}
               if series_ticker:
                   params["series_ticker"] = series_ticker

               data = await self._authenticated_request("GET", "/markets", params=params)
               return data.get("markets", [])

           async def get_orderbook(self, ticker: str) -> OrderBook:
               """Get order book for a market."""
               data = await self._authenticated_request("GET", f"/markets/{ticker}/orderbook")
               return OrderBook.model_validate(data.get("orderbook", {}))

           async def stream_prices(self, tickers: list[str]) -> AsyncIterator[MarketPrice]:
               """Stream real-time prices via WebSocket."""
               import websockets

               async with websockets.connect(self.WS_URL) as ws:
                   # Subscribe to tickers
                   await ws.send(json.dumps({
                       "type": "subscribe",
                       "channels": ["orderbook"],
                       "tickers": tickers
                   }))

                   async for message in ws:
                       data = json.loads(message)
                       if data.get("type") == "orderbook":
                           yield self._parse_price_update(data)

       3.3 Polymarket Client (EU-Ready)

       Key improvement: Designed to run as standalone service in EU region.

       # markets/polymarket/client.py
       from ..base import BaseMarketClient

       class PolymarketClient(BaseMarketClient):
           """Polymarket CLOB client designed for EU deployment."""

           CLOB_URL = "https://clob.polymarket.com"
           GAMMA_URL = "https://gamma-api.polymarket.com"

           def __init__(self, api_key: str = None, region: str = "us"):
               super().__init__(api_key or "", self.CLOB_URL)
               self.gamma_url = self.GAMMA_URL
               self.region = region

           def _auth_headers(self) -> dict:
               headers = {"Content-Type": "application/json"}
               if self.api_key:
                   headers["Authorization"] = f"Bearer {self.api_key}"
               return headers

           async def get_markets(self, tag: str = None, active: bool = True) -> list[dict]:
               """Get markets from Gamma API."""
               params = {"active": str(active).lower()}
               if tag:
                   params["tag"] = tag

               # Use Gamma API for market discovery
               async with self.session() as session:
                   async with session.get(f"{self.gamma_url}/markets", params=params) as resp:
                       resp.raise_for_status()
                       return await resp.json()

           async def get_orderbook(self, token_id: str) -> OrderBook:
               """Get CLOB order book for a token."""
               data = await self._request("GET", "/book", params={"token_id": token_id})
               return OrderBook.model_validate(data)

           def resolve_token_id(self, market: dict) -> Optional[str]:
               """Resolve YES token ID from market data."""
               # Try multiple field locations
               for source in [
                   market.get("tokens", []),
                   [{"outcome": o, "token_id": t}
                    for o, t in zip(market.get("outcomes", []), market.get("token_ids", []))],
               ]:
                   for token in source:
                       if str(token.get("outcome", "")).lower() == "yes":
                           return token.get("token_id") or token.get("id")

               # Try clobTokenIds field
               clob_ids = market.get("clobTokenIds")
               if clob_ids:
                   if isinstance(clob_ids, str):
                       clob_ids = json.loads(clob_ids)
                   if clob_ids and len(clob_ids) > 0:
                       return clob_ids[0]

               return None

       3.4 Verification Steps

       # Test Kalshi authentication
       python -m markets.kalshi.test_auth

       # Test Polymarket market fetch
       python -m markets.polymarket.test_fetch

       # Integration test
       pytest tests/integration/test_market_clients.py

       ---
       Phase 4: Data Providers and Game Engine (Week 4-6)

       4.1 Data Provider Interface

       Improvement: Unified interface for all sports data providers.

       # data_providers/base.py
       from abc import ABC, abstractmethod
       from typing import AsyncIterator, Optional
       from arbees_shared.models import GameInfo, GameState, Play, Sport

       class DataProvider(ABC):
           """Base interface for sports data providers."""

           @property
           @abstractmethod
           def supported_sports(self) -> list[Sport]:
               """List of sports this provider supports."""
               pass

           @abstractmethod
           async def get_live_games(self, sport: Sport) -> list[GameInfo]:
               """Get all live games for a sport."""
               pass

           @abstractmethod
           async def get_game_state(self, game_id: str) -> Optional[GameState]:
               """Get current state of a specific game."""
               pass

           @abstractmethod
           async def stream_plays(self, game_id: str) -> AsyncIterator[Play]:
               """Stream plays as they happen."""
               pass

       # data_providers/espn/client.py
       class ESPNDataProvider(DataProvider):
           """ESPN API data provider."""

           SPORT_URLS = {
               Sport.NFL: "https://site.api.espn.com/apis/site/v2/sports/football/nfl",
               Sport.NBA: "https://site.api.espn.com/apis/site/v2/sports/basketball/nba",
               Sport.NHL: "https://site.api.espn.com/apis/site/v2/sports/hockey/nhl",
               Sport.MLB: "https://site.api.espn.com/apis/site/v2/sports/baseball/mlb",
               # ... more sports
           }

           @property
           def supported_sports(self) -> list[Sport]:
               return list(self.SPORT_URLS.keys())

           async def get_live_games(self, sport: Sport) -> list[GameInfo]:
               url = f"{self.SPORT_URLS[sport]}/scoreboard"
               async with self._session.get(url) as resp:
                   data = await resp.json()
                   return [self._parse_game_info(e, sport) for e in data.get("events", [])]

       4.2 GameShard Service (Key Architectural Change)

       This is the biggest improvement: Instead of spawning a container per game (expensive, slow), a single GameShard process handles 10-20 games concurrently using
       asyncio.

       # services/game_shard/shard.py
       import asyncio
       from typing import Dict, Set
       from dataclasses import dataclass, field
       from datetime import datetime, timezone

       from arbees_shared.models import GameState, Play, Signal, Sport
       from arbees_shared.messaging import MessageBus, Channels
       from arbees_shared.db import Database
       from data_providers.base import DataProvider
       from markets.base import BaseMarketClient
       import arbees_core  # Rust bindings

       @dataclass
       class GameContext:
           """Context for a monitored game."""
           game_id: str
           sport: Sport
           home_team: str
           away_team: str
           state: GameState
           market_prices: dict = field(default_factory=dict)
           last_play_id: str = ""
           started_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))

       class GameShard:
           """
           Handles multiple games concurrently using asyncio.

           Improvements over container-per-game:
           - 10x lower memory footprint (one Python process vs many containers)
           - Instant game start (no container spawn latency)
           - Shared connections to DB, Redis, markets
           - Better resource utilization
           """

           MAX_GAMES = 20  # Configurable via env
           POLL_INTERVAL = 3.0  # Seconds between game state polls

           def __init__(
               self,
               shard_id: str,
               data_provider: DataProvider,
               market_clients: dict[str, BaseMarketClient],
               db: Database,
               message_bus: MessageBus,
           ):
               self.shard_id = shard_id
               self.data_provider = data_provider
               self.market_clients = market_clients
               self.db = db
               self.bus = message_bus

               self._games: Dict[str, GameContext] = {}
               self._tasks: Dict[str, asyncio.Task] = {}
               self._shutdown = asyncio.Event()

           @property
           def game_count(self) -> int:
               return len(self._games)

           @property
           def has_capacity(self) -> bool:
               return self.game_count < self.MAX_GAMES

           async def add_game(self, game_id: str, sport: Sport) -> bool:
               """Add a game to monitor. Returns False if at capacity."""
               if not self.has_capacity or game_id in self._games:
                   return False

               # Fetch initial state
               state = await self.data_provider.get_game_state(game_id)
               if not state:
                   return False

               context = GameContext(
                   game_id=game_id,
                   sport=sport,
                   home_team=state.home_team,
                   away_team=state.away_team,
                   state=state,
               )

               self._games[game_id] = context
               self._tasks[game_id] = asyncio.create_task(
                   self._monitor_game(context),
                   name=f"monitor_{game_id}"
               )

               return True

           async def remove_game(self, game_id: str) -> None:
               """Stop monitoring a game."""
               if game_id in self._tasks:
                   self._tasks[game_id].cancel()
                   del self._tasks[game_id]

               if game_id in self._games:
                   del self._games[game_id]

           async def _monitor_game(self, ctx: GameContext) -> None:
               """Monitor a single game - runs as asyncio task."""
               try:
                   # Start parallel monitoring tasks
                   await asyncio.gather(
                       self._poll_game_state(ctx),
                       self._poll_market_prices(ctx),
                       return_exceptions=True
                   )
               except asyncio.CancelledError:
                   pass
               except Exception as e:
                   logger.error(f"Error monitoring game {ctx.game_id}: {e}")

           async def _poll_game_state(self, ctx: GameContext) -> None:
               """Poll ESPN for game state updates."""
               while not self._shutdown.is_set():
                   try:
                       new_state = await self.data_provider.get_game_state(ctx.game_id)
                       if not new_state:
                           await asyncio.sleep(self.POLL_INTERVAL)
                           continue

                       # Check for new play
                       if new_state.last_play and new_state.last_play.play_id != ctx.last_play_id:
                           play = new_state.last_play
                           ctx.last_play_id = play.play_id

                           # Analyze play impact using Rust
                           old_prob = arbees_core.calculate_win_probability(
                               ctx.state.model_dump(), ctx.home_team
                           )
                           new_prob = arbees_core.calculate_win_probability(
                               new_state.model_dump(), ctx.home_team
                           )

                           # Generate signals if significant change
                           if abs(new_prob - old_prob) > 0.02:  # 2% change threshold
                               await self._generate_signals(ctx, old_prob, new_prob, play)

                           # Publish play event
                           await self.bus.publish(
                               Channels.PLAY_DETECTED.format(game_id=ctx.game_id),
                               play
                           )

                       # Update state
                       ctx.state = new_state
                       await self.db.write_game_state(new_state)

                       # Check if game ended
                       if new_state.game_status == "final":
                           await self.remove_game(ctx.game_id)
                           break

                       await asyncio.sleep(self.POLL_INTERVAL)

                   except Exception as e:
                       logger.error(f"Error polling game {ctx.game_id}: {e}")
                       await asyncio.sleep(self.POLL_INTERVAL)

           async def _poll_market_prices(self, ctx: GameContext) -> None:
               """Poll market prices for the game."""
               while not self._shutdown.is_set():
                   try:
                       for platform, client in self.market_clients.items():
                           prices = await client.get_prices_for_game(
                               ctx.game_id, ctx.home_team, ctx.away_team
                           )
                           ctx.market_prices[platform] = prices

                           # Write to DB
                           for price in prices:
                               await self.db.write_market_price(ctx.game_id, price)

                       await asyncio.sleep(self.POLL_INTERVAL)

                   except Exception as e:
                       logger.error(f"Error polling prices for {ctx.game_id}: {e}")
                       await asyncio.sleep(self.POLL_INTERVAL)

           async def _generate_signals(
               self,
               ctx: GameContext,
               old_prob: float,
               new_prob: float,
               play: Play
           ) -> None:
               """Generate trading signals using Rust core."""
               for platform, prices in ctx.market_prices.items():
                   if not prices:
                       continue

                   # Use Rust for fast arbitrage detection
                   opportunities = arbees_core.find_opportunities(
                       model_prob=new_prob,
                       market_prices=prices,
                       min_edge=0.03
                   )

                   for opp in opportunities:
                       signal = Signal(
                           game_id=ctx.game_id,
                           sport=ctx.sport,
                           platform=platform,
                           signal_type=opp.opportunity_type,
                           confidence=opp.confidence,
                           edge_pct=opp.edge_pct,
                           model_probability=new_prob,
                           market_price=opp.market_price,
                           trigger_play=play,
                       )

                       await self.bus.publish(Channels.SIGNAL_GENERATED, signal)
                       await self.db.write_signal(signal)

           async def run(self) -> None:
               """Main run loop."""
               logger.info(f"GameShard {self.shard_id} starting...")

               try:
                   while not self._shutdown.is_set():
                       # Health check - remove dead tasks
                       for game_id, task in list(self._tasks.items()):
                           if task.done():
                               del self._tasks[game_id]
                               if game_id in self._games:
                                   del self._games[game_id]

                       await asyncio.sleep(10)
               finally:
                   # Cleanup
                   for task in self._tasks.values():
                       task.cancel()
                   await asyncio.gather(*self._tasks.values(), return_exceptions=True)

           async def shutdown(self) -> None:
               """Graceful shutdown."""
               self._shutdown.set()

       4.3 Orchestrator Service

       Improvement: Manages GameShards instead of Docker containers.

       # services/orchestrator/orchestrator.py
       class Orchestrator:
           """
           Manages GameShard processes and distributes games across them.

           Improvements over old container-based orchestrator:
           - Sub-second game assignment (vs 5-10s container spawn)
           - Better load balancing across shards
           - Health monitoring and auto-recovery
           """

           def __init__(
               self,
               shards: list[GameShard],
               data_provider: DataProvider,
               db: Database,
           ):
               self.shards = shards
               self.data_provider = data_provider
               self.db = db
               self._game_to_shard: dict[str, GameShard] = {}

           async def discover_and_assign_games(self) -> None:
               """Discover live games and assign to shards."""
               for sport in self.data_provider.supported_sports:
                   games = await self.data_provider.get_live_games(sport)

                   for game in games:
                       if game.game_id in self._game_to_shard:
                           continue  # Already assigned

                       if game.status != "in_progress":
                           continue  # Not live yet

                       # Find shard with capacity
                       shard = self._find_available_shard()
                       if not shard:
                           logger.warning(f"No shard capacity for game {game.game_id}")
                           continue

                       # Assign game to shard
                       success = await shard.add_game(game.game_id, sport)
                       if success:
                           self._game_to_shard[game.game_id] = shard
                           logger.info(f"Assigned {game.game_id} to shard {shard.shard_id}")

           def _find_available_shard(self) -> Optional[GameShard]:
               """Find shard with lowest load that has capacity."""
               available = [s for s in self.shards if s.has_capacity]
               if not available:
                   return None
               return min(available, key=lambda s: s.game_count)

       4.4 Verification Steps

       # Test data provider
       pytest tests/integration/test_espn_provider.py

       # Test GameShard with mock data
       pytest tests/unit/test_game_shard.py

       # Load test - simulate 20 concurrent games
       python benchmarks/shard_load_test.py --games 20

       # Integration test
       docker-compose up -d
       python scripts/test_full_pipeline.py

       ---
       Phase 5: API and Frontend (Week 6-7)

       5.1 FastAPI Backend (Improved)

       Improvements:
       - Proper dependency injection with FastAPI's Depends
       - Background tasks for non-blocking operations
       - WebSocket connection management
       - OpenTelemetry instrumentation

       # services/api/main.py
       from fastapi import FastAPI, Depends, WebSocket, WebSocketDisconnect
       from fastapi.middleware.cors import CORSMiddleware
       from contextlib import asynccontextmanager
       from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor

       from arbees_shared.db import Database, get_db
       from arbees_shared.messaging import MessageBus, get_bus
       from .dependencies import get_current_user
       from .routers import opportunities, games, trades, monitoring

       @asynccontextmanager
       async def lifespan(app: FastAPI):
           """Manage application lifecycle."""
           # Startup
           app.state.db = await Database.connect()
           app.state.bus = await MessageBus.connect()

           yield

           # Shutdown
           await app.state.db.close()
           await app.state.bus.close()

       app = FastAPI(
           title="Arbees API",
           version="2.0.0",
           lifespan=lifespan,
       )

       # Middleware
       app.add_middleware(
           CORSMiddleware,
           allow_origins=["*"],
           allow_credentials=True,
           allow_methods=["*"],
           allow_headers=["*"],
       )

       # Instrumentation
       FastAPIInstrumentor.instrument_app(app)

       # Routers
       app.include_router(opportunities.router, prefix="/api/opportunities", tags=["opportunities"])
       app.include_router(games.router, prefix="/api/games", tags=["games"])
       app.include_router(trades.router, prefix="/api/trades", tags=["trades"])
       app.include_router(monitoring.router, prefix="/api/monitoring", tags=["monitoring"])

       # WebSocket connection manager
       class ConnectionManager:
           def __init__(self):
               self.active: dict[str, WebSocket] = {}

           async def connect(self, websocket: WebSocket, client_id: str):
               await websocket.accept()
               self.active[client_id] = websocket

           def disconnect(self, client_id: str):
               self.active.pop(client_id, None)

           async def broadcast(self, message: dict):
               for ws in self.active.values():
                   await ws.send_json(message)

       manager = ConnectionManager()

       @app.websocket("/ws")
       async def websocket_endpoint(websocket: WebSocket):
           client_id = str(uuid.uuid4())
           await manager.connect(websocket, client_id)

           try:
               # Subscribe to Redis channels and forward to WebSocket
               async def forward_messages():
                   bus = app.state.bus
                   async for msg in bus.subscribe_all():
                       await websocket.send_json(msg)

               await forward_messages()
           except WebSocketDisconnect:
               manager.disconnect(client_id)

       5.2 Frontend Improvements

       The existing React frontend is solid. Key improvements:

       1. React Query v5 - Already using, but upgrade patterns
       2. Zustand - Replace Redux for simpler state management
       3. TanStack Table - Already using for data tables
       4. Recharts - Continue using for charts

       // frontend/src/hooks/useGameStream.ts
       import { useEffect } from 'react';
       import { useQueryClient } from '@tanstack/react-query';

       export function useGameStream(gameId: string) {
         const queryClient = useQueryClient();

         useEffect(() => {
           const ws = new WebSocket(`${WS_URL}/ws`);

           ws.onmessage = (event) => {
             const data = JSON.parse(event.data);

             switch (data.type) {
               case 'game_state':
                 queryClient.setQueryData(['game', gameId, 'state'], data.payload);
                 break;
               case 'play':
                 queryClient.setQueryData(['game', gameId, 'plays'], (old: Play[]) =>
                   [data.payload, ...(old || [])].slice(0, 50)
                 );
                 break;
               case 'signal':
                 queryClient.setQueryData(['game', gameId, 'signals'], (old: Signal[]) =>
                   [data.payload, ...(old || [])].slice(0, 20)
                 );
                 break;
             }
           };

           return () => ws.close();
         }, [gameId, queryClient]);
       }

       ---
       Phase 6: Infrastructure and Deployment (Week 7-8)

       6.1 Terraform Configuration

       # infrastructure/terraform/main.tf
       terraform {
         required_providers {
           aws = {
             source  = "hashicorp/aws"
             version = "~> 5.0"
           }
         }
       }

       # US East - Primary region (Kalshi, core services)
       provider "aws" {
         alias  = "us_east"
         region = "us-east-1"
       }

       # EU Central - Polymarket proxy
       provider "aws" {
         alias  = "eu_central"
         region = "eu-central-1"
       }

       # VPC for US region
       module "vpc_us" {
         source = "./modules/vpc"
         providers = { aws = aws.us_east }

         name       = "arbees-us"
         cidr_block = "10.0.0.0/16"
       }

       # VPC for EU region
       module "vpc_eu" {
         source = "./modules/vpc"
         providers = { aws = aws.eu_central }

         name       = "arbees-eu"
         cidr_block = "10.1.0.0/16"
       }

       # VPC Peering
       resource "aws_vpc_peering_connection" "us_eu" {
         provider    = aws.us_east
         vpc_id      = module.vpc_us.vpc_id
         peer_vpc_id = module.vpc_eu.vpc_id
         peer_region = "eu-central-1"

         auto_accept = false
       }

       # TimescaleDB on RDS
       module "timescaledb" {
         source = "./modules/rds"
         providers = { aws = aws.us_east }

         identifier     = "arbees-tsdb"
         engine         = "postgres"
         engine_version = "15"
         instance_class = "db.t3.medium"

         # TimescaleDB extension
         parameter_group_family = "postgres15"
         parameters = {
           "shared_preload_libraries" = "timescaledb"
         }
       }

       # Redis for messaging
       module "redis" {
         source = "./modules/elasticache"
         providers = { aws = aws.us_east }

         cluster_id      = "arbees-redis"
         node_type       = "cache.t3.small"
         num_cache_nodes = 2
       }

       # ECS Cluster - US
       module "ecs_us" {
         source = "./modules/ecs"
         providers = { aws = aws.us_east }

         cluster_name = "arbees-us"

         services = {
           orchestrator = {
             image  = "${aws_ecr_repository.arbees.repository_url}:orchestrator"
             cpu    = 512
             memory = 1024
             count  = 1
           }
           game_shard = {
             image  = "${aws_ecr_repository.arbees.repository_url}:game-shard"
             cpu    = 1024
             memory = 2048
             count  = 3  # 3 shards x 20 games = 60 concurrent games
           }
           api = {
             image  = "${aws_ecr_repository.arbees.repository_url}:api"
             cpu    = 256
             memory = 512
             count  = 2
           }
         }
       }

       # ECS Cluster - EU (Polymarket only)
       module "ecs_eu" {
         source = "./modules/ecs"
         providers = { aws = aws.eu_central }

         cluster_name = "arbees-eu"

         services = {
           polymarket_proxy = {
             image  = "${aws_ecr_repository.arbees.repository_url}:polymarket-proxy"
             cpu    = 256
             memory = 512
             count  = 1
           }
         }
       }

       6.2 Docker Compose for Local Development

       # docker-compose.yml
       version: '3.8'

       services:
         timescaledb:
           image: timescale/timescaledb:latest-pg15
           environment:
             POSTGRES_PASSWORD: postgres
             POSTGRES_DB: arbees
           ports:
             - "5432:5432"
           volumes:
             - timescaledb_data:/var/lib/postgresql/data
             - ./shared/arbees_shared/db/migrations:/docker-entrypoint-initdb.d

         redis:
           image: redis:7-alpine
           ports:
             - "6379:6379"

         api:
           build:
             context: .
             dockerfile: services/api/Dockerfile
           environment:
             DATABASE_URL: postgresql://postgres:postgres@timescaledb:5432/arbees
             REDIS_URL: redis://redis:6379
           ports:
             - "8000:8000"
           depends_on:
             - timescaledb
             - redis

         game_shard:
           build:
             context: .
             dockerfile: services/game_shard/Dockerfile
           environment:
             DATABASE_URL: postgresql://postgres:postgres@timescaledb:5432/arbees
             REDIS_URL: redis://redis:6379
             SHARD_ID: local-1
           depends_on:
             - timescaledb
             - redis

         frontend:
           build:
             context: ./frontend
           ports:
             - "3000:3000"
           depends_on:
             - api

       volumes:
         timescaledb_data:

       ---
       Testing Strategy

       Unit Tests

       # tests/unit/test_rust_core.py
       import pytest
       import arbees_core

       def test_win_probability_basic():
           """Test basic win probability calculation."""
           state = {
               "sport": "nfl",
               "home_score": 21,
               "away_score": 17,
               "period": 3,
               "time_remaining_secs": 600,
               "possession": "home",
           }

           prob = arbees_core.calculate_win_probability(state, "home")
           assert 0.5 < prob < 0.8  # Leading team should have advantage

       def test_arbitrage_detection():
           """Test cross-market arbitrage detection."""
           opportunities = arbees_core.find_cross_market_arbitrage({
               "events": [
                   {"platform": "kalshi", "raw_data": {"yes_bid": 0.55, "yes_ask": 0.57}},
                   {"platform": "polymarket", "raw_data": {"yes_bid": 0.60, "yes_ask": 0.62}},
               ]
           })

           assert len(opportunities) == 1
           assert opportunities[0]["edge_pct"] > 0

       Integration Tests

       # tests/integration/test_game_shard.py
       import pytest
       import asyncio
       from services.game_shard import GameShard
       from tests.mocks import MockDataProvider, MockMarketClient

       @pytest.fixture
       async def shard():
           shard = GameShard(
               shard_id="test",
               data_provider=MockDataProvider(),
               market_clients={"mock": MockMarketClient()},
               db=MockDatabase(),
               message_bus=MockMessageBus(),
           )
           return shard

       async def test_add_game(shard):
           """Test adding a game to monitor."""
           success = await shard.add_game("game123", Sport.NFL)
           assert success
           assert shard.game_count == 1

       async def test_max_capacity(shard):
           """Test shard respects max capacity."""
           for i in range(20):
               await shard.add_game(f"game{i}", Sport.NFL)

           assert shard.game_count == 20
           assert not shard.has_capacity

           # Should reject new game
           success = await shard.add_game("game_overflow", Sport.NFL)
           assert not success

       End-to-End Tests

       # tests/e2e/test_full_pipeline.py
       async def test_live_game_to_signal():
           """Test complete pipeline from live game to trading signal."""
           # 1. Start services
           orchestrator = await start_orchestrator()

           # 2. Inject mock live game
           mock_game = create_mock_game(sport=Sport.NFL, status="in_progress")
           await inject_game(mock_game)

           # 3. Wait for orchestrator to assign game
           await asyncio.sleep(5)
           assert mock_game.game_id in orchestrator.active_games

           # 4. Inject scoring play
           scoring_play = create_mock_play(type=PlayType.TOUCHDOWN, points=7)
           await inject_play(mock_game.game_id, scoring_play)

           # 5. Wait for signal
           signal = await wait_for_signal(game_id=mock_game.game_id, timeout=10)
           assert signal is not None
           assert signal.edge_pct > 0

       ---
       Migration Checklist

       Phase 1 (Week 1-2)

       - Create Arbees project structure
       - Port rust_core with improved types
       - Add win probability to Rust
       - Set up maturin build
       - Verify Rust module imports

       Phase 2 (Week 2-3)

       - Create Pydantic v2 models
       - Write TimescaleDB migrations
       - Implement Redis messaging
       - Create shared package
       - Test database and messaging

       Phase 3 (Week 3-4)

       - Implement base market client
       - Port Kalshi client with improvements
       - Port Polymarket client
       - Create paper trading engine
       - Test market connections

       Phase 4 (Week 4-6)

       - Create data provider interface
       - Port ESPN client
       - Implement GameShard service
       - Implement Orchestrator
       - Load test with 20+ games

       Phase 5 (Week 6-7)

       - Set up FastAPI backend
       - Implement WebSocket streaming
       - Port React frontend
       - Add real-time updates
       - Test UI functionality

       Phase 6 (Week 7-8)

       - Create Terraform configs
       - Set up CI/CD pipeline
       - Deploy to staging
       - Run integration tests
       - Deploy to production

       ---
       Critical Files for Implementation

       1. P:\petes_code\ClaudeCode\IDBGfSportsBetS\rust\arb_core\src\lib.rs - Existing Rust core to port and expand; provides the pattern for PyO3 bindings
       2. P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\models.py - Core Pydantic models (GameState, Play, Signal) to migrate to Pydantic v2 with improvements
       3. P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\engine.py - LiveGameEngine pattern to refactor into GameShard architecture
       4. P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\kalshi\kalshi_client.py - RSA authentication pattern to improve with connection pooling and WebSocket support
       5. P:\petes_code\ClaudeCode\IDBGfSportsBetS\webapp\backend\main.py - FastAPI patterns to modernize with proper dependency injection and WebSocket management
  ⎿  Done (27 tool uses · 106.1k tokens · 5m 52s)

∴ Thinking…

  I have comprehensive exploration results from Phase 1 and a detailed implementation plan from Phase 2. Now I need to:

  1. Review the plan and read critical files to deepen understanding
  2. Ensure alignment with user's request
  3. Write my final plan to the plan file
  4. Call ExitPlanMode

  Let me read a few critical files to validate the plan, then write the final plan file.

● I have comprehensive exploration and planning results. Let me read a few critical files to validate the plan, then write the final plan.

● Read(P:\petes_code\ClaudeCode\IDBGfSportsBetS\rust\arb_core\src\lib.rs)
  ⎿  Read 367 lines

● Read(P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\models.py)
  ⎿  Read 588 lines

∴ Thinking…

  I now have a comprehensive understanding of the codebase and the plan. Let me write the final plan to the plan file, then call ExitPlanMode.

  The plan should be:
  1. Concise enough to scan quickly
  2. Detailed enough to execute effectively
  3. Include paths to critical files
  4. Include verification steps

  Let me write this up now.

● I now have a complete picture. Let me write the final plan to the plan file.

● Updated plan
  ⎿  Wrote 1 lines to C:\Users\petec\.claude\plans\delegated-humming-aho.md
     # Arbees Migration Plan

     Migrate IDBGfSportsBetS to a modernized Arbees architecture with key improvements:
     - **GameShard** architecture (asyncio multi-game) instead of container-per-game
     - **TimescaleDB** instead of InfluxDB for relational + time-series data
     - **Enhanced Rust core** with win probability models
     - **Proper async** Python with Pydantic v2
     - **AWS multi-region** deployment (US for Kalshi, EU for Polymarket)

     ---

     ## Phase 1: Project Scaffold & Rust Core

     ### 1.1 Create Project Structure
     ```
     Arbees/
     ├── pyproject.toml              # UV/Poetry workspace
     ├── Cargo.toml                  # Rust workspace
     ├── docker-compose.yml          # Local dev
     ├── rust_core/                  # PyO3 bindings via maturin
     │   ├── Cargo.toml
     │   ├── pyproject.toml
     │   └── src/
     │       ├── lib.rs              # Module entry + arb detection
     │       ├── win_prob.rs         # Win probability (NEW - port from Python)
     │       └── types.rs            # Native Rust types with #[pyclass]
     ├── shared/arbees_shared/       # Installable Python package
     ├── data_providers/             # ESPN, Goalserve
     ├── markets/                    # Kalshi, Polymarket, Paper
     ├── services/                   # orchestrator, game_shard, position_manager, api
     ├── frontend/                   # React dashboard
     ├── infrastructure/             # Terraform
     └── tests/
     ```

     ### 1.2 Port & Improve Rust Core
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\rust\arb_core\src\lib.rs`

     **Improvements:**
     - Replace JSON serialization with native `#[pyclass]` structs (faster)
     - Add win probability calculation in Rust (10-100x faster than Python)
     - Add SIMD batch processing for multiple games

     **Key functions to implement:**
     - `calculate_win_probability(game_state, team) -> f64`
     - `find_cross_market_arbitrage(events) -> Vec<Opportunity>`
     - `find_model_edges(events, model_prob) -> Vec<Opportunity>`
     - `batch_calculate_win_probs(game_states) -> Vec<f64>` (SIMD)

     ### Verification
     ```bash
     cd rust_core && maturin develop --release
     python -c "import arbees_core; print(arbees_core.calculate_win_probability({...}, 'KC'))"
     cargo test
     ```

     ---

     ## Phase 2: Shared Library & Database

     ### 2.1 Pydantic v2 Models
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\models.py`

     **Files to create:**
     - `shared/arbees_shared/models/game.py` - GameState, GameInfo, Play
     - `shared/arbees_shared/models/market.py` - MarketPrice, OrderBook
     - `shared/arbees_shared/models/signal.py` - TradingSignal, ImpactAnalysis
     - `shared/arbees_shared/models/trade.py` - PaperTrade, Position

     **Improvements:**
     - Use `ConfigDict(frozen=True)` for immutability/caching
     - Use `computed_field` for derived properties
     - Use discriminated unions for polymorphic types

     ### 2.2 TimescaleDB Schema
     **File:** `shared/arbees_shared/db/migrations/001_initial.sql`

     **Tables:**
     - `games` - Game metadata (relational)
     - `game_states` - Time-series hypertable
     - `plays` - Time-series hypertable
     - `market_prices` - Time-series hypertable
     - `arbitrage_opportunities` - Time-series hypertable
     - `paper_trades` - Trade records with PnL

     ### 2.3 Redis Messaging
     **File:** `shared/arbees_shared/messaging/redis_bus.py`

     Type-safe pub/sub with msgpack serialization:
     - `game:{game_id}:state` - Game state updates
     - `game:{game_id}:play` - New plays detected
     - `signals:new` - Trading signals
     - `trades:executed` - Paper trade confirmations

     ### Verification
     ```bash
     docker-compose up timescaledb redis -d
     alembic upgrade head
     pytest tests/integration/test_database.py
     pytest tests/integration/test_messaging.py
     ```

     ---

     ## Phase 3: Market Clients

     ### 3.1 Async Base Client
     **File:** `markets/base.py`

     Features:
     - Connection pooling with aiohttp
     - Rate limiting
     - Retry with tenacity
     - Abstract methods: `get_markets()`, `get_orderbook()`, `stream_prices()`

     ### 3.2 Kalshi Client
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\kalshi\kalshi_client.py`
     **Target:** `markets/kalshi/client.py`

     **Improvements:**
     - RSA signature caching (reduce crypto overhead)
     - WebSocket support for real-time prices
     - Proper async/await throughout

     ### 3.3 Polymarket Client
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\polymarket\polymarket_client.py`
     **Target:** `markets/polymarket/client.py`

     **Improvements:**
     - Designed as standalone microservice for EU deployment
     - No VPN/RPi hacks - proper cloud deployment
     - Robust token ID resolution

     ### 3.4 Paper Trading Engine
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\paper_trader.py`
     **Target:** `markets/paper/engine.py`

     Features:
     - Kelly criterion position sizing
     - Execution tracking with slippage modeling
     - PnL calculation and reporting

     ### Verification
     ```bash
     python -m markets.kalshi.test_auth
     python -m markets.polymarket.test_fetch
     pytest tests/integration/test_market_clients.py
     ```

     ---

     ## Phase 4: Data Providers & Game Engine

     ### 4.1 ESPN Data Provider
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\espn_*.py` (6 sport clients)
     **Target:** `data_providers/espn/`

     **Files:**
     - `base.py` - DataProvider interface
     - `client.py` - ESPN API client
     - `parsers/nfl.py`, `parsers/nba.py`, etc. - Sport-specific parsing

     ### 4.2 GameShard Service (KEY IMPROVEMENT)
     **Target:** `services/game_shard/shard.py`

     **Architecture:**
     - Single process handles 10-20 games via asyncio
     - Shared connections to DB, Redis, markets
     - Dynamic poll intervals (1s crunch time, 30s halftime)
     - Uses Rust core for win probability

     **Methods:**
     - `add_game(game_id, sport)` - Start monitoring
     - `remove_game(game_id)` - Stop monitoring
     - `_poll_game_state(ctx)` - ESPN polling loop
     - `_poll_market_prices(ctx)` - Market polling loop
     - `_generate_signals(ctx, old_prob, new_prob, play)` - Signal generation

     **Benefits over container-per-game:**
     - 10x lower memory footprint
     - Instant game start (no container spawn)
     - Better resource utilization

     ### 4.3 Orchestrator Service
     **Target:** `services/orchestrator/orchestrator.py`

     **Responsibilities:**
     - Discover live games across all sports
     - Assign games to shards with load balancing
     - Monitor shard health
     - Handle shard failures with redistribution

     ### Verification
     ```bash
     pytest tests/integration/test_espn_provider.py
     pytest tests/unit/test_game_shard.py
     python benchmarks/shard_load_test.py --games 20
     ```

     ---

     ## Phase 5: API & Frontend

     ### 5.1 FastAPI Backend
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\webapp\backend\main.py`
     **Target:** `services/api/`

     **Improvements:**
     - Proper dependency injection with `Depends`
     - WebSocket connection management
     - OpenTelemetry instrumentation
     - Structured routers: opportunities, games, trades, monitoring

     ### 5.2 React Frontend
     **Source:** `P:\petes_code\ClaudeCode\IDBGfSportsBetS\webapp\frontend/`
     **Target:** `frontend/`

     Port existing components (50+) with improvements:
     - React Query v5 patterns
     - Real-time WebSocket updates
     - Zustand for simpler state management

     ### Verification
     ```bash
     cd services/api && uvicorn main:app --reload
     cd frontend && npm run dev
     # Test WebSocket connections
     ```

     ---

     ## Phase 6: Infrastructure & Deployment

     ### 6.1 Terraform
     **Target:** `infrastructure/terraform/`

     **US Region (us-east-1):**
     - ECS cluster: orchestrator, game_shard (x3), api (x2)
     - RDS: TimescaleDB
     - ElastiCache: Redis
     - ALB: Load balancer

     **EU Region (eu-central-1):**
     - ECS cluster: polymarket_proxy
     - VPC peering to US

     ### 6.2 Docker Compose
     - `docker-compose.yml` - Local development
     - `docker-compose.prod.yml` - Production config

     ### Verification
     ```bash
     terraform plan
     docker-compose up -d
     python scripts/test_full_pipeline.py
     ```

     ---

     ## Critical Files Reference

     | Source | Target | Purpose |
     |--------|--------|---------|
     | `IDBGfSportsBetS/rust/arb_core/src/lib.rs` | `rust_core/src/lib.rs` | Arb detection + win prob |
     | `IDBGfSportsBetS/agents/live_game/models.py` | `shared/arbees_shared/models/` | Pydantic models |
     | `IDBGfSportsBetS/agents/kalshi/kalshi_client.py` | `markets/kalshi/client.py` | Kalshi API |
     | `IDBGfSportsBetS/agents/polymarket/polymarket_client.py` | `markets/polymarket/client.py` | Polymarket API |
     | `IDBGfSportsBetS/agents/live_game/engine.py` | `services/game_shard/shard.py` | Game monitoring |
     | `IDBGfSportsBetS/agents/arbitrage/fee_calculator.py` | `shared/arbees_shared/utils/fees.py` | Fee calculations |
     | `IDBGfSportsBetS/webapp/backend/main.py` | `services/api/main.py` | FastAPI backend |

     ---

     ## Execution Order

     1. **Phase 1** - Scaffold + Rust (foundation)
     2. **Phase 2** - Shared lib + DB (data layer)
     3. **Phase 3** - Market clients (external integrations)
     4. **Phase 4** - Game engine (core business logic)
     5. **Phase 5** - API + Frontend (user interface)
     6. **Phase 6** - Infrastructure (deployment)

     Each phase builds on the previous. Run verification steps before proceeding.


∴ Thinking…

  I've written the final plan. Now I should call ExitPlanMode to request user approval. The plan is comprehensive and covers:

  1. Project scaffold and Rust core improvements
  2. Shared library with Pydantic v2 and TimescaleDB
  3. Async market clients (Kalshi, Polymarket)
  4. GameShard architecture (the key improvement)
  5. API and frontend
  6. Infrastructure with Terraform

  The plan includes:
  - Specific files to create
  - Source files from the old project
  - Verification steps for each phase
  - Critical files reference table

  Let me call ExitPlanMode with the necessary permissions.

───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 Ready to code?

 Here is Claude's plan:
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 Arbees Migration Plan

 Migrate IDBGfSportsBetS to a modernized Arbees architecture with key improvements:
 - GameShard architecture (asyncio multi-game) instead of container-per-game
 - TimescaleDB instead of InfluxDB for relational + time-series data
 - Enhanced Rust core with win probability models
 - Proper async Python with Pydantic v2
 - AWS multi-region deployment (US for Kalshi, EU for Polymarket)

 ---
 Phase 1: Project Scaffold & Rust Core

 1.1 Create Project Structure

 Arbees/
 ├── pyproject.toml              # UV/Poetry workspace
 ├── Cargo.toml                  # Rust workspace
 ├── docker-compose.yml          # Local dev
 ├── rust_core/                  # PyO3 bindings via maturin
 │   ├── Cargo.toml
 │   ├── pyproject.toml
 │   └── src/
 │       ├── lib.rs              # Module entry + arb detection
 │       ├── win_prob.rs         # Win probability (NEW - port from Python)
 │       └── types.rs            # Native Rust types with #[pyclass]
 ├── shared/arbees_shared/       # Installable Python package
 ├── data_providers/             # ESPN, Goalserve
 ├── markets/                    # Kalshi, Polymarket, Paper
 ├── services/                   # orchestrator, game_shard, position_manager, api
 ├── frontend/                   # React dashboard
 ├── infrastructure/             # Terraform
 └── tests/

 1.2 Port & Improve Rust Core

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\rust\arb_core\src\lib.rs

 Improvements:
 - Replace JSON serialization with native #[pyclass] structs (faster)
 - Add win probability calculation in Rust (10-100x faster than Python)
 - Add SIMD batch processing for multiple games

 Key functions to implement:
 - calculate_win_probability(game_state, team) -> f64
 - find_cross_market_arbitrage(events) -> Vec<Opportunity>
 - find_model_edges(events, model_prob) -> Vec<Opportunity>
 - batch_calculate_win_probs(game_states) -> Vec<f64> (SIMD)

 Verification

 cd rust_core && maturin develop --release
 python -c "import arbees_core; print(arbees_core.calculate_win_probability({...}, 'KC'))"
 cargo test

 ---
 Phase 2: Shared Library & Database

 2.1 Pydantic v2 Models

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\models.py

 Files to create:
 - shared/arbees_shared/models/game.py - GameState, GameInfo, Play
 - shared/arbees_shared/models/market.py - MarketPrice, OrderBook
 - shared/arbees_shared/models/signal.py - TradingSignal, ImpactAnalysis
 - shared/arbees_shared/models/trade.py - PaperTrade, Position

 Improvements:
 - Use ConfigDict(frozen=True) for immutability/caching
 - Use computed_field for derived properties
 - Use discriminated unions for polymorphic types

 2.2 TimescaleDB Schema

 File: shared/arbees_shared/db/migrations/001_initial.sql

 Tables:
 - games - Game metadata (relational)
 - game_states - Time-series hypertable
 - plays - Time-series hypertable
 - market_prices - Time-series hypertable
 - arbitrage_opportunities - Time-series hypertable
 - paper_trades - Trade records with PnL

 2.3 Redis Messaging

 File: shared/arbees_shared/messaging/redis_bus.py

 Type-safe pub/sub with msgpack serialization:
 - game:{game_id}:state - Game state updates
 - game:{game_id}:play - New plays detected
 - signals:new - Trading signals
 - trades:executed - Paper trade confirmations

 Verification

 docker-compose up timescaledb redis -d
 alembic upgrade head
 pytest tests/integration/test_database.py
 pytest tests/integration/test_messaging.py

 ---
 Phase 3: Market Clients

 3.1 Async Base Client

 File: markets/base.py

 Features:
 - Connection pooling with aiohttp
 - Rate limiting
 - Retry with tenacity
 - Abstract methods: get_markets(), get_orderbook(), stream_prices()

 3.2 Kalshi Client

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\kalshi\kalshi_client.py
 Target: markets/kalshi/client.py

 Improvements:
 - RSA signature caching (reduce crypto overhead)
 - WebSocket support for real-time prices
 - Proper async/await throughout

 3.3 Polymarket Client

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\polymarket\polymarket_client.py
 Target: markets/polymarket/client.py

 Improvements:
 - Designed as standalone microservice for EU deployment
 - No VPN/RPi hacks - proper cloud deployment
 - Robust token ID resolution

 3.4 Paper Trading Engine

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\paper_trader.py
 Target: markets/paper/engine.py

 Features:
 - Kelly criterion position sizing
 - Execution tracking with slippage modeling
 - PnL calculation and reporting

 Verification

 python -m markets.kalshi.test_auth
 python -m markets.polymarket.test_fetch
 pytest tests/integration/test_market_clients.py

 ---
 Phase 4: Data Providers & Game Engine

 4.1 ESPN Data Provider

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\agents\live_game\espn_*.py (6 sport clients)
 Target: data_providers/espn/

 Files:
 - base.py - DataProvider interface
 - client.py - ESPN API client
 - parsers/nfl.py, parsers/nba.py, etc. - Sport-specific parsing

 4.2 GameShard Service (KEY IMPROVEMENT)

 Target: services/game_shard/shard.py

 Architecture:
 - Single process handles 10-20 games via asyncio
 - Shared connections to DB, Redis, markets
 - Dynamic poll intervals (1s crunch time, 30s halftime)
 - Uses Rust core for win probability

 Methods:
 - add_game(game_id, sport) - Start monitoring
 - remove_game(game_id) - Stop monitoring
 - _poll_game_state(ctx) - ESPN polling loop
 - _poll_market_prices(ctx) - Market polling loop
 - _generate_signals(ctx, old_prob, new_prob, play) - Signal generation

 Benefits over container-per-game:
 - 10x lower memory footprint
 - Instant game start (no container spawn)
 - Better resource utilization

 4.3 Orchestrator Service

 Target: services/orchestrator/orchestrator.py

 Responsibilities:
 - Discover live games across all sports
 - Assign games to shards with load balancing
 - Monitor shard health
 - Handle shard failures with redistribution

 Verification

 pytest tests/integration/test_espn_provider.py
 pytest tests/unit/test_game_shard.py
 python benchmarks/shard_load_test.py --games 20

 ---
 Phase 5: API & Frontend

 5.1 FastAPI Backend

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\webapp\backend\main.py
 Target: services/api/

 Improvements:
 - Proper dependency injection with Depends
 - WebSocket connection management
 - OpenTelemetry instrumentation
 - Structured routers: opportunities, games, trades, monitoring

 5.2 React Frontend

 Source: P:\petes_code\ClaudeCode\IDBGfSportsBetS\webapp\frontend/
 Target: frontend/

 Port existing components (50+) with improvements:
 - React Query v5 patterns
 - Real-time WebSocket updates
 - Zustand for simpler state management

 Verification

 cd services/api && uvicorn main:app --reload
 cd frontend && npm run dev
 # Test WebSocket connections

 ---
 Phase 6: Infrastructure & Deployment

 6.1 Terraform

 Target: infrastructure/terraform/

 US Region (us-east-1):
 - ECS cluster: orchestrator, game_shard (x3), api (x2)
 - RDS: TimescaleDB
 - ElastiCache: Redis
 - ALB: Load balancer

 EU Region (eu-central-1):
 - ECS cluster: polymarket_proxy
 - VPC peering to US

 6.2 Docker Compose

 - docker-compose.yml - Local development
 - docker-compose.prod.yml - Production config

 Verification

 terraform plan
 docker-compose up -d
 python scripts/test_full_pipeline.py

 ---
 Critical Files Reference
 ┌────────────────────────────────────────────────────────┬────────────────────────────────────┬──────────────────────────┐
 │                         Source                         │               Target               │         Purpose          │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/rust/arb_core/src/lib.rs               │ rust_core/src/lib.rs               │ Arb detection + win prob │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/agents/live_game/models.py             │ shared/arbees_shared/models/       │ Pydantic models          │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/agents/kalshi/kalshi_client.py         │ markets/kalshi/client.py           │ Kalshi API               │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/agents/polymarket/polymarket_client.py │ markets/polymarket/client.py       │ Polymarket API           │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/agents/live_game/engine.py             │ services/game_shard/shard.py       │ Game monitoring          │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/agents/arbitrage/fee_calculator.py     │ shared/arbees_shared/utils/fees.py │ Fee calculations         │
 ├────────────────────────────────────────────────────────┼────────────────────────────────────┼──────────────────────────┤
 │ IDBGfSportsBetS/webapp/backend/main.py                 │ services/api/main.py               │ FastAPI backend          │
 └────────────────────────────────────────────────────────┴────────────────────────────────────┴──────────────────────────┘
 ---
 Execution Order

 1. Phase 1 - Scaffold + Rust (foundation)
 2. Phase 2 - Shared lib + DB (data layer)
 3. Phase 3 - Market clients (external integrations)
 4. Phase 4 - Game engine (core business logic)
 5. Phase 5 - API + Frontend (user interface)
 6. Phase 6 - Infrastructure (deployment)

 Each phase builds on the previous. Run verification steps before proceeding.