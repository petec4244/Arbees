"""
Market type classification for bet matching.

This enables proper arbitrage detection across platforms by:
- Categorizing markets by bet type (moneyline, spread, total, etc.)
- Extracting betting lines (spreads, totals)
- Matching only compatible markets
"""

from enum import Enum
from typing import Optional
from pydantic import BaseModel


class MarketType(str, Enum):
    """
    Types of sports betting markets.
    
    Only markets of the same type can be compared for arbitrage.
    """
    MONEYLINE = "moneyline"  # Team to win (no spread)
    SPREAD = "spread"  # Team to cover point spread
    TOTAL = "total"  # Over/under total points
    PLAYER_PROP = "player_prop"  # Player performance (points, rebounds, etc.)
    FIRST_BASKET = "first_basket"  # First basket scorer (NBA)
    FIRST_TD = "first_touchdown"  # First touchdown scorer (NFL)
    QUARTER_WINNER = "quarter_winner"  # Winner of specific quarter/period
    HALF_WINNER = "half_winner"  # Winner of first/second half
    EXACT_SCORE = "exact_score"  # Exact final score
    SERIES_WINNER = "series_winner"  # Winner of playoff series
    SEASON_WIN_TOTAL = "season_total"  # Season win totals


class BettingLine(BaseModel):
    """
    Represents a betting line (spread or total).
    
    Examples:
    - Spread: -7.5 (team favored by 7.5 points)
    - Total: 220.5 (over/under 220.5 points)
    """
    value: float
    
    def __str__(self) -> str:
        if self.value > 0:
            return f"+{self.value}"
        return str(self.value)
    
    def matches(self, other: "BettingLine", tolerance: float = 0.5) -> bool:
        """
        Check if two lines match within tolerance.
        
        Args:
            other: Other betting line
            tolerance: Maximum difference allowed (default 0.5 points)
            
        Returns:
            True if lines match
        """
        return abs(self.value - other.value) <= tolerance


class ParsedMarket(BaseModel):
    """
    Parsed market information for matching.
    
    Examples:
    - "Will Lakers beat Celtics?" → MarketType.MONEYLINE, team="Lakers"
    - "Will Lakers cover -7.5?" → MarketType.SPREAD, team="Lakers", line=-7.5
    - "Will total score exceed 220.5?" → MarketType.TOTAL, line=220.5
    """
    market_type: MarketType
    team: Optional[str] = None
    player: Optional[str] = None
    line: Optional[BettingLine] = None
    period: Optional[str] = None  # "Q1", "Q2", "1H", "2H", "Full"
    
    def is_compatible_with(self, other: "ParsedMarket") -> bool:
        """
        Check if this market can be arbitraged with another.
        
        Requirements:
        - Same market type
        - Same team (for team markets)
        - Same player (for player props)
        - Same line (for spreads/totals, within tolerance)
        - Same period (for quarter/half markets)
        
        Args:
            other: Other parsed market
            
        Returns:
            True if markets are compatible for arbitrage
        """
        # Must be same type
        if self.market_type != other.market_type:
            return False
        
        # Team markets must reference same team
        if self.team and other.team:
            if self.team.lower() != other.team.lower():
                return False
        
        # Player props must reference same player
        if self.player and other.player:
            if self.player.lower() != other.player.lower():
                return False
        
        # Spreads and totals must have matching lines
        if self.market_type in (MarketType.SPREAD, MarketType.TOTAL):
            if not self.line or not other.line:
                return False
            if not self.line.matches(other.line):
                return False
        
        # Period-specific markets must match
        if self.period and other.period:
            if self.period != other.period:
                return False
        
        return True
