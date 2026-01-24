FROM rustlang/rust:nightly as builder

WORKDIR /usr/src/app

# Copy workspace definition
COPY Cargo.toml Cargo.lock ./

# Copy workspace members
COPY rust_core rust_core
COPY services/arbees_rust_core services/arbees_rust_core
COPY services/market_discovery_rust services/market_discovery_rust
COPY services/orchestrator_rust services/orchestrator_rust
COPY services/game_shard_rust services/game_shard_rust
COPY services/execution_service_rust services/execution_service_rust
COPY services/signal_processor_rust services/signal_processor_rust
COPY services/position_tracker_rust services/position_tracker_rust

# Build specific member
RUN cargo build --release --bin game_shard_rust

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/game_shard_rust /usr/local/bin/game_shard_rust

# Copy .env if available, otherwise expect it to be mounted
# COPY .env .env 

CMD ["game_shard_rust"]
