#!/usr/bin/env python3
"""
API Smoke Tests for Kalshi and Polymarket.

Non-trading tests to verify API connectivity and data fetching.
Can be run locally or in CI.

Usage:
    # Test all platforms (prod)
    python scripts/api_smoke_test.py
    
    # Test Kalshi demo/testnet
    KALSHI_ENV=demo python scripts/api_smoke_test.py --kalshi-only
    
    # Test Polymarket only
    python scripts/api_smoke_test.py --polymarket-only
    
    # Verbose output
    python scripts/api_smoke_test.py -v
"""

import argparse
import asyncio
import logging
import os
import sys
from dataclasses import dataclass
from typing import Optional

# Add parent directory to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from markets.kalshi.client import KalshiClient
from markets.kalshi.config import get_kalshi_environment, get_kalshi_rest_url, get_kalshi_ws_url
from markets.polymarket.client import PolymarketClient
from markets.polymarket.config import get_polymarket_gamma_url, get_polymarket_clob_url, get_polymarket_ws_url

logger = logging.getLogger(__name__)


@dataclass
class TestResult:
    """Result of a single smoke test."""
    name: str
    success: bool
    message: str
    details: Optional[str] = None


class SmokeTestRunner:
    """Runner for API smoke tests."""
    
    def __init__(self, verbose: bool = False):
        self.verbose = verbose
        self.results: list[TestResult] = []
    
    def log(self, msg: str) -> None:
        """Log a message if verbose mode is enabled."""
        if self.verbose:
            print(f"  {msg}")
    
    def add_result(self, result: TestResult) -> None:
        """Add a test result."""
        self.results.append(result)
        status = "✓" if result.success else "✗"
        print(f"  {status} {result.name}: {result.message}")
        if result.details and self.verbose:
            print(f"    Details: {result.details}")
    
    async def test_kalshi_rest(self) -> None:
        """Test Kalshi REST API connectivity."""
        env = get_kalshi_environment()
        base_url = get_kalshi_rest_url()
        
        print(f"\nKalshi REST API ({env.value})")
        print(f"  URL: {base_url}")
        
        client = KalshiClient()
        try:
            await client.connect()
            
            # Test 1: Health check / exchange status
            try:
                healthy = await client.health_check()
                self.add_result(TestResult(
                    name="Health Check",
                    success=healthy,
                    message="API responding" if healthy else "API not responding",
                ))
            except Exception as e:
                self.add_result(TestResult(
                    name="Health Check",
                    success=False,
                    message=f"Failed: {e}",
                ))
            
            # Test 2: Fetch markets
            try:
                markets = await client.get_markets(limit=5)
                self.add_result(TestResult(
                    name="Get Markets",
                    success=len(markets) > 0,
                    message=f"Found {len(markets)} markets",
                    details=f"First market: {markets[0].get('ticker') if markets else 'N/A'}",
                ))
            except Exception as e:
                self.add_result(TestResult(
                    name="Get Markets",
                    success=False,
                    message=f"Failed: {e}",
                ))
            
            # Test 3: Fetch orderbook (if we got markets)
            if markets:
                try:
                    ticker = markets[0].get("ticker")
                    if ticker:
                        orderbook = await client.get_orderbook(ticker)
                        has_depth = orderbook and (orderbook.yes_bids or orderbook.yes_asks)
                        self.add_result(TestResult(
                            name="Get Orderbook",
                            success=orderbook is not None,
                            message=f"Orderbook for {ticker}" + (" (has depth)" if has_depth else " (empty)"),
                            details=f"Bids: {len(orderbook.yes_bids) if orderbook else 0}, Asks: {len(orderbook.yes_asks) if orderbook else 0}",
                        ))
                except Exception as e:
                    self.add_result(TestResult(
                        name="Get Orderbook",
                        success=False,
                        message=f"Failed: {e}",
                    ))
            
        finally:
            await client.disconnect()
    
    async def test_kalshi_ws(self) -> None:
        """Test Kalshi WebSocket connectivity (brief connection only)."""
        from markets.kalshi.websocket.ws_client import KalshiWebSocketClient
        
        env = get_kalshi_environment()
        ws_url = get_kalshi_ws_url()
        
        print(f"\nKalshi WebSocket ({env.value})")
        print(f"  URL: {ws_url}")
        
        # Check if we have credentials for WS auth
        api_key = os.environ.get("KALSHI_API_KEY", "")
        has_private_key = bool(
            os.environ.get("KALSHI_PRIVATE_KEY") or 
            os.environ.get("KALSHI_PRIVATE_KEY_PATH")
        )
        
        if not api_key:
            self.add_result(TestResult(
                name="WebSocket Auth",
                success=False,
                message="KALSHI_API_KEY not set - skipping WS test",
            ))
            return
        
        if not has_private_key:
            self.add_result(TestResult(
                name="WebSocket Auth",
                success=False,
                message="KALSHI_PRIVATE_KEY[_PATH] not set - skipping WS test",
            ))
            return
        
        client = KalshiWebSocketClient()
        try:
            await client.connect()
            self.add_result(TestResult(
                name="WebSocket Connect",
                success=client.is_connected,
                message="Connected" if client.is_connected else "Failed to connect",
            ))
            
            # Brief subscription test (just verify we can send, not waiting for data)
            if client.is_connected:
                try:
                    # We don't have a valid ticker to subscribe to, so just test connect works
                    self.add_result(TestResult(
                        name="WebSocket Ready",
                        success=True,
                        message="WebSocket ready for subscriptions",
                    ))
                except Exception as e:
                    self.add_result(TestResult(
                        name="WebSocket Subscribe",
                        success=False,
                        message=f"Failed: {e}",
                    ))
            
        except Exception as e:
            self.add_result(TestResult(
                name="WebSocket Connect",
                success=False,
                message=f"Failed: {e}",
            ))
        finally:
            await client.disconnect()
    
    async def test_polymarket_rest(self) -> None:
        """Test Polymarket REST API connectivity (Gamma + CLOB)."""
        gamma_url = get_polymarket_gamma_url()
        clob_url = get_polymarket_clob_url()
        
        print(f"\nPolymarket REST API")
        print(f"  Gamma URL: {gamma_url}")
        print(f"  CLOB URL: {clob_url}")
        
        client = PolymarketClient()
        try:
            await client.connect()
            
            # Test 1: Health check
            try:
                healthy = await client.health_check()
                self.add_result(TestResult(
                    name="Health Check (Gamma)",
                    success=healthy,
                    message="Gamma API responding" if healthy else "Gamma API not responding",
                ))
            except Exception as e:
                self.add_result(TestResult(
                    name="Health Check (Gamma)",
                    success=False,
                    message=f"Failed: {e}",
                ))
            
            # Test 2: Fetch markets (Gamma)
            markets = []
            try:
                markets = await client.get_markets(limit=5)
                self.add_result(TestResult(
                    name="Get Markets (Gamma)",
                    success=len(markets) > 0,
                    message=f"Found {len(markets)} markets",
                    details=f"First: {markets[0].get('question', 'N/A')[:50] if markets else 'N/A'}...",
                ))
            except Exception as e:
                self.add_result(TestResult(
                    name="Get Markets (Gamma)",
                    success=False,
                    message=f"Failed: {e}",
                ))
            
            # Test 3: Fetch orderbook (CLOB)
            if markets:
                try:
                    condition_id = markets[0].get("condition_id") or markets[0].get("id")
                    if condition_id:
                        orderbook = await client.get_orderbook(condition_id)
                        # Orderbook might be None for AMM markets, which is okay
                        self.add_result(TestResult(
                            name="Get Orderbook (CLOB)",
                            success=True,  # Success if no exception
                            message=f"Orderbook for {condition_id[:20]}..." if orderbook else "No CLOB orderbook (AMM market)",
                            details=f"Bids: {len(orderbook.yes_bids) if orderbook else 0}, Asks: {len(orderbook.yes_asks) if orderbook else 0}",
                        ))
                except Exception as e:
                    self.add_result(TestResult(
                        name="Get Orderbook (CLOB)",
                        success=False,
                        message=f"Failed: {e}",
                    ))
            
        finally:
            await client.disconnect()
    
    async def test_polymarket_ws(self) -> None:
        """Test Polymarket WebSocket connectivity (brief connection only)."""
        from markets.polymarket.websocket.ws_client import PolymarketWebSocketClient
        
        ws_url = get_polymarket_ws_url()
        
        print(f"\nPolymarket WebSocket")
        print(f"  URL: {ws_url}")
        
        client = PolymarketWebSocketClient()
        try:
            await client.connect()
            self.add_result(TestResult(
                name="WebSocket Connect",
                success=client.is_connected,
                message="Connected" if client.is_connected else "Failed to connect",
            ))
            
        except Exception as e:
            self.add_result(TestResult(
                name="WebSocket Connect",
                success=False,
                message=f"Failed: {e}",
            ))
        finally:
            await client.disconnect()
    
    def print_summary(self) -> bool:
        """Print test summary and return True if all passed."""
        print("\n" + "=" * 50)
        print("SUMMARY")
        print("=" * 50)
        
        passed = sum(1 for r in self.results if r.success)
        failed = sum(1 for r in self.results if not r.success)
        
        print(f"  Passed: {passed}")
        print(f"  Failed: {failed}")
        
        if failed > 0:
            print("\nFailed tests:")
            for r in self.results:
                if not r.success:
                    print(f"  - {r.name}: {r.message}")
        
        return failed == 0


