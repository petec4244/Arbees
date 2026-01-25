# Build stage
FROM rustlang/rust:nightly as builder

WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust_core ./rust_core
COPY services/signal_processor_rust ./services/signal_processor_rust

# Build
RUN cargo build --release --package signal_processor_rust

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/signal_processor /app/signal_processor

# Environment
ENV RUST_LOG=info

EXPOSE 8080

CMD ["/app/signal_processor"]
