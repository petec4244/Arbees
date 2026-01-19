"""Base async market client with connection pooling, rate limiting, and retry logic."""

import asyncio
import time
from abc import ABC, abstractmethod
from collections import deque
from datetime import datetime
from typing import Any, AsyncIterator, Optional

import aiohttp
from tenacity import (
    AsyncRetrying,
    retry_if_exception_type,
    stop_after_attempt,
    wait_exponential,
)

from arbees_shared.models.market import MarketPrice, OrderBook, Platform


class RateLimiter:
    """Token bucket rate limiter for API calls."""

    def __init__(self, calls_per_second: float = 10.0, burst: int = 20):
        self.rate = calls_per_second
        self.burst = burst
        self.tokens = float(burst)
        self.last_update = time.monotonic()
        self._lock = asyncio.Lock()

    async def acquire(self) -> None:
        """Acquire a token, waiting if necessary."""
        async with self._lock:
            now = time.monotonic()
            elapsed = now - self.last_update
            self.tokens = min(self.burst, self.tokens + elapsed * self.rate)
            self.last_update = now

            if self.tokens < 1:
                wait_time = (1 - self.tokens) / self.rate
                await asyncio.sleep(wait_time)
                self.tokens = 0
            else:
                self.tokens -= 1


class BaseMarketClient(ABC):
    """Abstract base class for prediction market clients."""

    def __init__(
        self,
        base_url: str,
        platform: Platform,
        rate_limit: float = 10.0,
        timeout: float = 30.0,
        max_retries: int = 3,
    ):
        self.base_url = base_url.rstrip("/")
        self.platform = platform
        self.timeout = aiohttp.ClientTimeout(total=timeout)
        self.max_retries = max_retries
        self._session: Optional[aiohttp.ClientSession] = None
        self._rate_limiter = RateLimiter(calls_per_second=rate_limit)
        self._request_times: deque = deque(maxlen=100)

    async def __aenter__(self) -> "BaseMarketClient":
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await self.disconnect()

    async def connect(self) -> None:
        """Create the aiohttp session."""
        if self._session is None:
            connector = aiohttp.TCPConnector(
                limit=100,
                limit_per_host=30,
                keepalive_timeout=30,
            )
            self._session = aiohttp.ClientSession(
                connector=connector,
                timeout=self.timeout,
            )

    async def disconnect(self) -> None:
        """Close the aiohttp session."""
        if self._session:
            await self._session.close()
            self._session = None

    def _ensure_connected(self) -> aiohttp.ClientSession:
        """Ensure we have a session."""
        if self._session is None:
            raise RuntimeError("Client not connected. Call connect() first.")
        return self._session

    def _get_headers(self) -> dict[str, str]:
        """Get headers for requests. Override in subclasses for auth."""
        return {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }

    async def _request(
        self,
        method: str,
        endpoint: str,
        params: Optional[dict] = None,
        json: Optional[dict] = None,
        extra_headers: Optional[dict] = None,
    ) -> Any:
        """Make an HTTP request with rate limiting and retry."""
        session = self._ensure_connected()
        url = f"{self.base_url}{endpoint}"

        headers = self._get_headers()
        if extra_headers:
            headers.update(extra_headers)

        await self._rate_limiter.acquire()

        start_time = time.monotonic()

        async for attempt in AsyncRetrying(
            stop=stop_after_attempt(self.max_retries),
            wait=wait_exponential(multiplier=1, min=1, max=10),
            retry=retry_if_exception_type((aiohttp.ClientError, asyncio.TimeoutError)),
            reraise=True,
        ):
            with attempt:
                async with session.request(
                    method,
                    url,
                    params=params,
                    json=json,
                    headers=headers,
                ) as response:
                    elapsed = time.monotonic() - start_time
                    self._request_times.append(elapsed)

                    if response.status == 429:
                        # Rate limited - wait and retry
                        retry_after = int(response.headers.get("Retry-After", "5"))
                        await asyncio.sleep(retry_after)
                        raise aiohttp.ClientError("Rate limited")

                    response.raise_for_status()

                    if response.content_type == "application/json":
                        return await response.json()
                    return await response.text()

    async def get(
        self,
        endpoint: str,
        params: Optional[dict] = None,
        **kwargs
    ) -> Any:
        """Make a GET request."""
        return await self._request("GET", endpoint, params=params, **kwargs)

    async def post(
        self,
        endpoint: str,
        json: Optional[dict] = None,
        **kwargs
    ) -> Any:
        """Make a POST request."""
        return await self._request("POST", endpoint, json=json, **kwargs)

    @property
    def avg_latency_ms(self) -> float:
        """Average request latency in milliseconds."""
        if not self._request_times:
            return 0.0
        return (sum(self._request_times) / len(self._request_times)) * 1000

    # ==========================================================================
    # Abstract methods - must be implemented by subclasses
    # ==========================================================================

    @abstractmethod
    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """Get list of available markets."""
        ...

    @abstractmethod
    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get details for a specific market."""
        ...

    @abstractmethod
    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get order book for a market."""
        ...

    @abstractmethod
    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price snapshot."""
        ...

    @abstractmethod
    async def stream_prices(
        self,
        market_ids: list[str],
        interval_seconds: float = 5.0,
    ) -> AsyncIterator[MarketPrice]:
        """Stream price updates for markets."""
        ...

    # ==========================================================================
    # Optional methods for execution (override if supported)
    # ==========================================================================

    async def place_order(
        self,
        market_id: str,
        side: str,  # "buy" or "sell"
        price: float,
        quantity: float,
    ) -> dict:
        """Place an order. Override in subclasses that support execution."""
        raise NotImplementedError("Order placement not supported")

    async def cancel_order(self, order_id: str) -> bool:
        """Cancel an order. Override in subclasses that support execution."""
        raise NotImplementedError("Order cancellation not supported")

    async def get_positions(self) -> list[dict]:
        """Get open positions. Override in subclasses that support execution."""
        raise NotImplementedError("Position tracking not supported")


class MockMarketClient(BaseMarketClient):
    """Mock market client for testing."""

    def __init__(self, platform: Platform = Platform.PAPER):
        super().__init__("http://localhost", platform)
        self._markets: dict[str, dict] = {}
        self._prices: dict[str, MarketPrice] = {}

    async def connect(self) -> None:
        """No-op for mock."""
        pass

    async def disconnect(self) -> None:
        """No-op for mock."""
        pass

    def add_mock_market(self, market_id: str, data: dict) -> None:
        """Add a mock market for testing."""
        self._markets[market_id] = data

    def set_mock_price(self, market_id: str, price: MarketPrice) -> None:
        """Set a mock price for testing."""
        self._prices[market_id] = price

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        return list(self._markets.values())[:limit]

    async def get_market(self, market_id: str) -> Optional[dict]:
        return self._markets.get(market_id)

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        price = self._prices.get(market_id)
        if price is None:
            return None
        return OrderBook(
            market_id=market_id,
            platform=self.platform,
            yes_bids=[],
            yes_asks=[],
        )

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        return self._prices.get(market_id)

    async def stream_prices(
        self,
        market_ids: list[str],
        interval_seconds: float = 5.0,
    ) -> AsyncIterator[MarketPrice]:
        while True:
            for market_id in market_ids:
                price = self._prices.get(market_id)
                if price:
                    yield price
            await asyncio.sleep(interval_seconds)
