# Polymarket RPi Monitor - Simplified (system VPN)
# VPN is handled at the host level, not in container

FROM python:3.11-slim-bookworm

# Install minimal dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy requirements first for better caching
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy application code
COPY __init__.py .
COPY client.py .
COPY monitor.py .

# Expose ZMQ port
EXPOSE 5555

# Environment defaults
ENV ZMQ_PUBLISH_ENABLED=true \
    ZMQ_PUBLISH_ADDRESS=tcp://*:5555 \
    POLL_INTERVAL_SECONDS=10 \
    LOG_LEVEL=INFO

CMD ["python", "monitor.py"]
