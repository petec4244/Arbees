#!/usr/bin/env python3
"""
Crypto Support Verification Script

Verifies that crypto market support is working correctly by testing:
1. CoinGecko API connectivity
2. Polymarket crypto market discovery
3. Configuration settings
4. Volatility calculation data availability
"""

import os
import sys
import json
import subprocess
from pathlib import Path

# Add shared module to path
sys.path.insert(0, str(Path(__file__).parent.parent / "shared"))

try:
    import requests
except ImportError:
    print("Error: requests library not installed. Run: pip install requests")
    sys.exit(1)


def test_coingecko_api():
    """Test CoinGecko API connectivity and fetch BTC price."""
    print("\n1. Testing CoinGecko API...")
    try:
        resp = requests.get(
            "https://api.coingecko.com/api/v3/simple/price",
            params={"ids": "bitcoin", "vs_currencies": "usd"},
            timeout=10,
        )
        resp.raise_for_status()
        data = resp.json()
        btc_price = data.get("bitcoin", {}).get("usd")
        if btc_price:
            print(f"   BTC Price: ${btc_price:,.2f}")
            return True
        else:
            print("   Warning: No price data returned")
            return False
    except Exception as e:
        print(f"   Failed: {e}")
        return False


def test_polymarket_crypto_markets():
    """Test Polymarket Gamma API for crypto markets."""
    print("\n2. Testing Polymarket crypto market discovery...")
    try:
        resp = requests.get(
            "https://gamma-api.polymarket.com/markets",
            params={"closed": "false", "limit": 100},
            timeout=15,
        )
        resp.raise_for_status()
        markets = resp.json()

        # Filter for crypto-related markets
        crypto_keywords = ["bitcoin", "btc", "ethereum", "eth", "crypto", "solana", "sol"]
        crypto_markets = [
            m for m in markets
            if any(kw in m.get("question", "").lower() for kw in crypto_keywords)
        ]

        print(f"   Found {len(crypto_markets)} potential crypto markets")
        if crypto_markets:
            print("   Sample markets:")
            for m in crypto_markets[:3]:
                q = m.get("question", "")[:60]
                print(f"     - {q}...")
        return True
    except Exception as e:
        print(f"   Failed: {e}")
        return False


def test_kalshi_crypto_markets():
    """Test Kalshi API for crypto markets."""
    print("\n3. Testing Kalshi crypto market discovery...")
    try:
        resp = requests.get(
            "https://api.elections.kalshi.com/trade-api/v2/markets",
            params={"status": "open", "limit": 200},
            timeout=15,
        )
        resp.raise_for_status()
        data = resp.json()
        markets = data.get("markets", [])

        # Filter for crypto-related markets
        crypto_keywords = ["bitcoin", "btc", "ethereum", "eth", "crypto"]
        crypto_markets = [
            m for m in markets
            if any(kw in m.get("title", "").lower() for kw in crypto_keywords)
        ]

        print(f"   Found {len(crypto_markets)} crypto markets on Kalshi")
        if crypto_markets:
            print("   Sample markets:")
            for m in crypto_markets[:3]:
                title = m.get("title", "")[:60]
                print(f"     - {title}...")
        return True
    except Exception as e:
        print(f"   Failed: {e}")
        return False


def check_config():
    """Check if crypto is enabled in configuration."""
    print("\n4. Checking configuration...")
    env_path = Path(__file__).parent.parent / ".env"

    if env_path.exists():
        content = env_path.read_text()
        if "ENABLE_CRYPTO_MARKETS=true" in content:
            print("   ENABLE_CRYPTO_MARKETS=true (enabled)")
            return True
        elif "ENABLE_CRYPTO_MARKETS=false" in content:
            print("   ENABLE_CRYPTO_MARKETS=false (disabled)")
            return False
        else:
            print("   ENABLE_CRYPTO_MARKETS not set in .env")
            return None
    else:
        print("   .env file not found")
        return None


def test_volatility_data():
    """Test that historical price data is available for volatility calculation."""
    print("\n5. Testing volatility data availability...")
    try:
        resp = requests.get(
            "https://api.coingecko.com/api/v3/coins/bitcoin/market_chart",
            params={"vs_currency": "usd", "days": 30},
            timeout=15,
        )
        resp.raise_for_status()
        data = resp.json()
        price_count = len(data.get("prices", []))
        print(f"   Fetched {price_count} price points for 30-day volatility calculation")

        # Calculate simple volatility metric
        if price_count > 1:
            prices = [p[1] for p in data["prices"]]
            import math
            returns = [(prices[i] / prices[i-1] - 1) for i in range(1, len(prices))]
            daily_vol = (sum(r**2 for r in returns) / len(returns)) ** 0.5
            annual_vol = daily_vol * math.sqrt(365 * 24)  # Hourly data
            print(f"   Calculated BTC annualized volatility: {annual_vol*100:.1f}%")
        return True
    except Exception as e:
        print(f"   Failed: {e}")
        return False


def main():
    """Run all verification tests."""
    print("=" * 50)
    print("       Crypto Support Verification")
    print("=" * 50)

    results = {
        "CoinGecko API": test_coingecko_api(),
        "Polymarket Discovery": test_polymarket_crypto_markets(),
        "Kalshi Discovery": test_kalshi_crypto_markets(),
        "Volatility Data": test_volatility_data(),
    }

    config_status = check_config()

    print("\n" + "=" * 50)
    print("                 Summary")
    print("=" * 50)

    all_passed = True
    for name, passed in results.items():
        status = "PASS" if passed else "FAIL"
        color = "\033[92m" if passed else "\033[91m"
        reset = "\033[0m"
        print(f"   {name}: {color}{status}{reset}")
        if not passed:
            all_passed = False

    print()
    if config_status is None:
        print("   Config: Not configured - add ENABLE_CRYPTO_MARKETS=true to .env")
    elif config_status:
        print("   Config: Enabled")
    else:
        print("   Config: Disabled")

    print("\n" + "=" * 50)
    if all_passed:
        print("   All checks passed! Crypto support is ready.")
    else:
        print("   Some checks failed. See details above.")

    return 0 if all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
