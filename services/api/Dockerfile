# API Service Dockerfile
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
COPY services/api/ ./services/api/

# Copy market clients
COPY markets/ ./markets/

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/api/monitoring/health || exit 1

# Run the service
CMD ["uvicorn", "services.api.main:app", "--host", "0.0.0.0", "--port", "8000"]
