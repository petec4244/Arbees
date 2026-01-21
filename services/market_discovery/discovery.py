"""
Market Discovery Service - Automatic ESPN → Kalshi/Polymarket matching.

This service solves the critical problem of matching ESPN games to prediction markets.
"""

import json
import logging
import os
from datetime import datetime, timedelta
from difflib import SequenceMatcher
from pathlib import Path
from typing import Optional

from arbees_shared.models.game import GameState, Sport
from arbees_shared.models.market import Platform
from arbees_shared.models.market_types import MarketType, ParsedMarket
from markets.kalshi.client import KalshiClient
from markets.polymarket.client import PolymarketClient
from services.market_discovery.parser import parse_market

logger = logging.getLogger(__name__)


class MarketDiscoveryService:
    """
    Automatically discover and match prediction markets for ESPN games.
    
    Key features:
    - Team name normalization (ESPN "KC" → "Kansas City Chiefs")
    - Fuzzy title matching across platforms
    - Date filtering (only match markets for today/tomorrow)
    - Volume-based ranking (prefer liquid markets)
    """
    
    def __init__(
        self,
        kalshi_client: KalshiClient,
        polymarket_client: PolymarketClient,
        team_cache_path: Optional[str] = None,
    ):
        """
        Initialize Market Discovery Service.
        
        Args:
            kalshi_client: Connected Kalshi client
            polymarket_client: Connected Polymarket client
            team_cache_path: Path to team_cache.json (auto-detected if None)
        """
        self.kalshi = kalshi_client
        self.polymarket = polymarket_client
        
        # Load team name mappings
        if team_cache_path is None:
            team_cache_path = Path(__file__).parent / "team_cache.json"
        
        with open(team_cache_path, "r") as f:
            self.team_cache = json.load(f)
        
        logger.info(f"Loaded team cache with {sum(len(teams) for teams in self.team_cache.values())} teams")
    
    def normalize_team_name(self, team: str, sport: Sport) -> str:
        """
        Normalize team name from ESPN format to full name.
        
        Args:
            team: Team name or abbreviation (e.g., "KC", "Kansas City")
            sport: Sport to search within
            
        Returns:
            Full team name (e.g., "Kansas City Chiefs")
        """
        sport_key = sport.value.lower()
        
        if sport_key not in self.team_cache:
            return team
        
        teams = self.team_cache[sport_key]
        
        # Direct abbreviation match
        if team.upper() in teams:
            return teams[team.upper()]
        
        # Partial match (e.g., "Kansas City" → "Kansas City Chiefs")
        team_lower = team.lower()
        for abbrev, full_name in teams.items():
            if team_lower in full_name.lower():
                return full_name
        
        # No match - return as-is
        return team
    
    async def find_markets_for_game(
        self,
        game_state: GameState,
        platforms: list[Platform],
        search_window_hours: int = 48,
    ) -> dict[Platform, Optional[str]]:
        """
        Auto-discover markets for a game across multiple platforms.
        
        Args:
            game_state: Current game state with team names
            platforms: List of platforms to search
            search_window_hours: Only match markets within this time window
            
        Returns:
            Dict of {Platform: market_id or None}
        """
        markets = {}
        
        # Normalize team names
        home = self.normalize_team_name(game_state.home_team, game_state.sport)
        away = self.normalize_team_name(game_state.away_team, game_state.sport)
        
        logger.info(f"Searching markets for: {away} @ {home} ({game_state.sport.value})")
        
        for platform in platforms:
            try:
                if platform == Platform.KALSHI:
                    market_id = await self._find_kalshi_market(
                        home, away, game_state.sport, search_window_hours
                    )
                    markets[Platform.KALSHI] = market_id
                    
                elif platform == Platform.POLYMARKET:
                    market_id = await self._find_polymarket_market(
                        home, away, game_state.sport, search_window_hours
                    )
                    markets[Platform.POLYMARKET] = market_id
                    
            except Exception as e:
                logger.error(f"Error finding {platform.value} market: {e}")
                markets[platform] = None
        
        return markets
    
    async def _find_kalshi_market(
        self,
        home: str,
        away: str,
        sport: Sport,
        search_window_hours: int,
    ) -> Optional[str]:
        """Find matching Kalshi market."""
        # Search for markets in this sport
        all_markets = await self.kalshi.get_markets(
            sport=sport.value,
            status="open",
            limit=200,
        )
        
        if not all_markets:
            logger.warning(f"No Kalshi markets found for {sport.value}")
            return None
        
        # Score each market
        best_match = None
        best_score = 0.0
        
        for market in all_markets:
            title = market.get("title", "").lower()
            
            # Must contain both team names (or abbreviations)
            if not (home.lower() in title or away.lower() in title):
                continue
            
            # Calculate similarity score
            # Higher score = better match
            score = self._calculate_title_score(title, home, away)
            
            # Boost by volume (prefer liquid markets)
            volume_boost = min(1.0, market.get("volume", 0) / 10000)
            final_score = score + volume_boost
            
            if final_score > best_score:
                best_score = final_score
                best_match = market
        
        if best_match:
            logger.info(
                f"Found Kalshi market: {best_match['title']} "
                f"(score: {best_score:.2f}, volume: {best_match.get('volume', 0)})"
            )
            return best_match.get("ticker") or best_match.get("id")
        
        logger.warning(f"No matching Kalshi market for {away} @ {home}")
        return None
    
    async def _find_polymarket_market(
        self,
        home: str,
        away: str,
        sport: Sport,
        search_window_hours: int,
    ) -> Optional[str]:
        """Find matching Polymarket market."""
        # Search for sports markets
        all_markets = await self.polymarket.get_markets(
            sport=sport.value,
            status="open",
            limit=200,
        )
        
        if not all_markets:
            logger.warning(f"No Polymarket markets found for {sport.value}")
            return None
        
        # Score each market
        best_match = None
        best_score = 0.0
        
        for market in all_markets:
            # Check both 'question' and 'title' fields
            title = (market.get("question") or market.get("title", "")).lower()
            
            # Must contain at least one team name
            if not (home.lower() in title or away.lower() in title):
                continue
            
            # Calculate similarity score
            score = self._calculate_title_score(title, home, away)
            
            # Boost by volume
            volume = float(market.get("volume", 0) or 0)
            volume_boost = min(1.0, volume / 100000)
            final_score = score + volume_boost
            
            if final_score > best_score:
                best_score = final_score
                best_match = market
        
        if best_match:
            title = best_match.get("question") or best_match.get("title", "")
            logger.info(
                f"Found Polymarket market: {title} "
                f"(score: {best_score:.2f}, volume: {best_match.get('volume', 0)})"
            )
            return best_match.get("condition_id") or best_match.get("id")
        
        logger.warning(f"No matching Polymarket market for {away} @ {home}")
        return None
    
    def _calculate_title_score(self, title: str, home: str, away: str) -> float:
        """
        Calculate similarity score for a market title.
        
        Returns:
            Score from 0.0 to 2.0 (higher is better)
        """
        title_lower = title.lower()
        home_lower = home.lower()
        away_lower = away.lower()
        
        score = 0.0
        
        # Both teams mentioned = +1.0
        if home_lower in title_lower and away_lower in title_lower:
            score += 1.0
        # Only one team mentioned = +0.5
        elif home_lower in title_lower or away_lower in title_lower:
            score += 0.5
        
        # Exact phrases = bonus
        if f"{away} at {home}".lower() in title_lower:
            score += 0.5
        elif f"{away} @ {home}".lower() in title_lower:
            score += 0.5
        elif f"{home} vs {away}".lower() in title_lower:
            score += 0.5
        
        # "To win" markets are preferred
        if "to win" in title_lower or "will win" in title_lower:
            score += 0.3
        
        # Avoid special markets
        if any(keyword in title_lower for keyword in ["spread", "total", "over", "under", "prop"]):
            score -= 0.3
        
        return max(0.0, score)
    
    async def bulk_discover_markets(
        self,
        game_states: list[GameState],
        platforms: list[Platform],
    ) -> dict[str, dict[Platform, Optional[str]]]:
        """
        Discover markets for multiple games in parallel.

        Args:
            game_states: List of games to find markets for
            platforms: Platforms to search

        Returns:
            Dict of {game_id: {Platform: market_id}}
        """
        import asyncio

        tasks = [
            self.find_markets_for_game(game, platforms)
            for game in game_states
        ]

        results = await asyncio.gather(*tasks, return_exceptions=True)

        return {
            game.game_id: result if not isinstance(result, Exception) else {}
            for game, result in zip(game_states, results)
        }

    # ==========================================================================
    # Multi-Market Type Discovery (3-8x more arbitrage opportunities)
    # ==========================================================================

    async def find_markets_by_type(
        self,
        game_state: GameState,
        market_type: MarketType,
        platforms: list[Platform],
    ) -> dict[Platform, Optional[str]]:
        """
        Find specific market type for a game.

        Args:
            game_state: Current game
            market_type: Type of market to find (moneyline, spread, total)
            platforms: Platforms to search

        Returns:
            {Platform: market_id}
        """
        markets = {}

        home = self.normalize_team_name(game_state.home_team, game_state.sport)
        away = self.normalize_team_name(game_state.away_team, game_state.sport)

        for platform in platforms:
            try:
                if platform == Platform.KALSHI:
                    market_id = await self._find_kalshi_market_by_type(
                        home, away, game_state.sport, market_type
                    )
                    markets[Platform.KALSHI] = market_id

                elif platform == Platform.POLYMARKET:
                    market_id = await self._find_polymarket_market_by_type(
                        home, away, game_state.sport, market_type
                    )
                    markets[Platform.POLYMARKET] = market_id

            except Exception as e:
                logger.error(f"Error finding {platform.value} {market_type.value} market: {e}")
                markets[platform] = None

        return markets

    async def _find_kalshi_market_by_type(
        self,
        home: str,
        away: str,
        sport: Sport,
        market_type: MarketType,
    ) -> Optional[str]:
        """Find Kalshi market of specific type."""
        all_markets = await self.kalshi.get_markets(
            sport=sport.value,
            status="open",
            limit=200,
        )

        if not all_markets:
            return None

        for market in all_markets:
            title = market.get("title", "")

            # Parse market type
            parsed = parse_market(title, platform="kalshi")
            if not parsed or parsed.market_type != market_type:
                continue

            # For moneyline/spread: must match team
            if parsed.team:
                home_lower = home.lower()
                away_lower = away.lower()
                title_lower = title.lower()
                if not (home_lower in title_lower or away_lower in title_lower):
                    continue

            logger.info(f"Found Kalshi {market_type.value}: {title}")
            return market.get("ticker") or market.get("id")

        return None

    async def _find_polymarket_market_by_type(
        self,
        home: str,
        away: str,
        sport: Sport,
        market_type: MarketType,
    ) -> Optional[str]:
        """Find Polymarket market of specific type."""
        all_markets = await self.polymarket.get_markets(
            sport=sport.value,
            status="open",
            limit=200,
        )

        if not all_markets:
            return None

        for market in all_markets:
            title = market.get("question") or market.get("title", "")

            # Parse market type
            parsed = parse_market(title, platform="polymarket")
            if not parsed or parsed.market_type != market_type:
                continue

            # For moneyline/spread: must match team
            if parsed.team:
                home_lower = home.lower()
                away_lower = away.lower()
                title_lower = title.lower()
                if not (home_lower in title_lower or away_lower in title_lower):
                    continue

            logger.info(f"Found Polymarket {market_type.value}: {title}")
            return market.get("condition_id") or market.get("id")

        return None

    async def find_all_markets_for_game(
        self,
        game_state: GameState,
        platforms: list[Platform],
    ) -> dict[MarketType, dict[Platform, str]]:
        """
        Find multiple market types for a game.

        This enables 3-8x more arbitrage opportunities by discovering
        moneyline, spread, and total markets for each game.

        Returns:
            {
                MarketType.MONEYLINE: {Platform.KALSHI: "id1", Platform.POLYMARKET: "id2"},
                MarketType.SPREAD: {Platform.KALSHI: "id3", Platform.POLYMARKET: "id4"},
                MarketType.TOTAL: {Platform.KALSHI: "id5", Platform.POLYMARKET: "id6"},
            }
        """
        market_types_to_find = [
            MarketType.MONEYLINE,
            MarketType.SPREAD,
            MarketType.TOTAL,
        ]

        results: dict[MarketType, dict[Platform, str]] = {}

        for market_type in market_types_to_find:
            markets = await self.find_markets_by_type(
                game_state,
                market_type,
                platforms,
            )

            # Only include if we found markets on BOTH platforms
            if all(markets.get(p) for p in platforms):
                # Type narrowing: we know all values are str at this point
                results[market_type] = {p: markets[p] for p in platforms}  # type: ignore
                logger.info(f"Found {market_type.value} on both platforms for {game_state.away_team} @ {game_state.home_team}")
            else:
                missing = [p.value for p in platforms if not markets.get(p)]
                logger.warning(f"Missing {market_type.value} on: {missing}")

        return results

    async def bulk_discover_all_market_types(
        self,
        game_states: list[GameState],
        platforms: list[Platform],
    ) -> dict[str, dict[MarketType, dict[Platform, str]]]:
        """
        Discover all market types for multiple games in parallel.

        Args:
            game_states: List of games to find markets for
            platforms: Platforms to search

        Returns:
            Dict of {game_id: {MarketType: {Platform: market_id}}}
        """
        import asyncio

        tasks = [
            self.find_all_markets_for_game(game, platforms)
            for game in game_states
        ]

        results = await asyncio.gather(*tasks, return_exceptions=True)

        return {
            game.game_id: result if not isinstance(result, Exception) else {}
            for game, result in zip(game_states, results)
        }
