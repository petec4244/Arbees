# Build stage
FROM rustlang/rust:nightly as builder

WORKDIR /app

# Copy workspace files first
COPY Cargo.toml Cargo.lock ./
COPY services/Cargo.toml ./services/Cargo.toml
COPY services/arbees_rust_core ./services/arbees_rust_core
COPY services/game_shard_rust ./services/game_shard_rust
COPY services/market_discovery_rust ./services/market_discovery_rust
COPY services/orchestrator_rust ./services/orchestrator_rust
COPY services/execution_service_rust ./services/execution_service_rust
COPY services/signal_processor_rust ./services/signal_processor_rust
COPY services/position_tracker_rust ./services/position_tracker_rust

# Build
RUN cargo build --release --package position_tracker_rust

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/position_tracker /app/position_tracker

# Environment
ENV RUST_LOG=info

EXPOSE 8080

CMD ["/app/position_tracker"]
