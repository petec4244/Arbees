# syntax=docker/dockerfile:1.4
# Build stage
FROM rustlang/rust:nightly as builder

WORKDIR /build

# Copy workspace definition
COPY Cargo.toml /build/
COPY Cargo.lock /build/

# Copy workspace members
COPY rust_core /build/rust_core
COPY services/orchestrator_rust /build/services/orchestrator_rust

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
