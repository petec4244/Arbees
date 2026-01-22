"""
Polymarket CLOB API client.

Improvements over original:
- Fully async with aiohttp
- Designed as standalone microservice for EU deployment
- Robust token ID resolution with caching
- No VPN hacks - proper cloud deployment
"""

import asyncio
import json
import logging
import os
from datetime import datetime
from typing import Any, AsyncIterator, Optional

import aiohttp

from arbees_shared.models.market import (
    MarketPrice,
    MarketStatus,
    OrderBook,
    OrderBookLevel,
    Platform,
)
from markets.base import BaseMarketClient

logger = logging.getLogger(__name__)


class PolymarketClient(BaseMarketClient):
    """Async Polymarket CLOB API client."""

    CLOB_URL = "https://clob.polymarket.com"
    GAMMA_URL = "https://gamma-api.polymarket.com"

    # Sports-related tags
    SPORTS_TAGS = [
        "sports", "nfl", "nba", "nhl", "mlb",
        "ncaaf", "ncaab", "college football", "college basketball",
        "soccer", "football", "basketball", "hockey", "baseball",
        "mma", "ufc", "tennis", "golf",
    ]

    # Gamma tag slug -> numeric tag_id (from https://gamma-api.polymarket.com/tags).
    # IMPORTANT: Gamma /markets does NOT reliably filter with `tag=<slug>`, but it does with `tag_id=<int>`.
    TAG_ID_BY_SLUG: dict[str, int] = {
        "sports": 1,
        "basketball": 28,
        "football": 10,
        "hockey": 100088,
        "nba": 745,
        "nfl": 450,
        "nhl": 899,
        "ncaab": 101952,
    }

    def __init__(
        self,
        api_key: Optional[str] = None,
        proxy_url: Optional[str] = None,
        use_eu_proxy: bool = False,
        rate_limit: float = 10.0,
    ):
        """
        Initialize Polymarket client.

        Args:
            api_key: Optional API key for authenticated endpoints
            proxy_url: Optional proxy URL for routing
            use_eu_proxy: If True, use EU proxy service (for regulatory compliance)
            rate_limit: Max requests per second
        """
        super().__init__(
            base_url=self.CLOB_URL,
            platform=Platform.POLYMARKET,
            rate_limit=rate_limit,
        )

        self.api_key = api_key or os.environ.get("POLYMARKET_API_KEY")
        self.proxy_url = proxy_url
        self._token_id_cache: dict[str, Optional[str]] = {}

        # EU proxy configuration for regulatory compliance
        # EU proxy configuration for regulatory compliance
        if use_eu_proxy or os.environ.get("POLYMARKET_PROXY_URL"):
            self.proxy_url = os.environ.get(
                "POLYMARKET_PROXY_URL",
                proxy_url  # Fallback to init arg
            )
            if self.proxy_url:
                logger.info(f"Polymarket client using proxy: {self.proxy_url}")

    async def connect(self) -> None:
        """Create the aiohttp session with optional proxy."""
        if self._session is None:
            connector = aiohttp.TCPConnector(
                limit=100,
                limit_per_host=30,
                ssl=False if self.proxy_url else True
            )
            self._session = aiohttp.ClientSession(
                connector=connector,
                timeout=self.timeout,
            )

    def _get_headers(self) -> dict[str, str]:
        """Get headers for requests."""
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    async def _gamma_request(
        self,
        method: str,
        endpoint: str,
        params: Optional[dict] = None,
    ) -> Any:
        """Make request to Gamma API."""
        session = self._ensure_connected()
        url = f"{self.GAMMA_URL}{endpoint}"

        await self._rate_limiter.acquire()

        async with session.request(
            method,
            url,
            params=params,
            headers=self._get_headers(),
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

        await self._rate_limiter.acquire()

        async with session.request(
            method,
            url,
            params=params,
            headers=self._get_headers(),
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

        # Try clobTokenIds (new field from CLOB API)
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

        # Fallback: try CLOB API
        if not token_id:
            try:
                clob_market = await self._clob_request("GET", f"/markets/{condition_id}")
                if clob_market:
                    token_id = self._extract_yes_token_id(clob_market)
            except Exception as e:
                logger.debug(f"CLOB lookup failed for {condition_id}: {e}")

        # Cache result (even if None)
        self._token_id_cache[condition_id] = token_id
        return token_id

    def resolve_outcome_token_id(self, market: dict, candidates: str | list[str]) -> Optional[str]:
        """
        Resolve the token_id for a specific outcome name or aliases.
        Useful for 'Team A vs Team B' markets where we want the token for one specific team.
        Supports substring matching (alias in outcome).
        """
        if not candidates:
            return None
            
        targets = [candidates] if isinstance(candidates, str) else candidates
        targets = [t.lower().strip() for t in targets if t]
        
        # Helper to check match
        def is_match(outcome_text: str) -> bool:
            o_norm = str(outcome_text).lower().strip()
            # Exact match first
            if o_norm in targets:
                return True
            # Substring key match (e.g. "notre dame" in "notre dame fighting irish")
            for t in targets:
                if t in o_norm:
                    return True
            return False

        # 1. Check outcomes/clobTokenIds (Parallel Arrays)
        outcomes = market.get("outcomes")
        clob_ids = market.get("clobTokenIds")
        
        # Parse clobTokenIds if string
        if isinstance(clob_ids, str):
            try:
                clob_ids = json.loads(clob_ids)
            except json.JSONDecodeError:
                pass
                
        if isinstance(outcomes, list) and isinstance(clob_ids, list):
            if len(outcomes) == len(clob_ids):
                for idx, outcome in enumerate(outcomes):
                    if is_match(outcome):
                        raw_id = clob_ids[idx]
                        if self._is_valid_token_id(str(raw_id)):
                            return str(raw_id)

        # 2. Check tokens array (array of dicts)
        tokens = market.get("tokens")
        if isinstance(tokens, list):
            for t in tokens:
                t_outcome = t.get("outcome", "")
                if is_match(t_outcome):
                    tid = t.get("token_id") or t.get("id")
                    if self._is_valid_token_id(str(tid)):
                        return str(tid)
                        
        return None

    # ==========================================================================
    # Market Data Methods
    # ==========================================================================

    async def get_markets(
        self,
        sport: Optional[str] = None,
        status: str = "open",
        limit: int = 100,
        offset: int = 0,
    ) -> list[dict]:
        """Get markets from Polymarket."""
        # Default sort by volume to get most relevant markets first
        params: dict[str, Any] = {
            "limit": limit, 
            "offset": offset,
            "order": "volume",
            "ascending": "false"
        }

        if status == "open":
            params["active"] = "true"
            params["closed"] = "false" # Explicitly exclude closed

        # Map sport to tag
        if sport:
            sport_lower = sport.lower()
            # Prefer tag_id filtering (correct Gamma behavior)
            if sport_lower in self.TAG_ID_BY_SLUG:
                params["tag_id"] = self.TAG_ID_BY_SLUG[sport_lower]
            else:
                # Fall back to broad Sports tag_id to avoid pulling non-sports markets.
                params["tag_id"] = self.TAG_ID_BY_SLUG["sports"]

        try:
            data = await self._gamma_request("GET", "/markets", params=params)
            return data if isinstance(data, list) else []
        except Exception as e:
            logger.error(f"Error getting Polymarket markets: {e}")
            return []

    async def get_market(self, market_id: str) -> Optional[dict]:
        """Get detailed information about a specific market."""
        try:
            return await self._gamma_request("GET", f"/markets/{market_id}")
        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                return None
            raise

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Get order book for a market (by condition_id or token_id)."""
        # First, resolve to token_id if given condition_id
        token_id = market_id
        if len(market_id) < 30:
            # Likely a condition_id, need to resolve
            market = await self.get_market(market_id)
            if market:
                resolved = await self.resolve_yes_token_id(market)
                if resolved:
                    token_id = resolved
                else:
                    return None
            else:
                return None

        try:
            data = await self._clob_request("GET", "/book", params={"token_id": token_id})

            # Parse bids
            yes_bids = []
            for level in data.get("bids", []):
                price = float(level.get("price", 0))
                size = float(level.get("size", 0))
                if size > 0:
                    yes_bids.append(OrderBookLevel(price=price, quantity=size))

            # Parse asks
            yes_asks = []
            for level in data.get("asks", []):
                price = float(level.get("price", 0))
                size = float(level.get("size", 0))
                if size > 0:
                    yes_asks.append(OrderBookLevel(price=price, quantity=size))

            return OrderBook(
                market_id=market_id,
                platform=Platform.POLYMARKET,
                yes_bids=sorted(yes_bids, key=lambda x: x.price, reverse=True),
                yes_asks=sorted(yes_asks, key=lambda x: x.price),
            )

        except aiohttp.ClientResponseError as e:
            if e.status == 404:
                # AMM markets don't have CLOB orderbooks
                logger.debug(f"No CLOB orderbook for {market_id} (AMM market)")
                return None
            raise

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Get current market price snapshot."""
        market = await self.get_market(market_id)
        if not market:
            return None

        # Try to get orderbook for better prices
        orderbook = await self.get_orderbook(market_id)

        yes_bid = 0.0
        yes_ask = 1.0
        liquidity = 0.0

        if orderbook:
            if orderbook.best_yes_bid:
                yes_bid = orderbook.best_yes_bid
            if orderbook.best_yes_ask:
                yes_ask = orderbook.best_yes_ask
            liquidity = orderbook.total_bid_liquidity
        else:
            # Fall back to last traded price from market data
            # Polymarket uses different price formats
            outcomes = market.get("outcomePrices") or market.get("outcomes_prices") or []
            if outcomes and len(outcomes) > 0:
                try:
                    yes_price = float(outcomes[0]) if isinstance(outcomes[0], (int, float, str)) else 0.5
                    yes_bid = max(0.0, yes_price - 0.02)
                    yes_ask = min(1.0, yes_price + 0.02)
                except (ValueError, TypeError):
                    pass

        # Determine status
        status = MarketStatus.OPEN
        if market.get("closed"):
            status = MarketStatus.CLOSED
        elif market.get("resolved"):
            status = MarketStatus.SETTLED

        return MarketPrice(
            market_id=market_id,
            platform=Platform.POLYMARKET,
            game_id=market.get("game_id"),
            market_title=market.get("question", market.get("title", "")),
            yes_bid=yes_bid,
            yes_ask=yes_ask,
            volume=float(market.get("volume", 0) or 0),
            liquidity=liquidity,
            status=status,
        )

    async def stream_prices(
        self,
        market_ids: list[str],
        interval_seconds: float = 5.0,
    ) -> AsyncIterator[MarketPrice]:
        """Stream price updates for markets via polling."""
        if not market_ids:
            return

        logger.info(f"Streaming {len(market_ids)} Polymarket markets")

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
    # Sports-Specific Methods
    # ==========================================================================

    async def get_sports_markets(self, limit: int = 100) -> list[dict]:
        """Get all sports-related markets."""
        all_markets = []
        seen_ids = set()

        # Fetch from multiple sports tags
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

    async def search_markets(
        self,
        query: str,
        limit: int = 50,
        sport: Optional[str] = None
    ) -> list[dict]:
        """Search for markets by keyword, optionally filtered by sport."""
        try:
            # Gamma API doesn't have search, so we fetch markets and filter in memory.
            # Using sport filter significantly improves discovery probability
            # by fetching the relevant category bucket instead of global top 1000.
            
            all_markets = []
            MAX_FETCH = 5000  # Safety limit
            BATCH_SIZE = 500   # Max often 1000, keep safe
            
            fetch_sport = sport if sport else None
            
            # Map sport to all relevant tags
            tags_to_fetch = []
            if fetch_sport:
                sport_lower = fetch_sport.lower()
                tags_to_fetch.append(sport_lower)
                
                # Add broader categories
                if sport_lower in ["nba", "ncaab", "college basketball"]:
                    tags_to_fetch.append("basketball")
                elif sport_lower in ["nfl", "ncaaf", "college football"]:
                    tags_to_fetch.append("football")
                elif sport_lower in ["nhl"]:
                    tags_to_fetch.append("hockey")
                elif sport_lower in ["mlb"]:
                    tags_to_fetch.append("baseball")
                elif sport_lower in ["ufc"]:
                    tags_to_fetch.append("mma")
                
                # Deduplicate
                tags_to_fetch = list(dict.fromkeys(tags_to_fetch))
            else:
                # Fallback to get_sports_markets logic if no sport provided
                pass

            all_markets = []
            
            if tags_to_fetch:
                for tag in tags_to_fetch:
                    offset = 0
                    tag_markets = []
                    while True:
                        # We pass the tag directly to get_markets by using it as 'sport' 
                        # (since get_markets maps sport->tag if it's in SPORTS_TAGS)
                        # Ensure 'basketball' etc are in SPORTS_TAGS
                        batch = await self.get_markets(sport=tag, limit=BATCH_SIZE, offset=offset)
                        if not batch:
                            break
                        
                        tag_markets.extend(batch)
                        offset += len(batch)
                        
                        if len(batch) < BATCH_SIZE or len(tag_markets) >= MAX_FETCH:
                            break
                    
                    logger.debug(f"DEBUG: Fetched {len(tag_markets)} raw markets for tag {tag}")
                    all_markets.extend(tag_markets)
                
                # Deduplicate markets by ID
                seen_ids = set()
                unique_markets = []
                for m in all_markets:
                    mid = m.get("condition_id") or m.get("id")
                    if mid and mid not in seen_ids:
                        seen_ids.add(mid)
                        unique_markets.append(m)
                
                markets = unique_markets
                logger.debug(f"DEBUG: Total unique markets fetched: {len(markets)}")
            else:
                markets = [] # Fallback handled in else block below if I kept it structure
                # But to preserve logic flow:
                pass
            
            if not tags_to_fetch:
                 # Default to fetching valid sports markets if no sport specified
                markets = await self.get_sports_markets(limit=1000)
                logger.debug(f"DEBUG: Fetched {len(markets)} raw sports markets")

            query_lower = query.lower()
            return [
                m for m in markets
                if query_lower in (m.get("question", "") + m.get("title", "")).lower()
            ][:limit]
        except Exception as e:
            logger.error(f"Error searching markets: {e}")
            return []

    async def get_trades(self, market_id: str, limit: int = 100) -> list[dict]:
        """Get recent trades for a market."""
        try:
            # Resolve token_id
            market = await self.get_market(market_id)
            if not market:
                return []

            token_id = await self.resolve_yes_token_id(market)
            if not token_id:
                return []

            data = await self._clob_request(
                "GET",
                "/trades",
                params={"market": token_id, "limit": limit}
            )
            return data if isinstance(data, list) else []
        except Exception as e:
            logger.error(f"Error fetching trades for {market_id}: {e}")
            return []

    async def health_check(self) -> bool:
        """Check if APIs are accessible."""
        try:
            await self._gamma_request("GET", "/markets", params={"limit": 1})
            return True
        except Exception:
            return False


class EUPolymarketProxy:
    """
    EU deployment proxy for Polymarket access.

    Designed to run in eu-central-1 as a standalone microservice
    that forwards requests to Polymarket APIs.
    """

    def __init__(self, upstream_client: PolymarketClient):
        self.client = upstream_client

    async def get_markets(self, **kwargs) -> list[dict]:
        """Forward get_markets request."""
        return await self.client.get_markets(**kwargs)

    async def get_market_price(self, market_id: str) -> Optional[MarketPrice]:
        """Forward get_market_price request."""
        return await self.client.get_market_price(market_id)

    async def get_orderbook(self, market_id: str) -> Optional[OrderBook]:
        """Forward get_orderbook request."""
        return await self.client.get_orderbook(market_id)
