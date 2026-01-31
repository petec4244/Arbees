#!/bin/bash

# Crypto Shard Deployment & Testing Script
# Deploys all crypto services and monitors for errors

set -e

echo "=========================================="
echo "Crypto Shard Deployment & Testing"
echo "=========================================="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
TIMEOUT=60
RETRY_ATTEMPTS=5

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_service() {
    local service=$1
    local port=$2

    log_info "Checking if $service is running..."

    # Try to check if port is listening
    if nc -z localhost $port 2>/dev/null; then
        log_info "$service is running on port $port"
        return 0
    else
        log_warn "$service not responding on port $port yet..."
        return 1
    fi
}

wait_for_service() {
    local service=$1
    local port=$2
    local attempt=1

    while [ $attempt -le $RETRY_ATTEMPTS ]; do
        if check_service "$service" "$port"; then
            return 0
        fi
        echo "  Attempt $attempt/$RETRY_ATTEMPTS... waiting 5s"
        sleep 5
        attempt=$((attempt + 1))
    done

    log_error "Failed to connect to $service on port $port after $RETRY_ATTEMPTS attempts"
    return 1
}

# Step 1: Start infrastructure
log_info "Step 1: Starting infrastructure (TimescaleDB, Redis)..."
docker compose up -d timescaledb redis
log_info "Waiting for infrastructure to be healthy..."
sleep 10

# Step 2: Start price monitors
log_info "Step 2: Starting price monitors (Kalshi, Polymarket, Spot)..."
docker compose up -d kalshi_monitor polymarket_monitor crypto-spot-monitor
log_info "Waiting for monitors to connect..."
sleep 5

# Step 3: Start crypto_shard
log_info "Step 3: Starting crypto_shard_rust..."
docker compose up -d crypto_shard
log_info "Waiting for crypto_shard to initialize..."
wait_for_service "crypto_shard" 5559

# Step 4: Start execution service
log_info "Step 4: Starting execution_service_rust..."
docker compose up -d execution_service
log_info "Waiting for execution_service to initialize..."
wait_for_service "execution_service" 5560

echo ""
log_info "========== DEPLOYMENT COMPLETE =========="
echo ""

# Step 5: Monitor services
log_info "Step 5: Monitoring services for 30 seconds..."
echo ""

docker compose logs -f --tail=20 2>&1 | grep -E "(crypto|execution|price|arbitrage|signal)" &
MONITOR_PID=$!

sleep 30

kill $MONITOR_PID 2>/dev/null || true

echo ""
log_info "========== DEPLOYMENT SUMMARY =========="
echo ""

# Check running services
echo "Running services:"
docker compose ps --filter "status=running" | grep -E "(crypto|execution|kalshi|polymarket|spot)" || true

echo ""
echo "Service endpoints:"
echo "  - crypto_shard: tcp://localhost:5559"
echo "  - execution_service: tcp://localhost:5560"
echo "  - kalshi_monitor: tcp://localhost:5555"
echo "  - polymarket_monitor: tcp://localhost:5556"
echo "  - crypto_spot_monitor: tcp://localhost:5560"
echo ""

log_info "To view logs:"
echo "  docker compose logs -f crypto_shard"
echo "  docker compose logs -f execution_service"
echo "  docker compose logs -f crypto-spot-monitor"
echo ""

log_info "To check for arbitrage signals:"
echo "  docker compose logs crypto_shard | grep -i 'arbitrage detected'"
echo ""

log_info "To stop all services:"
echo "  docker compose down"
echo ""
