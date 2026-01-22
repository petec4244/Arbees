# Market Discovery Rust Service

High-performance market discovery service written in Rust. Replaces the CPU-intensive Python discovery loop in the Orchestrator.

## Architecture

This service runs independently and:
1. **Polls** for live/upcoming games (currently hardcoded, will integrate ESPN API)
2. **Searches** Polymarket and Kalshi APIs concurrently using `tokio`
3. **Matches** team names using fuzzy logic (handles "Notre Dame" vs "Notre Dame Fighting Irish")
4. **Publishes** discovered market IDs to Redis for the Orchestrator to consume

## Redis Schema

**Key**: `discovery:game:{HomeTeam}_vs_{AwayTeam}`

**Value** (JSON):
```json
{
  "sport": "ncaab",
  "home": "Notre Dame",
  "away": "North Carolina",
  "polymarket_moneyline": "0x123abc...",
  "kalshi_moneyline": "NCAAB-ND-UNC-24"
}
```

## Running

### Standalone
```bash
cd services/market_discovery_rust
cargo run --release
```

### Docker
```bash
docker-compose --profile full up market-discovery-rust
```

## Configuration

Environment variables:
- `REDIS_URL`: Redis connection string (default: `redis://localhost:6379`)
- `RUST_LOG`: Log level (`info`, `debug`, `trace`)

## Development

### Adding a new provider
1. Create `src/providers/newprovider.rs`
2. Implement search and matching logic
3. Add to `src/providers/mod.rs`
4. Update `main.rs` to call the new provider

### Testing
```bash
cargo test
```
