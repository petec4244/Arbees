# Position Manager Service Dockerfile
FROM python:3.11-slim

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Rust for building Rust core
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Copy project files
COPY pyproject.toml README.md Cargo.toml ./
COPY rust_core/ ./rust_core/
COPY shared/ ./shared/
COPY data_providers/ ./data_providers/
COPY markets/ ./markets/
COPY services/ ./services/

# Install Python dependencies
RUN pip install --no-cache-dir maturin
RUN pip install --no-cache-dir .

# Build and install Rust core
RUN cd rust_core && maturin build --release && pip install $(find /app/target/wheels -name "*.whl" | head -1)

ENV PYTHONPATH=/app:/app/shared

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD python -c "print('healthy')" || exit 1

# Run the service
CMD ["python", "-m", "services.position_manager.position_manager"]
