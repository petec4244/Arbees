# GameShard Service Dockerfile
FROM python:3.11-slim as base

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Rust for building Rust core
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Install Python dependencies
COPY pyproject.toml ./
RUN pip install --no-cache-dir -e ".[dev]"

# Build Rust core
COPY Cargo.toml ./
COPY rust_core/ ./rust_core/
RUN cd rust_core && pip install maturin && maturin develop --release

# Copy shared library
COPY shared/ ./shared/

# Copy service code
COPY services/game_shard/ ./services/game_shard/

# Copy data providers and market clients
COPY data_providers/ ./data_providers/
COPY markets/ ./markets/

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD python -c "import asyncio; from services.game_shard.shard import GameShard; print('healthy')" || exit 1

# Run the service
CMD ["python", "-m", "services.game_shard.shard"]
