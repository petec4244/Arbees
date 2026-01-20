# API Service Dockerfile
FROM python:3.11-slim

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy project files
COPY pyproject.toml README.md ./
COPY shared/ ./shared/
COPY data_providers/ ./data_providers/
COPY markets/ ./markets/
COPY services/ ./services/

# Install Python dependencies
RUN pip install --no-cache-dir .

ENV PYTHONPATH=/app:/app/shared

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Run the service
CMD ["uvicorn", "services.api.main:app", "--host", "0.0.0.0", "--port", "8000"]