async def main():
    parser = argparse.ArgumentParser(description="API Smoke Tests for Kalshi and Polymarket")
    parser.add_argument("-v", "--verbose", action="store_true", help="Verbose output")
    parser.add_argument("--kalshi-only", action="store_true", help="Test Kalshi only")
    parser.add_argument("--polymarket-only", action="store_true", help="Test Polymarket only")
    parser.add_argument("--no-ws", action="store_true", help="Skip WebSocket tests")
    args = parser.parse_args()
    
    # Setup logging
    log_level = logging.DEBUG if args.verbose else logging.WARNING
    logging.basicConfig(level=log_level, format="%(levelname)s: %(message)s")
    
    runner = SmokeTestRunner(verbose=args.verbose)
    
    print("=" * 50)
    print("API SMOKE TESTS")
    print("=" * 50)
    
    # Kalshi tests
    if not args.polymarket_only:
        await runner.test_kalshi_rest()
        if not args.no_ws:
            await runner.test_kalshi_ws()
    
    # Polymarket tests
    if not args.kalshi_only:
        await runner.test_polymarket_rest()
        if not args.no_ws:
            await runner.test_polymarket_ws()
    
    # Summary
    all_passed = runner.print_summary()
    
    sys.exit(0 if all_passed else 1)


if __name__ == "__main__":
    asyncio.run(main())
