"""
Unit tests for TeamMatchingClient.

Tests the client's caching, response correlation, and fail-closed behavior.
"""

import asyncio
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from arbees_shared.team_matching import TeamMatchingClient, TeamMatchResult


class TestTeamMatchResult:
    """Tests for TeamMatchResult dataclass."""

    def test_result_creation(self) -> None:
        """Test creating a TeamMatchResult."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Alias match",
        )
        assert result.is_match is True
        assert result.confidence == 0.9
        assert result.method == "High"
        assert result.reason == "Alias match"

    def test_result_repr(self) -> None:
        """Test string representation."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.85,
            method="High",
            reason="Test",
        )
        repr_str = repr(result)
        assert "match=True" in repr_str
        assert "confidence=0.85" in repr_str
        assert "method='High'" in repr_str

    def test_result_immutable(self) -> None:
        """Test that result is immutable (frozen dataclass)."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Test",
        )
        with pytest.raises(AttributeError):
            result.is_match = False  # type: ignore


class TestTeamMatchingClientCaching:
    """Tests for client caching behavior."""

    @pytest.fixture
    def client(self) -> TeamMatchingClient:
        """Create a client with short cache TTL for testing."""
        return TeamMatchingClient(
            redis_url="redis://localhost:6379",
            cache_ttl_seconds=1.0,
            cache_max_size=10,
        )

    def test_cache_key_normalization(self, client: TeamMatchingClient) -> None:
        """Test that cache keys are normalized."""
        key1 = client._cache_key("Boston Celtics", "Celtics", "NBA")
        key2 = client._cache_key("boston celtics", "celtics", "nba")
        key3 = client._cache_key("  BOSTON CELTICS  ", "  CELTICS  ", "  NBA  ")
        assert key1 == key2
        assert key2 == key3

    def test_set_and_get_cached(self, client: TeamMatchingClient) -> None:
        """Test setting and getting cached results."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Test",
        )

        # Cache miss initially
        assert client._get_cached("Celtics", "Boston Celtics", "nba") is None

        # Set cache
        client._set_cached("Celtics", "Boston Celtics", "nba", result)

        # Cache hit
        cached = client._get_cached("Celtics", "Boston Celtics", "nba")
        assert cached is not None
        assert cached.is_match is True
        assert cached.confidence == 0.9

    def test_cache_expiry(self, client: TeamMatchingClient) -> None:
        """Test that cached results expire."""
        import time

        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Test",
        )

        client._set_cached("Celtics", "Boston Celtics", "nba", result)

        # Should be in cache
        assert client._get_cached("Celtics", "Boston Celtics", "nba") is not None

        # Wait for TTL to expire (1 second + buffer)
        time.sleep(1.5)

        # Should be expired
        assert client._get_cached("Celtics", "Boston Celtics", "nba") is None

    def test_cache_eviction(self, client: TeamMatchingClient) -> None:
        """Test that cache evicts old entries when full."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Test",
        )

        # Fill cache beyond max size
        for i in range(15):
            client._set_cached(f"Team{i}", f"Candidate{i}", "nba", result)

        # Should have evicted some entries
        assert client.cache_size <= client._cache_max_size

    def test_clear_cache(self, client: TeamMatchingClient) -> None:
        """Test clearing the cache."""
        result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Test",
        )

        client._set_cached("Celtics", "Boston Celtics", "nba", result)
        assert client.cache_size > 0

        client.clear_cache()
        assert client.cache_size == 0


class TestTeamMatchingClientValidation:
    """Tests for input validation."""

    @pytest.fixture
    def connected_client(self) -> TeamMatchingClient:
        """Create a mock-connected client."""
        client = TeamMatchingClient()
        client._connected = True
        client.redis = AsyncMock()
        return client

    @pytest.mark.asyncio
    async def test_empty_target_team(self, connected_client: TeamMatchingClient) -> None:
        """Test that empty target team returns no-match result."""
        result = await connected_client.match_teams("", "Celtics", "nba")
        assert result is not None
        assert result.is_match is False
        assert result.method == "empty_input"

    @pytest.mark.asyncio
    async def test_empty_candidate_team(self, connected_client: TeamMatchingClient) -> None:
        """Test that empty candidate team returns no-match result."""
        result = await connected_client.match_teams("Celtics", "", "nba")
        assert result is not None
        assert result.is_match is False
        assert result.method == "empty_input"

    @pytest.mark.asyncio
    async def test_not_connected_raises(self) -> None:
        """Test that calling match_teams without connecting raises."""
        client = TeamMatchingClient()
        with pytest.raises(RuntimeError, match="Not connected"):
            await client.match_teams("Celtics", "Boston Celtics", "nba")


class TestTeamMatchingClientRPC:
    """Tests for RPC request/response flow."""

    @pytest.mark.asyncio
    async def test_timeout_returns_none(self) -> None:
        """Test that timeout returns None (fail-closed)."""
        client = TeamMatchingClient()
        client._connected = True
        client.redis = AsyncMock()
        client._response_futures = {}

        # Don't set up any response, so it will timeout
        result = await client.match_teams(
            "Celtics", "Boston Celtics", "nba", timeout=0.1
        )

        # Should return None (fail-closed)
        assert result is None

    @pytest.mark.asyncio
    async def test_request_format(self) -> None:
        """Test that request is published in correct format."""
        import json

        client = TeamMatchingClient()
        client._connected = True
        client.redis = AsyncMock()
        client._response_futures = {}

        # Start match but let it timeout
        try:
            await asyncio.wait_for(
                client.match_teams("Boston Celtics", "Celtics", "nba"),
                timeout=0.1,
            )
        except asyncio.TimeoutError:
            pass

        # Check that publish was called
        client.redis.publish.assert_called_once()
        call_args = client.redis.publish.call_args

        # Verify channel
        assert call_args[0][0] == "team:match:request"

        # Verify payload structure
        payload = json.loads(call_args[0][1])
        assert "request_id" in payload
        assert payload["target_team"] == "Boston Celtics"
        assert payload["candidate_team"] == "Celtics"
        assert payload["sport"] == "nba"

    @pytest.mark.asyncio
    async def test_cached_result_skips_rpc(self) -> None:
        """Test that cached results don't make RPC calls."""
        client = TeamMatchingClient()
        client._connected = True
        client.redis = AsyncMock()

        # Pre-populate cache
        cached_result = TeamMatchResult(
            is_match=True,
            confidence=0.9,
            method="High",
            reason="Cached",
        )
        client._set_cached("Celtics", "Boston Celtics", "nba", cached_result)

        # Request should hit cache
        result = await client.match_teams("Celtics", "Boston Celtics", "nba")

        # Should return cached result without calling Redis
        assert result is not None
        assert result.is_match is True
        assert result.reason == "Cached"
        client.redis.publish.assert_not_called()


