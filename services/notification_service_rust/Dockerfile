# Build stage
FROM rust:1.85-bookworm as builder

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
COPY services/notification_service_rust /build/services/notification_service_rust
COPY services/zmq_listener_rust /build/services/zmq_listener_rust

# Build release binary
# Limit parallel jobs to avoid memory exhaustion
RUN CARGO_BUILD_JOBS=2 cargo build --release --bin notification_service

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/notification_service /app/notification_service

# Set environment
ENV RUST_LOG=info
ENV REDIS_URL=redis://redis:6379

CMD ["/app/notification_service"]

