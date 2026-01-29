"""
Kalshi API environment configuration.

Supports production, demo (testnet), and custom environments.
Environment can be selected via:
- KALSHI_ENV environment variable (prod|demo)
- Explicit base_url/ws_url parameters
- KALSHI_BASE_URL / KALSHI_WS_URL overrides
"""

import os
from dataclasses import dataclass
from enum import Enum
from typing import Optional


class KalshiEnvironment(str, Enum):
    """Kalshi API environments."""
    PROD = "prod"
    DEMO = "demo"  # Kalshi's testnet/demo environment


@dataclass(frozen=True)
class KalshiEndpoints:
    """Kalshi API endpoint configuration."""
    rest_url: str
    ws_url: str
    name: str


# Official Kalshi endpoints
# Production: https://docs.kalshi.com/
# Demo: https://docs.kalshi.com/ (demo environment section)
KALSHI_ENDPOINTS: dict[KalshiEnvironment, KalshiEndpoints] = {
    KalshiEnvironment.PROD: KalshiEndpoints(
        rest_url="https://api.elections.kalshi.com/trade-api/v2",
        ws_url="wss://api.elections.kalshi.com/trade-api/ws/v2",
        name="Production",
    ),
    KalshiEnvironment.DEMO: KalshiEndpoints(
        rest_url="https://demo-api.kalshi.co/trade-api/v2",
        ws_url="wss://demo-api.kalshi.co/trade-api/ws/v2",
        name="Demo/Testnet",
    ),
}


def get_kalshi_environment() -> KalshiEnvironment:
    """Get Kalshi environment from KALSHI_ENV env var."""
    env_str = os.environ.get("KALSHI_ENV", "prod").lower()
    try:
        return KalshiEnvironment(env_str)
    except ValueError:
        return KalshiEnvironment.PROD


def get_kalshi_rest_url(
    env: Optional[KalshiEnvironment] = None,
    override_url: Optional[str] = None,
) -> str:
    """
    Get Kalshi REST API base URL.
    
    Priority:
    1. override_url parameter
    2. KALSHI_BASE_URL environment variable
    3. Environment-specific default (from KALSHI_ENV)
    """
    if override_url:
        return override_url
    
    env_override = os.environ.get("KALSHI_BASE_URL")
    if env_override:
        return env_override
    
    env = env or get_kalshi_environment()
    return KALSHI_ENDPOINTS[env].rest_url


def get_kalshi_ws_url(
    env: Optional[KalshiEnvironment] = None,
    override_url: Optional[str] = None,
) -> str:
    """
    Get Kalshi WebSocket URL.
    
    Priority:
    1. override_url parameter
    2. KALSHI_WS_URL environment variable
    3. Environment-specific default (from KALSHI_ENV)
    """
    if override_url:
        return override_url
    
    env_override = os.environ.get("KALSHI_WS_URL")
    if env_override:
        return env_override
    
    env = env or get_kalshi_environment()
    return KALSHI_ENDPOINTS[env].ws_url


def get_kalshi_api_key(env: Optional[KalshiEnvironment] = None) -> str:
    """
    Get Kalshi API key for the specified environment.
    
    Environment-specific keys:
    - KALSHI_API_KEY (production, or fallback)
    - KALSHI_DEMO_API_KEY (demo environment)
    """
    env = env or get_kalshi_environment()
    
    if env == KalshiEnvironment.DEMO:
        demo_key = os.environ.get("KALSHI_DEMO_API_KEY")
        if demo_key:
            return demo_key
    
    return os.environ.get("KALSHI_API_KEY", "")


def get_kalshi_private_key(env: Optional[KalshiEnvironment] = None) -> Optional[str]:
    """
    Get Kalshi private key for the specified environment.
    
    Environment-specific keys:
    - KALSHI_PRIVATE_KEY (production, or fallback)
    - KALSHI_DEMO_PRIVATE_KEY (demo environment)
    
    Returns the key as a string, or None if not set.
    """
    env = env or get_kalshi_environment()
    
    if env == KalshiEnvironment.DEMO:
        demo_key = os.environ.get("KALSHI_DEMO_PRIVATE_KEY")
        if demo_key:
            return demo_key
    
    return os.environ.get("KALSHI_PRIVATE_KEY")


def get_kalshi_private_key_path(env: Optional[KalshiEnvironment] = None) -> Optional[str]:
    """
    Get Kalshi private key file path for the specified environment.

    Environment-specific paths:
    - KALSHI_PRIVATE_KEY_PATH (production, or fallback)
    - KALSHI_DEMO_PRIVATE_KEY_PATH (demo environment)
    """
    env = env or get_kalshi_environment()

    if env == KalshiEnvironment.DEMO:
        demo_path = os.environ.get("KALSHI_DEMO_PRIVATE_KEY_PATH")
        if demo_path:
            return demo_path

    return os.environ.get("KALSHI_PRIVATE_KEY_PATH")


def _get_env_float(name: str, default: float) -> float:
    """Get float from environment variable."""
    value = os.environ.get(name)
    if value is None:
        return default
    try:
        return float(value)
    except ValueError:
        return default


def get_kalshi_ws_ping_interval() -> float:
    """Ping interval (seconds) for WebSocket heartbeat."""
    return _get_env_float("KALSHI_WS_PING_INTERVAL", 30.0)


def get_kalshi_ws_ping_timeout() -> float:
    """Ping timeout (seconds) - close connection if pong not received."""
    return _get_env_float("KALSHI_WS_PING_TIMEOUT", 10.0)


def get_kalshi_ws_reconnect_base() -> float:
    """Base delay (seconds) for WS exponential backoff."""
    return _get_env_float("KALSHI_WS_RECONNECT_BASE", 5.0)


def get_kalshi_ws_reconnect_max() -> float:
    """Max delay (seconds) for WS exponential backoff."""
    return _get_env_float("KALSHI_WS_RECONNECT_MAX", 120.0)


def get_kalshi_ws_stale_timeout() -> float:
    """Stale timeout (seconds) - reconnect if no messages received in this time."""
    return _get_env_float("KALSHI_WS_STALE_TIMEOUT", 60.0)
