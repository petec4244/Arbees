# Project Migration: IDBGfSportsBetS → Arbees

## Overview

Migrate the existing sports betting arbitrage system from `P:\petes_code\ClaudeCode\IDBGfSportsBetS` to a new, clean project called `Arbees` at `P:\petes_code\ClaudeCode\Arbees`.

The new project is a **live sports arbitrage system** that:
- Monitors live games across multiple sports (NFL, NBA, NHL, MLB, NCAA, MLS, Soccer, Tennis, MMA)
- Ingests play-by-play data faster than prediction markets adjust
- **Leverages Rust** for high-performance signal generation and parsing (`shared/core` with PyO3)
- Generates trading signals based on information latency and mispricing
- Supports multiple simultaneous games via **Sharded Agents** (more efficient than container-per-game)
- The Trader initially executes paper trades and will support future live trading capability
- Deploys to AWS multi-region (**us-east-1** for Kalshi/Core, **eu-central-1** for Polymarket), removing reliance on RPi tunnels
- Exposes a web portal hosted externally (under existing domains) for monitoring and control

## MCP Agents for Migration

Before starting, set up these MCP agents to track progress:

### 1. Migration Tracker (Create First)

Create a migration tracking file that we'll update throughout:
```
Arbees/MIGRATION_STATUS.md
```

### 2. File Inventory Agent

Inventory the old project to understand what exists:
```bash
find P:/petes_code/ClaudeCode/IDBGfSportsBetS -type f -name "*.py" | head -100 > /tmp/old_project_py_files.txt
find P:/petes_code/ClaudeCode/IDBGfSportsBetS -type f -name "*.yml" -o -name "*.yaml" > /tmp/old_project_yaml_files.txt
find P:/petes_code/ClaudeCode/IDBGfSportsBetS -type f -name "Dockerfile*" > /tmp/old_project_dockerfiles.txt
```

### 3. Validation Agent

After each major component, run validation:
- Python syntax check: `python -m py_compile <file>`
- Rust build check: `maturin develop`
- Docker build check: `docker build -t test .`

---

## Project Structure to Create

```
Arbees/
├── README.md
├── MIGRATION_STATUS.md                 # Track migration progress
├── .gitignore
├── .env.example
├── docker-compose.yml                  # Local development
├── docker-compose.prod.yml             # Production (AWS)
├── pyproject.toml                      # Python project config (uv/poetry)
├── Cargo.toml                          # Rust workspace config
│
├── infrastructure/                     # AWS/Terraform
│   ├── terraform/                      # IaC for AWS
│   └── scripts/                        # Deployment scripts
│
├── rust_core/                          # Rust High-Performance Module
│   ├── Cargo.toml
│   ├── pyproject.toml                  # Maturin config
│   └── src/
│       ├── lib.rs                      # PyO3 bindings
│       ├── arb.rs                      # Arbitrage calc
│       └── win_prob.rs                 # Fast win probability (Monte Carlo?)
│
├── shared/                             # Shared Python package
│   ├── __init__.py
│   ├── core.pyi                        # Type stubs for rust_core
│   ├── models/                         # Pydantic models (Sport, Game, Market, Signal)
│   ├── messaging/                      # Redis/SQS bus
│   ├── db/                             # TimescaleDB (Postgres) + Redis
│   └── utils/
│
├── data_providers/                     # External data sources
│   ├── base.py
│   ├── goalserve/                      # Primary
│   └── espn/                           # Backup
│
├── markets/                            # Market integrations
│   ├── kalshi/                         # Kalshi Client
│   ├── polymarket/                     # Polymarket Client (runs in EU container)
│   └── paper/                          # Paper Trading Engine
│
├── services/
│   ├── orchestrator/                   # Managing Game Shards
│   ├── game_shard/                     # Handles N games concurrently
│   ├── position_manager/               # Risk & Execution
│   └── api/                            # REST API + WebSocket
│
├── frontend/                           # React dashboard
│   ├── src/
│   └── Dockerfile
│
└── tests/
    ├── unit/
    ├── integration/
    └── e2e/
```

