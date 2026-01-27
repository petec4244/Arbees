# Kalshi Monitor Service Dockerfile
# No VPN needed - Kalshi API is accessible from US
FROM python:3.11-slim

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy project files (same pattern as polymarket_monitor)
COPY pyproject.toml README.md ./
COPY shared/ ./shared/
COPY data_providers/ ./data_providers/
COPY markets/ ./markets/
COPY services/ ./services/

# Install Python dependencies
RUN pip install --no-cache-dir .

# Install additional dependencies for this service
RUN pip install --no-cache-dir httpx loguru

ENV PYTHONPATH=/app:/app/shared

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD python -c "print('healthy')" || exit 1

# Run the service
CMD ["python", "-m", "services.kalshi_monitor.monitor"]
