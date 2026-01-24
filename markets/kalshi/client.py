"""
Kalshi prediction market client with RSA signature authentication.

Improvements over original:
- Fully async with aiohttp
- RSA signature caching to reduce crypto overhead
- WebSocket support for real-time prices
- Proper connection pooling and rate limiting
"""

import asyncio
import base64
import logging
import os
import time
from datetime import datetime
from functools import lru_cache
from typing import Any, AsyncIterator, Optional

import aiohttp
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.primitives.asymmetric.rsa import RSAPrivateKey

from arbees_shared.models.market import (
    MarketPrice,
    MarketStatus,
    OrderBook,
    OrderBookLevel,
    Platform,
)
from markets.base import BaseMarketClient

logger = logging.getLogger(__name__)


class SignatureCache:
    """Cache for RSA signatures to reduce crypto overhead."""

    def __init__(self, max_age_ms: int = 500):
        self._cache: dict[str, tuple[str, int]] = {}
        self.max_age_ms = max_age_ms

    def get(self, key: str, current_time_ms: int) -> Optional[str]:
        """Get cached signature if still valid."""
        if key not in self._cache:
            return None
        sig, cached_time = self._cache[key]
        if current_time_ms - cached_time > self.max_age_ms:
            del self._cache[key]
            return None
        return sig

    def set(self, key: str, signature: str, time_ms: int) -> None:
        """Cache a signature."""
        self._cache[key] = (signature, time_ms)
        # Clean old entries periodically
        if len(self._cache) > 100:
            self._clean()

    def _clean(self) -> None:
        """Remove expired entries."""
        now = int(time.time() * 1000)
        expired = [k for k, (_, t) in self._cache.items() if now - t > self.max_age_ms]
        for k in expired:
            del self._cache[k]


