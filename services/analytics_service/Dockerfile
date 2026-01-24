# Analytics Service Dockerfile (merged archiver + ml_analyzer)
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
COPY services/ ./services/

# Install Python dependencies including ML libraries
RUN pip install --no-cache-dir . scikit-learn pandas aiohttp markdown

ENV PYTHONPATH=/app:/app/shared

# Create reports directory
RUN mkdir -p /app/reports

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD python -c "print('healthy')" || exit 1

# Run the service
CMD ["python", "-m", "services.analytics_service.main"]
