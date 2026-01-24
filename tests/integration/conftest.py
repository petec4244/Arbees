"""
Shared pytest fixtures for integration tests.

These tests require:
- TimescaleDB running (docker-compose up timescaledb)
- Redis running (docker-compose up redis)
- DATABASE_URL environment variable set
"""

import os
import pytest


def pytest_configure(config):
    """Add custom markers."""
    config.addinivalue_line(
        "markers", "integration: mark test as integration test (requires DB/Redis)"
    )


@pytest.fixture(scope="session")
def event_loop():
    """Create an event loop for the test session."""
    import asyncio
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="session")
def database_url() -> str:
    """Get database URL from environment."""
    url = os.environ.get("DATABASE_URL")
    if not url:
        pytest.skip("DATABASE_URL not set")
    return url
