# Polymarket RPi Monitor - ARM64 with OpenVPN support
# For Raspberry Pi deployment with geo-bypass

FROM python:3.11-slim-bookworm

# Install system dependencies including OpenVPN
RUN apt-get update && apt-get install -y --no-install-recommends \
    openvpn \
    iptables \
    iproute2 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy requirements first for better caching
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy application code
COPY __init__.py .
COPY client.py .
COPY monitor.py .
COPY openvpn_entrypoint.sh /entrypoint.sh

# Make entrypoint executable
RUN chmod +x /entrypoint.sh

# Create directory for OpenVPN config
RUN mkdir -p /etc/openvpn

# Expose ZMQ port
EXPOSE 5555

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD python -c "import zmq; c=zmq.Context(); s=c.socket(zmq.REQ); s.connect('tcp://localhost:5555'); exit(0)" || exit 1

# Environment defaults
ENV ZMQ_PUBLISH_ENABLED=true \
    ZMQ_PUBLISH_ADDRESS=tcp://*:5555 \
    POLL_INTERVAL_SECONDS=10 \
    LOG_LEVEL=INFO \
    OPENVPN_CONFIG=/etc/openvpn/client.ovpn

# Use entrypoint for VPN setup
ENTRYPOINT ["/entrypoint.sh"]
CMD ["python", "monitor.py"]
