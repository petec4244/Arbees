# Build stage
# NOTE: Cargo.lock v4 requires newer Cargo than 1.75 provides.
FROM rust:1.83 as builder

WORKDIR /build

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust_core ./rust_core
COPY services/market_discovery_rust ./services/market_discovery_rust

# Build release binary
RUN cargo build --release --bin market_discovery

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/market_discovery /app/market_discovery

# Set environment
ENV RUST_LOG=info
ENV REDIS_URL=redis://redis:6379

CMD ["/app/market_discovery"]
