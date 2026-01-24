"""
Team matching client - connects to Rust team matching service via Redis RPC.

This is the ONLY way to match team names in Arbees.
Do NOT implement your own matching logic.
"""

import asyncio
import logging
import os
import time
import uuid
from dataclasses import dataclass
from typing import Optional

import redis.asyncio as redis

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class TeamMatchResult:
    """Result of team matching."""

    is_match: bool
    confidence: float  # 0.0 to 1.0
    method: str  # Confidence level: None, Low, Medium, High, Exact
    reason: str

    def __repr__(self) -> str:
        return (
            f"TeamMatchResult(match={self.is_match}, "
            f"confidence={self.confidence:.2f}, "
            f"method='{self.method}')"
        )


class TeamMatchingClient:
    """
    Client for Rust-based team matching service.

    This is the ONLY way to match team names in Arbees.
    Do NOT implement your own matching logic.

    Usage:
        client = TeamMatchingClient()
        await client.connect()

        result = await client.match_teams(
            target_team="Boston Celtics",
            candidate_team="Celtics",
            sport="nba"
        )

        if result and result.is_match and result.confidence >= 0.7:
            # Teams match with high confidence
            logger.info(f"Match found: {result}")

    Performance:
        - ~1-5ms per match (including Redis roundtrip)
        - Much faster with cache hits
        - Consistent across all services

    Failure behavior (fail-closed):
        - On timeout: returns None (callers should reject/skip)
        - On service unavailable: returns None
        - Callers are expected to handle None appropriately
    """

    REQUEST_CHANNEL = "team:match:request"
    RESPONSE_PATTERN = "team:match:response:*"

    def __init__(
        self,
        redis_url: Optional[str] = None,
        cache_ttl_seconds: float = 300.0,  # 5 minutes
        cache_max_size: int = 1000,
    ):
        """
        Initialize team matching client.

        Args:
            redis_url: Redis connection URL (default: from REDIS_URL env var)
            cache_ttl_seconds: How long to cache match results (default: 5 min)
            cache_max_size: Maximum number of cached results
        """
        self.redis_url = redis_url or os.environ.get("REDIS_URL", "redis://redis:6379")
        self.redis: Optional[redis.Redis] = None
        self.pubsub: Optional[redis.client.PubSub] = None
        self._response_futures: dict[str, asyncio.Future] = {}
        self._listen_task: Optional[asyncio.Task] = None
        self._connected = False

        # Simple LRU-ish cache: (sport, target_norm, candidate_norm) -> (result, timestamp)
        self._cache: dict[tuple[str, str, str], tuple[TeamMatchResult, float]] = {}
        self._cache_ttl = cache_ttl_seconds
        self._cache_max_size = cache_max_size

    async def connect(self) -> None:
        """Connect to Redis and start listening for responses."""
        if self._connected:
            return

        self.redis = redis.from_url(self.redis_url, decode_responses=True)
        await self.redis.ping()

        # Subscribe to response channel pattern
        self.pubsub = self.redis.pubsub()
        await self.pubsub.psubscribe(self.RESPONSE_PATTERN)

        # Start listening for responses
        self._listen_task = asyncio.create_task(self._listen_responses())
        self._connected = True

        logger.info("TeamMatchingClient connected to Rust matching service")

    async def disconnect(self) -> None:
        """Disconnect from Redis."""
        if not self._connected:
            return

        self._connected = False

        if self._listen_task:
            self._listen_task.cancel()
            try:
                await self._listen_task
            except asyncio.CancelledError:
                pass

        if self.pubsub:
            await self.pubsub.punsubscribe()
            await self.pubsub.close()

        if self.redis:
            await self.redis.close()

        logger.info("TeamMatchingClient disconnected")

    async def _listen_responses(self) -> None:
        """Listen for match responses from Rust service."""
        try:
            async for message in self.pubsub.listen():
                if message["type"] != "pmessage":
                    continue

                try:
                    import json

                    data = json.loads(message["data"])
                    request_id = data.get("request_id")

                    if request_id in self._response_futures:
                        result = TeamMatchResult(
                            is_match=data["is_match"],
                            confidence=data["confidence"],
                            method=data["method"],
                            reason=data["reason"],
                        )
                        future = self._response_futures.get(request_id)
                        if future and not future.done():
                            future.set_result(result)
                except Exception as e:
                    logger.error(f"Error processing team match response: {e}")
        except asyncio.CancelledError:
            pass
        except Exception as e:
            logger.error(f"Team matching listener error: {e}")

    def _normalize(self, s: str) -> str:
        """Normalize string for cache key."""
        return s.lower().strip()

    def _cache_key(
        self, target_team: str, candidate_team: str, sport: str
    ) -> tuple[str, str, str]:
        """Generate cache key."""
        return (
            self._normalize(sport),
            self._normalize(target_team),
            self._normalize(candidate_team),
        )

    def _get_cached(
        self, target_team: str, candidate_team: str, sport: str
    ) -> Optional[TeamMatchResult]:
        """Get cached result if valid."""
        key = self._cache_key(target_team, candidate_team, sport)
        if key in self._cache:
            result, timestamp = self._cache[key]
            if time.time() - timestamp < self._cache_ttl:
                return result
            # Expired, remove it
            del self._cache[key]
        return None

    def _set_cached(
        self, target_team: str, candidate_team: str, sport: str, result: TeamMatchResult
    ) -> None:
        """Cache a result."""
        # Simple eviction: remove oldest entries if over limit
        if len(self._cache) >= self._cache_max_size:
            # Remove ~10% of oldest entries
            items = sorted(self._cache.items(), key=lambda x: x[1][1])
            for key, _ in items[: self._cache_max_size // 10]:
                del self._cache[key]

        key = self._cache_key(target_team, candidate_team, sport)
        self._cache[key] = (result, time.time())

    async def match_teams(
        self,
        target_team: str,
        candidate_team: str,
        sport: str,
        timeout: float = 2.0,
    ) -> Optional[TeamMatchResult]:
        """
        Match two team names with confidence scoring.

        Args:
            target_team: The team we're looking for (from signal/game)
            candidate_team: The team to check against (from market price or text)
            sport: Sport code (nba, nfl, nhl, mlb, ncaab, ncaaf, soccer, etc.)
            timeout: Max seconds to wait for response (default: 2.0s)

        Returns:
            TeamMatchResult with confidence score, or None if timeout/error.

            On timeout or service unavailable, returns None.
            Callers should treat None as "cannot validate" and fail closed
            (reject entries, skip exits).

        Example:
            result = await client.match_teams(
                target_team="Boston Celtics",
                candidate_team="Celtics",
                sport="nba"
            )
            if result and result.is_match and result.confidence >= 0.7:
                # High confidence match
                ...
            elif result is None:
                # Service unavailable - fail closed
                logger.warning("Team matching unavailable")
                return None
        """
        if not self._connected:
            raise RuntimeError("Not connected - call connect() first")

        if not target_team or not candidate_team:
            return TeamMatchResult(
                is_match=False,
                confidence=0.0,
                method="empty_input",
                reason="Target or candidate team is empty",
            )

        # Check cache first
        cached = self._get_cached(target_team, candidate_team, sport)
        if cached is not None:
            return cached

        # Generate unique request ID
        request_id = str(uuid.uuid4())

        # Create future for response
        future: asyncio.Future[TeamMatchResult] = asyncio.Future()
        self._response_futures[request_id] = future

        try:
            import json

            # Publish request
            request = {
                "request_id": request_id,
                "target_team": target_team,
                "candidate_team": candidate_team,
                "sport": sport.lower(),
            }
            await self.redis.publish(self.REQUEST_CHANNEL, json.dumps(request))

            # Wait for response
            result = await asyncio.wait_for(future, timeout=timeout)

            # Cache the result
            self._set_cached(target_team, candidate_team, sport, result)

            return result

        except asyncio.TimeoutError:
            logger.warning(
                f"Team match timeout: '{target_team}' vs '{candidate_team}' "
                f"(sport: {sport}, timeout: {timeout}s)"
            )
            return None
        except Exception as e:
            logger.error(f"Team match error: {e}")
            return None
        finally:
            # Cleanup
            self._response_futures.pop(request_id, None)

    async def match_teams_batch(
        self,
        matches: list[tuple[str, str, str]],  # [(target, candidate, sport), ...]
        timeout: float = 5.0,
    ) -> list[Optional[TeamMatchResult]]:
        """
        Match multiple team pairs concurrently.

        Args:
            matches: List of (target_team, candidate_team, sport) tuples
            timeout: Total timeout for all matches

        Returns:
            List of results in same order as input. None for timeouts.
        """
        tasks = [
            self.match_teams(target, candidate, sport, timeout=timeout)
            for target, candidate, sport in matches
        ]
        return await asyncio.gather(*tasks)

    def clear_cache(self) -> None:
        """Clear the result cache."""
        self._cache.clear()

    @property
    def cache_size(self) -> int:
        """Current number of cached entries."""
        return len(self._cache)
