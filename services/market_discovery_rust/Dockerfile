# Build stage
FROM rustlang/rust:nightly as builder

WORKDIR /build

# Copy workspace definition
COPY Cargo.toml Cargo.lock ./

# Copy workspace members
COPY services/Cargo.toml ./services/Cargo.toml
COPY services/arbees_rust_core ./services/arbees_rust_core
COPY services/game_shard_rust ./services/game_shard_rust
COPY services/market_discovery_rust ./services/market_discovery_rust
COPY services/orchestrator_rust ./services/orchestrator_rust
COPY services/execution_service_rust ./services/execution_service_rust
COPY services/signal_processor_rust ./services/signal_processor_rust
COPY services/position_tracker_rust ./services/position_tracker_rust

# Build release binary
RUN cargo build --release --bin market_discovery

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    redis-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/market_discovery /app/market_discovery

# Set environment
ENV RUST_LOG=info
ENV REDIS_URL=redis://redis:6379

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s \
  CMD redis-cli -u $REDIS_URL ping || exit 1

CMD ["/app/market_discovery"]
