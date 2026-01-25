FROM rustlang/rust:nightly as builder

WORKDIR /usr/src/app

# Copy workspace definition
COPY Cargo.toml Cargo.lock ./

# Copy workspace members
COPY rust_core ./rust_core
COPY services/execution_service_rust ./services/execution_service_rust

# Build specific member
RUN cargo build --release --bin execution_service_rust

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/execution_service_rust /usr/local/bin/execution_service_rust

CMD ["execution_service_rust"]
