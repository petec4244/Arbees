"""
Market title parsing to extract bet type, team, and betting line.

This enables intelligent market matching across platforms by parsing
natural language market titles into structured data.
"""

import re
from typing import Optional, Tuple

from arbees_shared.models.market_types import MarketType, BettingLine, ParsedMarket


class MarketParser:
    """
    Parse market titles to extract bet type, team, and line.
    
    Examples:
    - "Will the Lakers beat the Celtics?" → MONEYLINE, Lakers
    - "Will the Chiefs cover -7.5?" → SPREAD, Chiefs, -7.5
    - "Will total score exceed 220.5?" → TOTAL, 220.5
    """
    
    # Regex patterns for different market types
    MONEYLINE_PATTERNS = [
        r"(?:will|can)\s+(?:the\s+)?(\w+(?:\s+\w+)*)\s+(?:beat|defeat|win)",
        r"(\w+(?:\s+\w+)*)\s+to\s+win",
        r"(\w+(?:\s+\w+)*)\s+(?:wins|victory)",
    ]
    
    SPREAD_PATTERNS = [
        r"(?:will|can)\s+(?:the\s+)?(\w+(?:\s+\w+)*)\s+cover\s+([-+]?\d+\.?\d*)",
        r"(\w+(?:\s+\w+)*)\s+([-+]\d+\.?\d*)",
        r"(\w+(?:\s+\w+)*)\s+spread\s+([-+]?\d+\.?\d*)",
    ]
    
    TOTAL_PATTERNS = [
        r"(?:will\s+)?total\s+(?:score\s+)?(?:be\s+)?(?:over|exceed|above)\s+(\d+\.?\d*)",
        r"over\s+(\d+\.?\d*)\s+points",
        r"(?:will\s+)?(?:total\s+)?(?:points?\s+)?(?:be\s+)?over\s+(\d+\.?\d*)",
    ]
    
    PLAYER_PROP_PATTERNS = [
        r"(?:will\s+)?(\w+\s+\w+)\s+(?:score|have|get)\s+(?:over|more\s+than)\s+(\d+\.?\d*)\s+(\w+)",
        r"(\w+\s+\w+)\s+over\s+(\d+\.?\d*)\s+(\w+)",
    ]
    
    @classmethod
    def parse(cls, title: str, platform: str = "unknown") -> Optional[ParsedMarket]:
        """
        Parse a market title into structured data.
        
        Args:
            title: Market title/question
            platform: Platform name (for platform-specific parsing)
            
        Returns:
            ParsedMarket or None if parsing fails
        """
        title_lower = title.lower().strip()
        
        # Try each market type in order of specificity
        
        # 1. Player props (most specific)
        result = cls._parse_player_prop(title_lower)
        if result:
            return result
        
        # 2. Spreads
        result = cls._parse_spread(title_lower)
        if result:
            return result
        
        # 3. Totals
        result = cls._parse_total(title_lower)
        if result:
            return result
        
        # 4. Moneyline (least specific)
        result = cls._parse_moneyline(title_lower)
        if result:
            return result
        
        # Failed to parse
        return None
    
    @classmethod
    def _parse_moneyline(cls, title: str) -> Optional[ParsedMarket]:
        """Parse moneyline market."""
        for pattern in cls.MONEYLINE_PATTERNS:
            match = re.search(pattern, title, re.IGNORECASE)
            if match:
                team = match.group(1).strip()
                return ParsedMarket(
                    market_type=MarketType.MONEYLINE,
                    team=cls._normalize_team_name(team),
                )
        return None
    
    @classmethod
    def _parse_spread(cls, title: str) -> Optional[ParsedMarket]:
        """Parse spread market."""
        for pattern in cls.SPREAD_PATTERNS:
            match = re.search(pattern, title, re.IGNORECASE)
            if match:
                team = match.group(1).strip()
                line_value = float(match.group(2))
                return ParsedMarket(
                    market_type=MarketType.SPREAD,
                    team=cls._normalize_team_name(team),
                    line=BettingLine(value=line_value),
                )
        return None
    
    @classmethod
    def _parse_total(cls, title: str) -> Optional[ParsedMarket]:
        """Parse total (over/under) market."""
        for pattern in cls.TOTAL_PATTERNS:
            match = re.search(pattern, title, re.IGNORECASE)
            if match:
                line_value = float(match.group(1))
                return ParsedMarket(
                    market_type=MarketType.TOTAL,
                    line=BettingLine(value=line_value),
                )
        return None
    
    @classmethod
    def _parse_player_prop(cls, title: str) -> Optional[ParsedMarket]:
        """Parse player prop market."""
        for pattern in cls.PLAYER_PROP_PATTERNS:
            match = re.search(pattern, title, re.IGNORECASE)
            if match:
                player = match.group(1).strip()
                line_value = float(match.group(2))
                stat_type = match.group(3).strip()
                
                return ParsedMarket(
                    market_type=MarketType.PLAYER_PROP,
                    player=cls._normalize_player_name(player),
                    line=BettingLine(value=line_value),
                )
        return None
    
    @classmethod
    def _normalize_team_name(cls, team: str) -> str:
        """Normalize team name for comparison."""
        # Remove common prefixes
        team = re.sub(r"^(the\s+)", "", team, flags=re.IGNORECASE)
        
        # Title case
        team = team.title()
        
        return team.strip()
    
    @classmethod
    def _normalize_player_name(cls, player: str) -> str:
        """Normalize player name for comparison."""
        # Title case
        player = player.title()
        
        return player.strip()


# Convenience function
def parse_market(title: str, platform: str = "unknown") -> Optional[ParsedMarket]:
    """Parse a market title into structured data."""
    return MarketParser.parse(title, platform)


if __name__ == "__main__":
    # Test cases
    test_cases = [
        "Will the Lakers beat the Celtics?",
        "Lakers to win",
        "Will the Chiefs cover -7.5?",
        "Chiefs -7.5",
        "Will total score exceed 220.5?",
        "Over 220.5 points",
        "Will LeBron James score over 25.5 points?",
        "LeBron James over 25.5 points",
    ]
    
    print("Market Parser Test Cases:")
    print("=" * 80)
    
    for title in test_cases:
        parsed = parse_market(title)
        if parsed:
            print(f"\n✓ \"{title}\"")
            print(f"  Type: {parsed.market_type.value}")
            if parsed.team:
                print(f"  Team: {parsed.team}")
            if parsed.player:
                print(f"  Player: {parsed.player}")
            if parsed.line:
                print(f"  Line: {parsed.line}")
        else:
            print(f"\n✗ \"{title}\"")
            print(f"  Failed to parse")
    
    print("\n" + "=" * 80)
    
    # Test compatibility
    print("\nCompatibility Tests:")
    print("=" * 80)
    
    market1 = parse_market("Will the Lakers beat the Celtics?")
    market2 = parse_market("Lakers to win")
    market3 = parse_market("Will the Chiefs cover -7.5?")
    market4 = parse_market("Chiefs -7.5")
    market5 = parse_market("Chiefs -8.0")
    
    if market1 and market2:
        print(f"\nMoneyline Lakers (Kalshi) vs Moneyline Lakers (Poly): {market1.is_compatible_with(market2)}")
    
    if market3 and market4:
        print(f"Spread Chiefs -7.5 (Kalshi) vs Spread Chiefs -7.5 (Poly): {market3.is_compatible_with(market4)}")
    
    if market3 and market5:
        print(f"Spread Chiefs -7.5 (Kalshi) vs Spread Chiefs -8.0 (Poly): {market3.is_compatible_with(market5)}")
