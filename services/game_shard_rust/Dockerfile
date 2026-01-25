FROM rustlang/rust:nightly as builder

WORKDIR /usr/src/app

# Copy workspace definition
COPY Cargo.toml Cargo.lock ./

# Copy workspace members
COPY rust_core ./rust_core
COPY services/game_shard_rust ./services/game_shard_rust

# Build specific member
RUN cargo build --release --bin game_shard_rust

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/game_shard_rust /usr/local/bin/game_shard_rust

CMD ["game_shard_rust"]