---

## Migration Phases

### Phase 1: Project Scaffold & Rust Core
1.  Initialize Git & Directories
2.  Setup `pyproject.toml` & `Cargo.toml`
3.  **Rust Integration**: Port `rust/arb_core` to `Arbees/rust_core` and set up `maturin` build.
    - *Goal*: Ensure `import arbees_core` works in Python.

### Phase 2: Shared Library & Database
1.  **TimescaleDB**: Setup `shared/db/postgres.py`. Move away from InfluxDB for structured trade data.
    - *Why*: Better for relational queries ("Show all trades for KC Chiefs > $50").
2.  **Redis**: Setup `shared/messaging/redis_bus.py` for real-time pub/sub.
3.  **Models**: Migrate `agents/live_game/models.py` to `shared/models/`.

### Phase 3: Market Clients (Refactor)
1.  **Polymarket**: Create `markets/polymarket/` designed to run as a standalone microservice (for EU deployment).
2.  **Kalshi**: Migrate existing Kalshi client.
3.  **Paper Trader**: Port the new `RealTimePaperTrader` logic.

### Phase 4: Data & Game Engine
1.  **Data Providers**: Standardize ESPN/Goalserve behind an interface.
2.  **Game Shard**: Instead of "Container-per-game", create a `GameShard` service that can monitor 10-20 games via `asyncio`.
    - *Why*: Reduces AWS Fargate costs significantly.
    - *Logic*: `GameShard` receives a list of GameIDs to monitor from Orchestrator.

### Phase 5: Infrastructure & Deployment
1.  **AWS Setup**: Terraform for:
    - VPC Peering (US <-> EU)
    - Redis (ElastiCache)
    - TimescaleDB (RDS or EC2)
    - ECS Services (`game-shard`, `polymarket-proxy`, `api`)
2.  **Frontend**: Hook up React dashboard to `services/api` via WebSockets.

---

### 5. Latency & Observability Architecture
- **Distributed Tracing**: Implement OpenTelemetry headers in every message.
    - Track time from: `Provider_Event_Time` -> `Ingest_Time` -> `Parse_Time` -> `Signal_Gen_Time` -> `Execution_Time`.
- **LatencyMonitor Agent**: A dedicated service that consumes metric streams.
    - **Bottleneck Detection**: "Why is NBA processing 50ms slower today?"
    - **Market Latency Profiling**: "Kalshi is lagging Polymarket by 1.2s on game X." -> *Strategy Adjustment*.
    - **Feed Health**: "Goalserve has stopped sending updates for 10s."

---

## Key Technical Decisions

### 1. Database: TimescaleDB vs InfluxDB
- **Decision**: Move to TimescaleDB (PostgreSQL extension).
- **Reasoning**: We need relational data (Players, Teams, Trades) *and* time-series (Prices). Postgres handles both. InfluxDB is great for metrics but poor for structured trade logs and complex queries.

### 2. Architecture: Sharded Services
- **Decision**: `GameShard` service instead of "One Container Per Game".
- **Reasoning**: Running 15 containers for 15 NFL games is resource-heavy (memory overhead). Python `asyncio` can easily handle I/O for 20 games in a single process. We can scale horizontal shards if needed (Shard 1: NFL, Shard 2: NBA).

### 3. Rust Integration
- **Decision**: Use `maturin` for PyO3 bindings.
- **Reasoning**: Simplest way to mix Python/Rust. Critical calculations (Implied Prob, Arb Edge) happen in Rust.

### 4. Remote Polymarket Agent
- **Decision**: Run a dedicated ECS service in `eu-central-1`.
- **Reasoning**: More reliable than RPi. Configurable via Terraform. Connects back to US Core via internal private subnet peering or secure VPN.
