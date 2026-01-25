FROM rustlang/rust:nightly as builder

WORKDIR /usr/src/app

# Copy workspace definition (from project root via docker-compose or services/ via manual)
COPY services/Cargo.toml services/Cargo.lock ./

# Copy workspace members (only those in Cargo.toml workspace)
COPY services/arbees_rust_core arbees_rust_core
COPY services/market_discovery_rust market_discovery_rust
COPY services/orchestrator_rust orchestrator_rust
COPY services/game_shard_rust game_shard_rust
COPY services/execution_service_rust execution_service_rust
COPY services/signal_processor_rust signal_processor_rust
COPY services/position_tracker_rust position_tracker_rust

# Build specific member
RUN cargo build --release --bin game_shard_rust

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/game_shard_rust /usr/local/bin/game_shard_rust

CMD ["game_shard_rust"]
