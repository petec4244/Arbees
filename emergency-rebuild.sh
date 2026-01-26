#!/bin/bash
# Emergency rebuild script - fixes all three bugs

echo "=========================================="
echo "EMERGENCY REBUILD - FIXING ALL 3 BUGS"
echo "=========================================="
echo ""
echo "Bug #1: Kelly sizing for sell signals"
echo "Bug #2: Only emit strongest edge per game"  
echo "Bug #3: Hold winning positions to settlement"
echo ""
echo "All code fixes are already applied!"
echo "Just need to rebuild Docker containers..."
echo ""

# Stop all services
echo "Stopping all services..."
docker-compose down

# Remove old builds to force clean rebuild
echo "Cleaning old builds..."
docker-compose rm -f

# Rebuild all Rust services
echo "Rebuilding Rust services..."
docker-compose build --no-cache \
    arbees_rust_core \
    game_shard \
    signal_processor \
    position_tracker \
    execution_service \
    orchestrator \
    market-discovery-rust

# Start everything
echo "Starting services..."
docker-compose up -d

# Wait for services to be ready
echo "Waiting for services to start..."
sleep 10

# Check service status
echo ""
echo "=========================================="
echo "SERVICE STATUS"
echo "=========================================="
docker-compose ps

echo ""
echo "=========================================="
echo "WATCHING LOGS - Press Ctrl+C to exit"
echo "=========================================="
echo ""
echo "Looking for:"
echo "  ✅ 'SIGNAL: Team X to win/lose' (only ONE per game)"
echo "  ✅ 'OPEN: ... - \$XX.XX' (sell trades should have normal \$ amounts, not \$1)"
echo "  ✅ 'holding_for_settlement' (don't close winners early)"
echo ""

# Follow logs from key services
docker-compose logs -f --tail=50 game_shard signal_processor position_tracker