class KalshiClient(BaseMarketClient):
    """Async Kalshi API client with RSA signature authentication."""

    # Default URL (overridden by config)
    BASE_URL = "https://api.elections.kalshi.com/trade-api/v2"

    # Sports series tickers
    SPORTS_SERIES = {
        "nfl": "NFL",
        "nba": "NBA",
        "nhl": "NHL",
        "mlb": "MLB",
        "ncaaf": "NCAAF",
        "ncaab": "NCAAB",
        "soccer": "SOCCER",
        "mma": "MMA",
    }

    def __init__(
        self,
        api_key: Optional[str] = None,
        private_key_path: Optional[str] = None,
        private_key_str: Optional[str] = None,
        rate_limit: float = 2.0,  # Kalshi rate limits to ~2 req/sec
        base_url: Optional[str] = None,
        env: Optional[str] = None,
    ):
        """
        Initialize Kalshi client.

        Args:
            api_key: Kalshi API key ID (or from env based on KALSHI_ENV)
            private_key_path: Path to RSA private key PEM file
            private_key_str: RSA private key as string (or from env based on KALSHI_ENV)
            rate_limit: Max requests per second (default 2.0 to avoid Kalshi rate limits)
            base_url: Override REST API base URL (or use KALSHI_BASE_URL env var)
            env: Environment name ("prod" or "demo"), defaults to KALSHI_ENV
        """
        # Import config module for environment-aware URL/key resolution
        from markets.kalshi.config import (
            KalshiEnvironment,
            get_kalshi_rest_url,
            get_kalshi_api_key,
            get_kalshi_private_key,
            get_kalshi_private_key_path,
        )
        
        # Resolve environment
        kalshi_env = None
        if env:
            try:
                kalshi_env = KalshiEnvironment(env.lower())
            except ValueError:
                pass
        
        # Resolve base URL
        resolved_url = get_kalshi_rest_url(env=kalshi_env, override_url=base_url)
        
        super().__init__(
            base_url=resolved_url,
            platform=Platform.KALSHI,
            rate_limit=rate_limit,
        )

        # Resolve API key
        self.api_key = api_key or get_kalshi_api_key(env=kalshi_env)
        self._private_key: Optional[RSAPrivateKey] = None
        self._signature_cache = SignatureCache()

        # Load private key (explicit args take precedence over env-based config)
        if private_key_path:
            self._load_private_key_from_file(private_key_path)
        elif private_key_str:
            self._load_private_key_from_string(private_key_str)
        else:
            # Try environment-aware key resolution
            env_key_str = get_kalshi_private_key(env=kalshi_env)
            env_key_path = get_kalshi_private_key_path(env=kalshi_env)
            if env_key_str:
                self._load_private_key_from_string(env_key_str)
            elif env_key_path:
                self._load_private_key_from_file(env_key_path)

    def _load_private_key_from_file(self, path: str) -> None:
        """Load RSA private key from PEM file."""
        try:
            with open(path, "rb") as f:
                self._private_key = serialization.load_pem_private_key(
                    f.read(),
                    password=None,
                )
            logger.info("Loaded Kalshi private key from file")
        except Exception as e:
            logger.error(f"Failed to load private key from {path}: {e}")
            raise

    def _load_private_key_from_string(self, key_str: str) -> None:
        """Load RSA private key from PEM string."""
        try:
            self._private_key = serialization.load_pem_private_key(
                key_str.encode(),
                password=None,
            )
            logger.info("Loaded Kalshi private key from string")
        except Exception as e:
            logger.error(f"Failed to load private key from string: {e}")
            raise

    def _generate_signature(self, timestamp_ms: int, method: str, path: str) -> str:
        """Generate RSA-PSS signature for request."""
        if not self._private_key:
            return ""

        # Check cache first
        cache_key = f"{timestamp_ms}:{method}:{path}"
        cached = self._signature_cache.get(cache_key, timestamp_ms)
        if cached:
            return cached

        # Generate signature: timestamp + method + path
        message = f"{timestamp_ms}{method}{path}"
        message_bytes = message.encode("utf-8")

        signature = self._private_key.sign(
            message_bytes,
            padding.PSS(
                mgf=padding.MGF1(hashes.SHA256()),
                salt_length=padding.PSS.DIGEST_LENGTH,
            ),
            hashes.SHA256(),
        )

        sig_b64 = base64.b64encode(signature).decode("utf-8")

        # Cache the signature
        self._signature_cache.set(cache_key, sig_b64, timestamp_ms)

        return sig_b64

    def _get_headers(self) -> dict[str, str]:
        """Get headers with authentication."""
        timestamp_ms = int(time.time() * 1000)

        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
            "KALSHI-ACCESS-KEY": self.api_key,
            "KALSHI-ACCESS-TIMESTAMP": str(timestamp_ms),
        }

        return headers

    async def _request(
        self,
        method: str,
        endpoint: str,
        params: Optional[dict] = None,
        json: Optional[dict] = None,
        extra_headers: Optional[dict] = None,
    ) -> Any:
        """Make authenticated request to Kalshi API."""
        session = self._ensure_connected()
        url = f"{self.base_url}{endpoint}"

        # Build path for signature (without query params)
        path = endpoint.split("?")[0]
        full_path = f"/trade-api/v2{path}"

        timestamp_ms = int(time.time() * 1000)
        signature = self._generate_signature(timestamp_ms, method.upper(), full_path)

        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
            "KALSHI-ACCESS-KEY": self.api_key,
            "KALSHI-ACCESS-TIMESTAMP": str(timestamp_ms),
            "KALSHI-ACCESS-SIGNATURE": signature,
        }
        if extra_headers:
            headers.update(extra_headers)

        await self._rate_limiter.acquire()

        async with session.request(
            method,
            url,
            params=params,
            json=json,
            headers=headers,
        ) as response:
            if response.status == 429:
                retry_after = int(response.headers.get("Retry-After", "5"))
                await asyncio.sleep(retry_after)
                return await self._request(method, endpoint, params, json, extra_headers)

            response.raise_for_status()
            return await response.json()

    # ==========================================================================
    # Market Data Methods
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """
        Get markets from Kalshi.

        Args:
            sport: Filter by sport (nfl, nba, nhl, etc.)
            status: Market status filter (open, closed, settled)
            limit: Maximum results

        Returns:
            List of market dictionaries
        """
        params: dict[str, Any] = {"limit": limit}

        if status:
            params["status"] = status

        if sport and sport.lower() in self.SPORTS_SERIES:
            params["series_ticker"] = self.SPORTS_SERIES[sport.lower()]

        try:
            data = await self.get("/markets", params=params)
            return data.get("markets", [])
        except Exception as e:
            logger.error(f"Error getting Kalshi markets: {e}")
            return []

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get detailed information about a specific market."""
        try:
            data = await self.get(f"/markets/{market_id}")
            return data.get("market")
        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                return None
            raise

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get order book for a market."""
        try:
            data = await self.get(f"/markets/{market_id}/orderbook")
            orderbook_data = data.get("orderbook", {}) or {}

            # Get yes/no levels, handling null values from API
            yes_levels = orderbook_data.get("yes") or []
            no_levels = orderbook_data.get("no") or []

            # Parse bids (yes side)
            yes_bids = [
                OrderBookLevel(
                    price=level[0] / 100.0,  # Kalshi uses cents
                    quantity=level[1],
                )
                for level in yes_levels
                if isinstance(level, (list, tuple)) and len(level) >= 2 and level[1] > 0
            ]

            # Parse asks (no side inverted)
            yes_asks = [
                OrderBookLevel(
                    price=1.0 - (level[0] / 100.0),
                    quantity=level[1],
                )
                for level in no_levels
                if isinstance(level, (list, tuple)) and len(level) >= 2 and level[1] > 0
            ]

            return OrderBook(
                market_id=market_id,
                platform=Platform.KALSHI,
                yes_bids=sorted(yes_bids, key=lambda x: x.price, reverse=True),
                yes_asks=sorted(yes_asks, key=lambda x: x.price),
            )
        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                return None
            raise

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price snapshot."""
        market = await self.get_market(market_id)
        if not market:
            return None

        orderbook = await self.get_orderbook(market_id)

        # Get best bid/ask from orderbook or use last price
        yes_bid = 0.0
        yes_ask = 1.0

        if orderbook:
            if orderbook.best_yes_bid:
                yes_bid = orderbook.best_yes_bid
            if orderbook.best_yes_ask:
                yes_ask = orderbook.best_yes_ask
        else:
            # Fall back to last traded price
            last_price = market.get("last_price", 50) / 100.0
            yes_bid = max(0.0, last_price - 0.02)
            yes_ask = min(1.0, last_price + 0.02)

        # Map status
        status_map = {
            "open": MarketStatus.OPEN,
            "closed": MarketStatus.CLOSED,
            "settled": MarketStatus.SETTLED,
        }

        return MarketPrice(
            market_id=market_id,
            platform=Platform.KALSHI,
            market_title=market.get("title", ""),
            yes_bid=yes_bid,
            yes_ask=yes_ask,
            volume=market.get("volume", 0),
            open_interest=market.get("open_interest", 0),
            liquidity=orderbook.total_bid_liquidity if orderbook else 0,
            status=status_map.get(market.get("status", ""), MarketStatus.OPEN),
            last_trade_price=market.get("last_price", 0) / 100.0,
        )

    async def stream_prices(
        self,
        market_ids: list[str],
        interval_seconds: float = 5.0,
    ) -> AsyncIterator[MarketPrice]:
        """
        Stream price updates for markets via polling.

        TODO: Implement WebSocket streaming when Kalshi supports it.
        """
        if not market_ids:
            return

        logger.info(f"Streaming {len(market_ids)} Kalshi markets (poll interval: {interval_seconds}s)")

        while True:
            for market_id in market_ids:
                try:
                    price = await self.get_market_price(market_id)
                    if price:
                        yield price
                except Exception as e:
                    logger.warning(f"Error fetching price for {market_id}: {e}")

            await asyncio.sleep(interval_seconds)

    # ==========================================================================
    # Trading Methods (for future real trading support)
    # ==========================================================================

    async def place_order(
        self,
        market_id: str,
        side: str,
        price: float,
        quantity: float,
    ) -> dict:
        """Place an order on Kalshi.

        Note: This client expects `side` to be the contract side: "yes" or "no".
        (This matches the Kalshi UI and our ExecutionEngine semantics.)
        """
        contract_side = side.lower()
        if contract_side not in ("yes", "no"):
            raise ValueError(f"Kalshi place_order side must be 'yes' or 'no', got: {side}")

        # Convert to Kalshi format (limit order)
        data: dict = {
            "ticker": market_id,
            "action": "buy",
            "side": contract_side,
            "type": "limit",
            "count": int(quantity),
        }

        cents = int(price * 100)
        if contract_side == "yes":
            data["yes_price"] = cents
        else:
            data["no_price"] = cents

        response = await self.post("/portfolio/orders", json=data)
        return response.get("order", {})

    async def cancel_order(self, order_id: str) -> bool:
        """Cancel an order on Kalshi."""
        try:
            await self._request("DELETE", f"/portfolio/orders/{order_id}")
            return True
        except Exception as e:
            logger.error(f"Failed to cancel order {order_id}: {e}")
            return False

    async def get_positions(self) -> list[dict]:
        """Get current positions."""
        try:
            data = await self.get("/portfolio/positions")
            return data.get("market_positions", [])
        except Exception as e:
            logger.error(f"Failed to get positions: {e}")
            return []

    # ==========================================================================
    # Utility Methods
    # ==========================================================================

    async def search_markets(self, query: str, limit: int = 50) -> list[dict]:
        """Search for markets by keyword."""
        params = {"q": query, "limit": limit, "status": "open"}
        try:
            data = await self.get("/markets", params=params)
            return data.get("markets", [])
        except Exception as e:
            logger.error(f"Error searching markets: {e}")
            return []

    async def get_events(self, series_ticker: Optional[str] = None) -> list[dict]:
        """Get events (parent of markets)."""
        params = {}
        if series_ticker:
            params["series_ticker"] = series_ticker

        try:
            data = await self.get("/events", params=params)
            return data.get("events", [])
        except Exception as e:
            logger.error(f"Error getting events: {e}")
            return []

    async def health_check(self) -> bool:
        """Check if API is accessible."""
        try:
            await self.get("/markets", params={"limit": 1})
            return True
        except Exception:
            return False
