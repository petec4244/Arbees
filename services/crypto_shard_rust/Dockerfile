FROM rust:1.85-bookworm as builder

WORKDIR /app

# Copy workspace files first
COPY Cargo.toml Cargo.lock ./

# Copy ALL workspace members (Cargo needs to resolve entire workspace)
COPY rust_core ./rust_core
COPY services/crypto_shard_rust ./services/crypto_shard_rust
COPY services/game_shard_rust ./services/game_shard_rust
COPY services/market_discovery_rust ./services/market_discovery_rust
COPY services/orchestrator_rust ./services/orchestrator_rust
COPY services/execution_service_rust ./services/execution_service_rust
COPY services/signal_processor_rust ./services/signal_processor_rust
COPY services/position_tracker_rust ./services/position_tracker_rust
COPY services/notification_service_rust ./services/notification_service_rust
COPY services/zmq_listener_rust ./services/zmq_listener_rust

# Build specific member
# Limit parallel jobs to avoid memory exhaustion
RUN CARGO_BUILD_JOBS=2 cargo build --release --package crypto_shard_rust --verbose

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/crypto_shard_rust /usr/local/bin/crypto_shard_rust

CMD ["crypto_shard_rust"]
