# Arbees

Live sports arbitrage trading system that monitors games across multiple sports, ingests play-by-play data faster than prediction markets adjust, and generates trading signals based on information latency and mispricing.

## Quick Start

```bash
# Start infrastructure
docker-compose up -d

# Install Python package
pip install -e ".[dev]"

# Build Rust core
cd rust_core && maturin develop --release

# Run API
python -m services.api.main
```

## Architecture

- **Rust Core**: High-performance arbitrage detection and win probability calculations
- **TimescaleDB**: Time-series database for game states, prices, and trades
- **Redis**: Real-time pub/sub messaging
- **GameShard**: Async service handling multiple live games concurrently
- **FastAPI**: REST + WebSocket API for frontend

## Supported Markets

- Kalshi (US)
- Polymarket (EU proxy)
- Paper trading (simulation)

## Supported Sports

NFL, NBA, NHL, MLB, NCAAF, NCAAB, MLS, Soccer, Tennis, MMA
