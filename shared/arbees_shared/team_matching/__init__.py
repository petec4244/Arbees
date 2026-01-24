"""
Unified team matching client.

ALL services must use this instead of implementing their own matching.
This ensures consistency across Kalshi discovery, Polymarket discovery,
and trade execution.

The matching is performed by the Rust-based market-discovery-rust service
which provides a Redis RPC interface for team name validation.
"""

from .client import TeamMatchingClient, TeamMatchResult

__all__ = ["TeamMatchingClient", "TeamMatchResult"]
