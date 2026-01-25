# syntax=docker/dockerfile:1.4
# Build stage
FROM rustlang/rust:nightly as builder

WORKDIR /build

# Copy workspace definition
COPY Cargo.toml /build/
COPY Cargo.lock /build/

# Copy ALL workspace members (Cargo needs to resolve entire workspace)
COPY rust_core /build/rust_core
COPY services/game_shard_rust /build/services/game_shard_rust
COPY services/market_discovery_rust /build/services/market_discovery_rust
COPY services/orchestrator_rust /build/services/orchestrator_rust
COPY services/execution_service_rust /build/services/execution_service_rust
COPY services/signal_processor_rust /build/services/signal_processor_rust
COPY services/position_tracker_rust /build/services/position_tracker_rust

# Build release binary
RUN cargo build --release --bin orchestrator_rust

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/orchestrator_rust /app/orchestrator_rust

# Set environment
ENV RUST_LOG=info
ENV REDIS_URL=redis://redis:6379

CMD ["/app/orchestrator_rust"]
