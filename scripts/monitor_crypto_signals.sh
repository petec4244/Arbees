#!/bin/bash

# Monitor Crypto Signals in Real-Time
# Watches for arbitrage detection, signal generation, and execution

set -e

echo "=========================================="
echo "Crypto Shard Signal Monitor"
echo "=========================================="
echo ""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
UPDATE_INTERVAL=5
DURATION=300  # 5 minutes default

log_header() {
    echo -e "${BLUE}=== $1 ===${NC}"
}

log_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

log_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

log_error() {
    echo -e "${RED}✗ $1${NC}"
}

# Function to get service metrics
get_metrics() {
    local service=$1
    local metric=$2

    docker compose logs $service 2>/dev/null | grep -c "$metric" || echo "0"
}

# Function to show recent activity
show_activity() {
    echo ""
    log_header "Recent Activity (Last 20 lines)"

    docker compose logs --tail=20 crypto_shard execution_service crypto-spot-monitor 2>&1 | \
        grep -E "(arbitrage|signal|ExecutionRequest|price|connected|error)" || \
        echo "No matching activity yet..."

    echo ""
}

# Function to show metrics
show_metrics() {
    echo ""
    log_header "Service Metrics"

    echo ""
    echo "crypto_shard:"
    prices=$(get_metrics "crypto_shard" "price")
    signals=$(get_metrics "crypto_shard" "arbitrage\|signal")
    errors=$(get_metrics "crypto_shard" "error\|Error")
    echo "  Prices processed: $prices"
    echo "  Signals generated: $signals"
    echo "  Errors: $errors"

    echo ""
    echo "execution_service:"
    exec_requests=$(get_metrics "execution_service" "ExecutionRequest\|ZMQ signal")
    executed=$(get_metrics "execution_service" "Executing\|Executed")
    exec_errors=$(get_metrics "execution_service" "error\|Error\|rejection")
    echo "  Execution requests: $exec_requests"
    echo "  Trades executed: $executed"
    echo "  Errors: $exec_errors"

    echo ""
    echo "crypto-spot-monitor:"
    spot_prices=$(get_metrics "crypto-spot-monitor" "Published.*spot prices")
    ws_errors=$(get_metrics "crypto-spot-monitor" "error\|Error")
    echo "  Spot prices published: $spot_prices"
    echo "  WebSocket errors: $ws_errors"

    echo ""
}

# Function to watch for signals
watch_signals() {
    echo ""
    log_header "Watching for Arbitrage Signals"
    echo "(Press Ctrl+C to stop)"
    echo ""

    docker compose logs -f crypto_shard execution_service 2>&1 | \
        grep -E "(arbitrage|ExecutionRequest|signal)" --line-buffered
}

# Main menu
show_menu() {
    echo ""
    echo "Options:"
    echo "  1) Show recent activity"
    echo "  2) Show metrics"
    echo "  3) Watch signals (live)"
    echo "  4) Show all service logs (live)"
    echo "  5) Check service health"
    echo "  6) Restart services"
    echo "  q) Quit"
    echo ""
}

check_health() {
    echo ""
    log_header "Service Health Check"
    echo ""

    services=("crypto_shard" "execution_service" "crypto-spot-monitor" "kalshi_monitor" "polymarket_monitor")

    for service in "${services[@]}"; do
        status=$(docker compose ps $service 2>/dev/null | grep -E "Up|Exit" | awk '{print $NF}')
        if [[ $status == *"Up"* ]]; then
            log_success "$service is running"
        else
            log_error "$service is not running"
        fi
    done

    echo ""
    echo "Network connectivity:"
    if docker compose exec -T crypto_shard nc -z localhost 5555 2>/dev/null; then
        log_success "crypto_shard can reach kalshi_monitor:5555"
    else
        log_warning "crypto_shard cannot reach kalshi_monitor:5555"
    fi

    echo ""
}

restart_services() {
    echo ""
    log_header "Restarting Services"

    docker compose restart crypto_shard execution_service crypto-spot-monitor

    echo ""
    log_success "Services restarted"
    sleep 5
}

# Interactive mode
if [ "$1" == "watch" ]; then
    watch_signals
    exit 0
elif [ "$1" == "metrics" ]; then
    show_metrics
    exit 0
elif [ "$1" == "activity" ]; then
    show_activity
    exit 0
fi

# Default interactive mode
while true; do
    show_menu
    read -p "Choose option: " choice

    case $choice in
        1) show_activity ;;
        2) show_metrics ;;
        3) watch_signals ;;
        4) docker compose logs -f crypto_shard execution_service crypto-spot-monitor ;;
        5) check_health ;;
        6) restart_services ;;
        q) echo "Exiting..."; exit 0 ;;
        *) log_error "Invalid option" ;;
    esac
done
