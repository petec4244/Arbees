"""
Test WebSocket clients for syntax and basic functionality.

This will catch any import errors, type issues, or basic logic problems.
"""

import asyncio
import sys
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).parent))

async def test_kalshi_ws():
    """Test Kalshi WebSocket client initialization."""
    try:
        from markets.kalshi.websocket import KalshiWebSocketClient
        
        # Test initialization
        client = KalshiWebSocketClient(api_key="test_key")
        
        # Test properties
        assert client.subscribed_markets == set()
        assert not client.is_connected
        
        print("✓ Kalshi WebSocket client: Syntax and initialization OK")
        return True
        
    except Exception as e:
        print(f"✗ Kalshi WebSocket client error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_polymarket_ws():
    """Test Polymarket WebSocket client initialization."""
    try:
        from markets.polymarket.websocket import PolymarketWebSocketClient
        
        # Test initialization
        client = PolymarketWebSocketClient()
        
        # Test properties
        assert client.subscribed_markets == set()
        assert not client.is_connected
        
        print("✓ Polymarket WebSocket client: Syntax and initialization OK")
        return True
        
    except Exception as e:
        print(f"✗ Polymarket WebSocket client error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_execution_engine():
    """Test execution engine initialization."""
    try:
        from services.execution_engine import ExecutionEngine, OrderResult, ArbitrageExecution
        from arbees_shared.models.market import Platform
        
        # Test OrderResult dataclass
        result = OrderResult(
            success=True,
            platform=Platform.KALSHI,
            market_id="test",
            side="yes",
            price=0.5,
            quantity=10,
        )
        assert result.success
        
        print("✓ Execution Engine: Syntax and initialization OK")
        return True
        
    except Exception as e:
        print(f"✗ Execution Engine error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_market_discovery():
    """Test market discovery service initialization."""
    try:
        # NOTE: MarketDiscoveryService is now handled by Rust service
        # from services.market_discovery import MarketDiscoveryService
        MarketDiscoveryService = None  # Rust service handles this now
        
        # Check team cache exists
        from pathlib import Path
        team_cache = Path(__file__).parent / "services" / "market_discovery" / "team_cache.json"
        assert team_cache.exists(), f"Team cache not found at {team_cache}"
        
        print("✓ Market Discovery Service: Syntax and team cache OK")
        return True
        
    except Exception as e:
        print(f"✗ Market Discovery Service error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def main():
    """Run all tests."""
    print("=" * 80)
    print("TESTING CRITICAL FIXES")
    print("=" * 80)
    
    results = []
    
    results.append(await test_kalshi_ws())
    results.append(await test_polymarket_ws())
    results.append(await test_execution_engine())
    results.append(await test_market_discovery())
    
    print("=" * 80)
    if all(results):
        print("✓ ALL TESTS PASSED")
    else:
        print(f"✗ {sum(not r for r in results)} TESTS FAILED")
    print("=" * 80)


if __name__ == "__main__":
    asyncio.run(main())
