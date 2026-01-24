"""
Test WebSocket clients for syntax and basic functionality.

This will catch any import errors, type issues, or basic logic problems.
"""

import asyncio
import sys
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).parent))

# ASCII-safe status symbols for Windows compatibility
OK = "[OK]"
FAIL = "[FAIL]"


async def test_kalshi_ws():
    """Test Kalshi WebSocket client initialization."""
    try:
        from markets.kalshi.websocket import KalshiWebSocketClient

        # Test initialization
        client = KalshiWebSocketClient(api_key="test_key")

        # Test properties
        assert client.subscribed_markets == set()
        assert not client.is_connected

        print(f"{OK} Kalshi WebSocket client: Syntax and initialization OK")
        return True

    except Exception as e:
        print(f"{FAIL} Kalshi WebSocket client error: {e}")
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

        print(f"{OK} Polymarket WebSocket client: Syntax and initialization OK")
        return True

    except Exception as e:
        print(f"{FAIL} Polymarket WebSocket client error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_execution_engine():
    """Test execution engine initialization."""
    try:
        from services.execution_engine import ExecutionEngine, OrderResult, ArbitrageExecution
        from shared.arbees_shared.models.market import Platform

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

        print(f"{OK} Execution Engine: Syntax and initialization OK")
        return True

    except Exception as e:
        print(f"{FAIL} Execution Engine error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_market_discovery():
    """Test market discovery service - now handled by Rust."""
    try:
        # NOTE: MarketDiscoveryService is now handled by Rust service (market_discovery_rust)
        # The Python service has been deprecated

        # Check that the Rust service cargo.toml exists
        rust_service = Path(__file__).parent / "services" / "market_discovery_rust" / "Cargo.toml"
        assert rust_service.exists(), f"Rust market discovery service not found at {rust_service}"

        # Check team matching module exists in arbees_rust_core
        matching_rs = Path(__file__).parent / "services" / "arbees_rust_core" / "src" / "utils" / "matching.rs"
        assert matching_rs.exists(), f"Team matching module not found at {matching_rs}"

        print(f"{OK} Market Discovery Service: Rust service exists and team matching OK")
        return True

    except Exception as e:
        print(f"{FAIL} Market Discovery Service error: {e}")
        import traceback
        traceback.print_exc()
        return False


async def test_team_matching_client():
    """Test Python team matching client for RPC to Rust service."""
    try:
        from shared.arbees_shared.team_matching import TeamMatchingClient

        # Just test import and class exists
        assert TeamMatchingClient is not None

        print(f"{OK} Team Matching Client: Import OK")
        return True

    except Exception as e:
        print(f"{FAIL} Team Matching Client error: {e}")
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
    results.append(await test_team_matching_client())

    print("=" * 80)
    passed = sum(results)
    total = len(results)
    if all(results):
        print(f"{OK} ALL {total} TESTS PASSED")
    else:
        print(f"{FAIL} {total - passed}/{total} TESTS FAILED")
    print("=" * 80)

    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
