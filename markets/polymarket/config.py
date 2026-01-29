"""
Polymarket API environment configuration.

Supports endpoint overrides via environment variables.
Unlike Kalshi, Polymarket does not have an official testnet/demo environment,
but we support URL overrides for flexibility and testing.
"""

import os
from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class PolymarketEndpoints:
    """Polymarket API endpoint configuration."""
    gamma_url: str
    clob_url: str
    ws_url: str


# Default Polymarket endpoints
DEFAULT_ENDPOINTS = PolymarketEndpoints(
    gamma_url="https://gamma-api.polymarket.com",
    clob_url="https://clob.polymarket.com",
    ws_url="wss://ws-subscriptions-clob.polymarket.com/ws/market",
)


def _get_env_float(name: str, default: float) -> float:
    value = os.environ.get(name)
    if value is None:
        return default
    try:
        return float(value)
    except ValueError:
        return default


def get_polymarket_gamma_url(override_url: Optional[str] = None) -> str:
    """
    Get Polymarket Gamma API URL.
    
    Priority:
    1. override_url parameter
    2. POLYMARKET_GAMMA_URL environment variable
    3. Default URL
    """
    if override_url:
        return override_url
    
    return os.environ.get("POLYMARKET_GAMMA_URL", DEFAULT_ENDPOINTS.gamma_url)


def get_polymarket_clob_url(override_url: Optional[str] = None) -> str:
    """
    Get Polymarket CLOB API URL.
    
    Priority:
    1. override_url parameter
    2. POLYMARKET_CLOB_URL environment variable
    3. Default URL
    """
    if override_url:
        return override_url
    
    return os.environ.get("POLYMARKET_CLOB_URL", DEFAULT_ENDPOINTS.clob_url)


def get_polymarket_ws_url(override_url: Optional[str] = None) -> str:
    """
    Get Polymarket WebSocket URL.
    
    Priority:
    1. override_url parameter
    2. POLYMARKET_WS_URL environment variable
    3. Default URL
    """
    if override_url:
        return override_url
    
    return os.environ.get("POLYMARKET_WS_URL", DEFAULT_ENDPOINTS.ws_url)


def get_polymarket_ws_reconnect_base() -> float:
    """Base delay (seconds) for WS exponential backoff."""
    return _get_env_float("POLYMARKET_WS_RECONNECT_BASE", 0.25)


def get_polymarket_ws_reconnect_max() -> float:
    """Max delay (seconds) for WS exponential backoff."""
    return _get_env_float("POLYMARKET_WS_RECONNECT_MAX", 30.0)


def get_polymarket_ws_reconnect_jitter() -> float:
    """Jitter percentage (0-1) for WS exponential backoff."""
    return _get_env_float("POLYMARKET_WS_RECONNECT_JITTER", 0.2)


def get_polymarket_api_key() -> Optional[str]:
    """Get Polymarket API key from environment."""
    return os.environ.get("POLYMARKET_API_KEY")


def get_polymarket_proxy_url() -> Optional[str]:
    """Get Polymarket proxy URL from environment (for geo-restrictions)."""
    return os.environ.get("POLYMARKET_PROXY_URL")
