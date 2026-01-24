# Build stage
FROM rust:1.75-bullseye as builder

WORKDIR /app

# Copy workspace files first
COPY Cargo.toml Cargo.lock ./
COPY rust_core ./rust_core
COPY services/arbees_rust_core ./services/arbees_rust_core
COPY services/position_tracker_rust ./services/position_tracker_rust

# Build
RUN cargo build --release --package position_tracker_rust

# Runtime stage
FROM debian:bullseye-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl1.1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/position_tracker /app/position_tracker

# Environment
ENV RUST_LOG=info

EXPOSE 8080

CMD ["/app/position_tracker"]
