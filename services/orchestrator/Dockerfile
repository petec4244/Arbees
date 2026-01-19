# Orchestrator Service Dockerfile
FROM python:3.11-slim as base

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Python dependencies
COPY pyproject.toml ./
RUN pip install --no-cache-dir -e ".[dev]"

# Copy shared library
COPY shared/ ./shared/

# Copy service code
COPY services/orchestrator/ ./services/orchestrator/

# Copy data providers and market clients
COPY data_providers/ ./data_providers/
COPY markets/ ./markets/

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD python -c "import asyncio; from services.orchestrator.orchestrator import Orchestrator; print('healthy')" || exit 1

# Run the service
CMD ["python", "-m", "services.orchestrator.orchestrator"]