class TestTeamMatchingClientIntegration:
    """Integration tests (require Redis + market-discovery-rust running).

    These tests are skipped by default. Run with:
        pytest -v --run-integration
    """

    @pytest.fixture
    def integration_client(self) -> TeamMatchingClient:
        """Create client for integration tests."""
        import os

        redis_url = os.environ.get("REDIS_URL", "redis://localhost:6379")
        return TeamMatchingClient(redis_url=redis_url)

    @pytest.mark.skip(reason="Requires Redis + market-discovery-rust running")
    @pytest.mark.asyncio
    async def test_real_match_celtics(
        self, integration_client: TeamMatchingClient
    ) -> None:
        """Test real RPC match for Celtics."""
        await integration_client.connect()
        try:
            result = await integration_client.match_teams(
                "Boston Celtics", "Celtics", "nba", timeout=5.0
            )
            assert result is not None
            assert result.is_match is True
            assert result.confidence >= 0.7
        finally:
            await integration_client.disconnect()

    @pytest.mark.skip(reason="Requires Redis + market-discovery-rust running")
    @pytest.mark.asyncio
    async def test_real_no_match_different_teams(
        self, integration_client: TeamMatchingClient
    ) -> None:
        """Test real RPC for different teams (should not match)."""
        await integration_client.connect()
        try:
            result = await integration_client.match_teams(
                "Boston Celtics", "Los Angeles Lakers", "nba", timeout=5.0
            )
            assert result is not None
            assert result.is_match is False
        finally:
            await integration_client.disconnect()

    @pytest.mark.skip(reason="Requires Redis + market-discovery-rust running")
    @pytest.mark.asyncio
    async def test_real_batch_match(
        self, integration_client: TeamMatchingClient
    ) -> None:
        """Test batch matching."""
        await integration_client.connect()
        try:
            matches = [
                ("Boston Celtics", "Celtics", "nba"),
                ("Los Angeles Lakers", "Lakers", "nba"),
                ("Philadelphia Flyers", "Flyers", "nhl"),
            ]
            results = await integration_client.match_teams_batch(matches, timeout=10.0)

            assert len(results) == 3
            for result in results:
                assert result is not None
                assert result.is_match is True
        finally:
            await integration_client.disconnect()
