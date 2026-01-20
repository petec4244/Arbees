"""
Simplified Polymarket REST client for RPi deployment.

This is a standalone client with no dependencies on the main Arbees codebase.
Designed to run on Raspberry Pi with OpenVPN for geo-bypass.
"""

import asyncio
import json
import logging
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Optional

import aiohttp

logger = logging.getLogger(__name__)


@dataclass
class MarketPrice:
    """Market price snapshot - matches Arbees MarketPrice schema."""
    market_id: str
    platform: str = "polymarket"
    game_id: Optional[str] = None
    market_title: str = ""
    yes_bid: float = 0.0
    yes_ask: float = 1.0
    volume: float = 0.0
    liquidity: float = 0.0
    sport: Optional[str] = None
    timestamp: datetime = field(default_factory=datetime.utcnow)

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "market_id": self.market_id,
            "platform": self.platform,
            "game_id": self.game_id,
            "market_title": self.market_title,
            "yes_bid": self.yes_bid,
            "yes_ask": self.yes_ask,
            "volume": self.volume,
            "liquidity": self.liquidity,
            "sport": self.sport,
        }


class PolymarketRPiClient:
    """Simplified async Polymarket client for RPi."""

    CLOB_URL = "https://clob.polymarket.com"
    GAMMA_URL = "https://gamma-api.polymarket.com"

    SPORTS_TAGS = [
        "sports", "nfl", "nba", "nhl", "mlb",
        "soccer", "football", "basketball", "hockey", "baseball",
        "mma", "ufc", "tennis", "golf",
    ]

    def __init__(
        self,
        rate_limit: float = 10.0,
        timeout_seconds: float = 30.0,
    ):
        """
        Initialize the client.

        Args:
            rate_limit: Max requests per second
            timeout_seconds: Request timeout in seconds
        """
        self.rate_limit = rate_limit
        self.timeout = aiohttp.ClientTimeout(total=timeout_seconds)
        self._session: Optional[aiohttp.ClientSession] = None
        self._last_request_time = 0.0
        self._token_id_cache: dict[str, Optional[str]] = {}

    async def connect(self) -> None:
        """Create the aiohttp session."""
        if self._session is None:
            connector = aiohttp.TCPConnector(limit=50, limit_per_host=20)
            self._session = aiohttp.ClientSession(
                connector=connector,
                timeout=self.timeout,
            )
            logger.info("Polymarket client connected")

    async def disconnect(self) -> None:
        """Close the aiohttp session."""
        if self._session:
            await self._session.close()
            self._session = None
            logger.info("Polymarket client disconnected")

    def _ensure_connected(self) -> aiohttp.ClientSession:
        """Ensure session exists."""
        if self._session is None:
            raise RuntimeError("Client not connected. Call connect() first.")
        return self._session

    async def _rate_limit_wait(self) -> None:
        """Wait to respect rate limit."""
        if self.rate_limit <= 0:
            return

        now = asyncio.get_event_loop().time()
        min_interval = 1.0 / self.rate_limit
        elapsed = now - self._last_request_time

        if elapsed < min_interval:
            await asyncio.sleep(min_interval - elapsed)

        self._last_request_time = asyncio.get_event_loop().time()

    async def _gamma_request(
        self,
        method: str,
        endpoint: str,
        params: Optional[dict] = None,
    ) -> Any:
        """Make request to Gamma API."""
        session = self._ensure_connected()
        url = f"{self.GAMMA_URL}{endpoint}"

        await self._rate_limit_wait()

        headers = {"Accept": "application/json"}

        async with session.request(
            method, url, params=params, headers=headers
        ) as response:
            response.raise_for_status()
            return await response.json()

    async def _clob_request(
        self,
        method: str,
        endpoint: str,
        params: Optional[dict] = None,
    ) -> Any:
        """Make request to CLOB API."""
        session = self._ensure_connected()
        url = f"{self.CLOB_URL}{endpoint}"

        await self._rate_limit_wait()

        headers = {"Accept": "application/json"}

        async with session.request(
            method, url, params=params, headers=headers
        ) as response:
            response.raise_for_status()
            return await response.json()

    # ==========================================================================
    # Token ID Resolution
    # ==========================================================================

    def _is_valid_token_id(self, token_id: Optional[str]) -> bool:
        """Validate that a token_id looks legitimate."""
        if not token_id or not isinstance(token_id, str):
            return False
        if len(token_id) < 10:
            return False
        if token_id in ["[", "]", "{", "}", "null", "undefined", "None"]:
            return False
        if not token_id[0].isalnum():
            return False
        return True

    def _extract_yes_token_id(self, market: dict) -> Optional[str]:
        """Extract YES token_id from market data."""
        # Try tokens array
        tokens = market.get("tokens") or []
        for token in tokens:
            outcome = str(token.get("outcome", "")).lower()
            token_id = token.get("token_id") or token.get("id")
            if outcome == "yes" and self._is_valid_token_id(token_id):
                return str(token_id)

        # Try outcomes + token_ids arrays
        outcomes = market.get("outcomes")
        token_ids = market.get("token_ids")
        if isinstance(outcomes, list) and isinstance(token_ids, list):
            for idx, outcome in enumerate(outcomes):
                if str(outcome).lower() == "yes" and idx < len(token_ids):
                    tid = str(token_ids[idx])
                    if self._is_valid_token_id(tid):
                        return tid

        # Try direct fields
        direct = market.get("token_id") or market.get("yes_token_id")
        if self._is_valid_token_id(direct):
            return str(direct)

        # Try clobTokenIds
        clob_token_ids = market.get("clobTokenIds")
        if clob_token_ids:
            if isinstance(clob_token_ids, str):
                try:
                    clob_token_ids = json.loads(clob_token_ids)
                except json.JSONDecodeError:
                    pass

            if isinstance(clob_token_ids, list) and len(clob_token_ids) > 0:
                token_id = str(clob_token_ids[0])
                if self._is_valid_token_id(token_id):
                    return token_id

        return None

    async def resolve_yes_token_id(self, market: dict) -> Optional[str]:
        """Resolve YES token_id with caching and fallback strategies."""
        condition_id = market.get("condition_id") or market.get("id")
        if not condition_id:
            return None

        condition_id = str(condition_id)

        # Check cache
        if condition_id in self._token_id_cache:
            return self._token_id_cache[condition_id]

        # Try extracting from provided market data
        token_id = self._extract_yes_token_id(market)

        # Fallback: refresh from Gamma API
        if not token_id:
            try:
                refreshed = await self._gamma_request("GET", f"/markets/{condition_id}")
                if refreshed:
                    token_id = self._extract_yes_token_id(refreshed)
            except Exception as e:
                logger.debug(f"Gamma refresh failed for {condition_id}: {e}")

        # Cache result (even if None)
        self._token_id_cache[condition_id] = token_id
        return token_id

    # ==========================================================================
    # Market Data Methods
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
    ) -> list[dict]:
        """Get markets from Polymarket."""
        params: dict[str, Any] = {"limit": limit}

        if status == "open":
            params["active"] = "true"

        if sport:
            sport_lower = sport.lower()
            if sport_lower in self.SPORTS_TAGS:
                params["tag"] = sport_lower
            else:
                params["tag"] = "sports"

        try:
            data = await self._gamma_request("GET", "/markets", params=params)
            return data if isinstance(data, list) else []
        except Exception as e:
            logger.error(f"Error getting markets: {e}")
            return []

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get detailed information about a specific market."""
        try:
            return await self._gamma_request("GET", f"/markets/{market_id}")
        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                return None
            raise

    async def get_orderbook(self, token_id: str) -> Optional[dict]:
        """Get order book for a market by token_id."""
        try:
            return await self._clob_request("GET", "/book", params={"token_id": token_id})
        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                return None
            raise

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price snapshot."""
        market = await self.get_market(market_id)
        if not market:
            return None

        yes_bid = 0.0
        yes_ask = 1.0
        liquidity = 0.0

        # Try to get orderbook for better prices
        token_id = await self.resolve_yes_token_id(market)
        if token_id:
            try:
                orderbook = await self.get_orderbook(token_id)
                if orderbook:
                    bids = orderbook.get("bids", [])
                    asks = orderbook.get("asks", [])

                    if bids:
                        yes_bid = max(float(b.get("price", 0)) for b in bids)
                        liquidity = sum(
                            float(b.get("price", 0)) * float(b.get("size", 0))
                            for b in bids
                        )
                    if asks:
                        yes_ask = min(float(a.get("price", 1)) for a in asks)
            except Exception as e:
                logger.debug(f"Orderbook fetch failed for {market_id}: {e}")

        # Fall back to last traded price from market data if no orderbook
        if yes_bid == 0.0 and yes_ask == 1.0:
            outcomes = market.get("outcomePrices") or market.get("outcomes_prices") or []
            if outcomes and len(outcomes) > 0:
                try:
                    yes_price = float(outcomes[0]) if isinstance(outcomes[0], (int, float, str)) else 0.5
                    yes_bid = max(0.01, yes_price - 0.02)
                    yes_ask = min(0.99, yes_price + 0.02)
                except (ValueError, TypeError):
                    pass

        return MarketPrice(
            market_id=market_id,
            market_title=market.get("question", market.get("title", "")),
            yes_bid=yes_bid,
            yes_ask=yes_ask,
            volume=float(market.get("volume", 0) or 0),
            liquidity=liquidity,
        )

    async def get_sports_markets(self, limit: int = 100) -> list[dict]:
        """Get all sports-related markets."""
        all_markets = []
        seen_ids = set()

        for tag in ["sports", "nfl", "nba", "soccer", "mma"]:
            try:
                markets = await self.get_markets(sport=tag, limit=limit)
                for market in markets:
                    market_id = market.get("condition_id") or market.get("id")
                    if market_id and market_id not in seen_ids:
                        seen_ids.add(market_id)
                        all_markets.append(market)
            except Exception as e:
                logger.warning(f"Error fetching {tag} markets: {e}")

        return all_markets

    async def health_check(self) -> bool:
        """Check if Polymarket APIs are accessible."""
        try:
            await self._gamma_request("GET", "/markets", params={"limit": 1})
            return True
        except Exception as e:
            logger.error(f"Health check failed: {e}")
            return False
