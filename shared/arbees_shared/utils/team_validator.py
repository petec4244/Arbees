"""
Team name matching and validation logic.

Provides confidence-scored team matching to prevent wrong-team price selection
at entry and exit in trading flows.
"""

import re
from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class TeamMatchResult:
    """Result of team matching validation."""

    is_match: bool
    confidence: float  # 0.0 to 1.0
    method: str  # exact_match, nickname_match, contains_target, contains_contract, abbreviation_match, no_match
    reason: str

    def __repr__(self) -> str:
        return (
            f"TeamMatchResult(match={self.is_match}, "
            f"confidence={self.confidence:.2f}, "
            f"method='{self.method}')"
        )


class TeamValidator:
    """
    Validates team name matching across different formats.

    Used to ensure that:
    1. Entry prices come from the correct team's contract
    2. Exit prices are compared against the same team that was entered
    """

    # Common team name abbreviations/aliases
    # Maps normalized lowercase name -> canonical abbreviation
    ABBREVIATIONS: dict[str, str] = {
        # NBA
        "celtics": "BOS",
        "boston celtics": "BOS",
        "boston": "BOS",
        "lakers": "LAL",
        "los angeles lakers": "LAL",
        "warriors": "GSW",
        "golden state warriors": "GSW",
        "golden state": "GSW",
        "heat": "MIA",
        "miami heat": "MIA",
        "miami": "MIA",
        "knicks": "NYK",
        "new york knicks": "NYK",
        "nets": "BKN",
        "brooklyn nets": "BKN",
        "brooklyn": "BKN",
        "bulls": "CHI",
        "chicago bulls": "CHI",
        "chicago": "CHI",
        "cavaliers": "CLE",
        "cleveland cavaliers": "CLE",
        "cleveland": "CLE",
        "mavericks": "DAL",
        "dallas mavericks": "DAL",
        "dallas": "DAL",
        "nuggets": "DEN",
        "denver nuggets": "DEN",
        "denver": "DEN",
        "pistons": "DET",
        "detroit pistons": "DET",
        "detroit": "DET",
        "rockets": "HOU",
        "houston rockets": "HOU",
        "houston": "HOU",
        "pacers": "IND",
        "indiana pacers": "IND",
        "indiana": "IND",
        "clippers": "LAC",
        "la clippers": "LAC",
        "los angeles clippers": "LAC",
        "grizzlies": "MEM",
        "memphis grizzlies": "MEM",
        "memphis": "MEM",
        "bucks": "MIL",
        "milwaukee bucks": "MIL",
        "milwaukee": "MIL",
        "timberwolves": "MIN",
        "minnesota timberwolves": "MIN",
        "minnesota": "MIN",
        "pelicans": "NOP",
        "new orleans pelicans": "NOP",
        "new orleans": "NOP",
        "thunder": "OKC",
        "oklahoma city thunder": "OKC",
        "oklahoma city": "OKC",
        "magic": "ORL",
        "orlando magic": "ORL",
        "orlando": "ORL",
        "76ers": "PHI",
        "sixers": "PHI",
        "philadelphia 76ers": "PHI",
        "philadelphia": "PHI",
        "suns": "PHX",
        "phoenix suns": "PHX",
        "phoenix": "PHX",
        "trail blazers": "POR",
        "blazers": "POR",
        "portland trail blazers": "POR",
        "portland": "POR",
        "kings": "SAC",
        "sacramento kings": "SAC",
        "sacramento": "SAC",
        "spurs": "SAS",
        "san antonio spurs": "SAS",
        "san antonio": "SAS",
        "raptors": "TOR",
        "toronto raptors": "TOR",
        "toronto": "TOR",
        "jazz": "UTA",
        "utah jazz": "UTA",
        "utah": "UTA",
        "wizards": "WAS",
        "washington wizards": "WAS",
        "washington": "WAS",
        "hawks": "ATL",
        "atlanta hawks": "ATL",
        "atlanta": "ATL",
        "hornets": "CHA",
        "charlotte hornets": "CHA",
        "charlotte": "CHA",
        # NHL
        "bruins": "BOS",
        "boston bruins": "BOS",
        "sabres": "BUF",
        "buffalo sabres": "BUF",
        "buffalo": "BUF",
        "flames": "CGY",
        "calgary flames": "CGY",
        "calgary": "CGY",
        "hurricanes": "CAR",
        "carolina hurricanes": "CAR",
        "carolina": "CAR",
        "blackhawks": "CHI",
        "chicago blackhawks": "CHI",
        "avalanche": "COL",
        "colorado avalanche": "COL",
        "colorado": "COL",
        "blue jackets": "CBJ",
        "columbus blue jackets": "CBJ",
        "columbus": "CBJ",
        "stars": "DAL",
        "dallas stars": "DAL",
        "red wings": "DET",
        "detroit red wings": "DET",
        "oilers": "EDM",
        "edmonton oilers": "EDM",
        "edmonton": "EDM",
        "panthers": "FLA",
        "florida panthers": "FLA",
        "florida": "FLA",
        "kings": "LAK",
        "la kings": "LAK",
        "los angeles kings": "LAK",
        "wild": "MIN",
        "minnesota wild": "MIN",
        "canadiens": "MTL",
        "montreal canadiens": "MTL",
        "montreal": "MTL",
        "predators": "NSH",
        "nashville predators": "NSH",
        "nashville": "NSH",
        "devils": "NJD",
        "new jersey devils": "NJD",
        "new jersey": "NJD",
        "islanders": "NYI",
        "new york islanders": "NYI",
        "rangers": "NYR",
        "new york rangers": "NYR",
        "senators": "OTT",
        "ottawa senators": "OTT",
        "ottawa": "OTT",
        "flyers": "PHI",
        "philadelphia flyers": "PHI",
        "coyotes": "ARI",
        "arizona coyotes": "ARI",
        "arizona": "ARI",
        "penguins": "PIT",
        "pittsburgh penguins": "PIT",
        "pittsburgh": "PIT",
        "sharks": "SJS",
        "san jose sharks": "SJS",
        "san jose": "SJS",
        "kraken": "SEA",
        "seattle kraken": "SEA",
        "seattle": "SEA",
        "blues": "STL",
        "st louis blues": "STL",
        "st. louis blues": "STL",
        "st louis": "STL",
        "lightning": "TBL",
        "tampa bay lightning": "TBL",
        "tampa bay": "TBL",
        "maple leafs": "TOR",
        "toronto maple leafs": "TOR",
        "canucks": "VAN",
        "vancouver canucks": "VAN",
        "vancouver": "VAN",
        "golden knights": "VGK",
        "vegas golden knights": "VGK",
        "vegas": "VGK",
        "capitals": "WSH",
        "washington capitals": "WSH",
        "jets": "WPG",
        "winnipeg jets": "WPG",
        "winnipeg": "WPG",
        "ducks": "ANA",
        "anaheim ducks": "ANA",
        "anaheim": "ANA",
        # NFL (sample - add more as needed)
        "patriots": "NE",
        "new england patriots": "NE",
        "new england": "NE",
        "chiefs": "KC",
        "kansas city chiefs": "KC",
        "kansas city": "KC",
        "eagles": "PHI",
        "philadelphia eagles": "PHI",
        "bills": "BUF",
        "buffalo bills": "BUF",
        "ravens": "BAL",
        "baltimore ravens": "BAL",
        "baltimore": "BAL",
        "bengals": "CIN",
        "cincinnati bengals": "CIN",
        "cincinnati": "CIN",
        "browns": "CLE",
        "cleveland browns": "CLE",
        "steelers": "PIT",
        "pittsburgh steelers": "PIT",
        "colts": "IND",
        "indianapolis colts": "IND",
        "indianapolis": "IND",
        "jaguars": "JAX",
        "jacksonville jaguars": "JAX",
        "jacksonville": "JAX",
        "texans": "HOU",
        "houston texans": "HOU",
        "titans": "TEN",
        "tennessee titans": "TEN",
        "tennessee": "TEN",
        "broncos": "DEN",
        "denver broncos": "DEN",
        "raiders": "LV",
        "las vegas raiders": "LV",
        "las vegas": "LV",
        "chargers": "LAC",
        "los angeles chargers": "LAC",
        "cowboys": "DAL",
        "dallas cowboys": "DAL",
        "giants": "NYG",
        "new york giants": "NYG",
        "commanders": "WAS",
        "washington commanders": "WAS",
        "bears": "CHI",
        "chicago bears": "CHI",
        "lions": "DET",
        "detroit lions": "DET",
        "packers": "GB",
        "green bay packers": "GB",
        "green bay": "GB",
        "vikings": "MIN",
        "minnesota vikings": "MIN",
        "falcons": "ATL",
        "atlanta falcons": "ATL",
        "saints": "NO",
        "new orleans saints": "NO",
        "buccaneers": "TB",
        "tampa bay buccaneers": "TB",
        "bucs": "TB",
        "cardinals": "ARI",
        "arizona cardinals": "ARI",
        "rams": "LAR",
        "los angeles rams": "LAR",
        "49ers": "SF",
        "san francisco 49ers": "SF",
        "san francisco": "SF",
        "seahawks": "SEA",
        "seattle seahawks": "SEA",
        # MLB (sample)
        "yankees": "NYY",
        "new york yankees": "NYY",
        "red sox": "BOS",
        "boston red sox": "BOS",
        "dodgers": "LAD",
        "los angeles dodgers": "LAD",
        "cubs": "CHC",
        "chicago cubs": "CHC",
        "white sox": "CHW",
        "chicago white sox": "CHW",
        "mets": "NYM",
        "new york mets": "NYM",
        "braves": "ATL",
        "atlanta braves": "ATL",
        "astros": "HOU",
        "houston astros": "HOU",
        "phillies": "PHI",
        "philadelphia phillies": "PHI",
        "padres": "SD",
        "san diego padres": "SD",
        "san diego": "SD",
        "mariners": "SEA",
        "seattle mariners": "SEA",
        "twins": "MIN",
        "minnesota twins": "MIN",
        "guardians": "CLE",
        "cleveland guardians": "CLE",
        "tigers": "DET",
        "detroit tigers": "DET",
        "royals": "KC",
        "kansas city royals": "KC",
        "angels": "LAA",
        "los angeles angels": "LAA",
        "athletics": "OAK",
        "oakland athletics": "OAK",
        "oakland": "OAK",
        "rangers": "TEX",
        "texas rangers": "TEX",
        "texas": "TEX",
        "orioles": "BAL",
        "baltimore orioles": "BAL",
        "rays": "TB",
        "tampa bay rays": "TB",
        "blue jays": "TOR",
        "toronto blue jays": "TOR",
        "rockies": "COL",
        "colorado rockies": "COL",
        "diamondbacks": "ARI",
        "arizona diamondbacks": "ARI",
        "dbacks": "ARI",
        "reds": "CIN",
        "cincinnati reds": "CIN",
        "brewers": "MIL",
        "milwaukee brewers": "MIL",
        "pirates": "PIT",
        "pittsburgh pirates": "PIT",
        "marlins": "MIA",
        "miami marlins": "MIA",
        "nationals": "WSH",
        "washington nationals": "WSH",
    }

    @staticmethod
    def normalize(team: str) -> str:
        """
        Normalize team name for comparison.

        - Lowercase
        - Remove special characters (except alphanumeric and spaces)
        - Collapse whitespace
        - Strip leading/trailing whitespace
        """
        if not team:
            return ""
        # Remove special chars except alphanumeric and spaces
        normalized = re.sub(r"[^\w\s]", "", team.lower())
        # Collapse whitespace
        normalized = re.sub(r"\s+", " ", normalized).strip()
        return normalized

    @staticmethod
    def extract_nickname(team: str) -> str:
        """
        Extract team nickname (last word).

        E.g., "Philadelphia Flyers" -> "flyers"
        """
        if not team:
            return ""
        normalized = TeamValidator.normalize(team)
        parts = normalized.split()
        return parts[-1] if parts else ""

    @staticmethod
    def extract_city(team: str) -> str:
        """
        Extract city/location (first word(s) before nickname).

        E.g., "Philadelphia Flyers" -> "philadelphia"
        """
        if not team:
            return ""
        normalized = TeamValidator.normalize(team)
        parts = normalized.split()
        if len(parts) > 1:
            return " ".join(parts[:-1])
        return ""

    def get_abbreviation(self, team: str) -> Optional[str]:
        """Get canonical abbreviation for a team name."""
        normalized = self.normalize(team)
        return self.ABBREVIATIONS.get(normalized)

    def validate_match(
        self,
        target_team: str,
        contract_team: str,
    ) -> TeamMatchResult:
        """
        Validate if contract_team matches target_team.

        Returns TeamMatchResult with confidence score (0.0 to 1.0).

        Matching methods (in order of confidence):
        1. Exact match (1.0 confidence)
        2. Nickname match (0.9 confidence)
        3. One contains the other (0.8 confidence)
        4. Abbreviation match (0.75 confidence)
        5. No match (0.0 confidence)
        """
        if not target_team or not contract_team:
            return TeamMatchResult(
                is_match=False,
                confidence=0.0,
                method="empty_input",
                reason="Target or contract team is empty",
            )

        target_norm = self.normalize(target_team)
        contract_norm = self.normalize(contract_team)

        # Method 1: Exact match (100% confidence)
        if target_norm == contract_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=1.0,
                method="exact_match",
                reason=f"Exact: '{target_team}' == '{contract_team}'",
            )

        # Method 2: Nickname match (90% confidence)
        target_nick = self.extract_nickname(target_team)
        contract_nick = self.extract_nickname(contract_team)
        if target_nick and contract_nick and target_nick == contract_nick:
            return TeamMatchResult(
                is_match=True,
                confidence=0.9,
                method="nickname_match",
                reason=f"Nickname: '{target_nick}' == '{contract_nick}'",
            )

        # Method 3: One contains the other (80% confidence)
        # Guard against false positives: require minimum length for substring matches
        min_substring_len = 4  # Prevents "NY" matching "Sydney"
        if len(target_norm) >= min_substring_len and target_norm in contract_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=0.8,
                method="contains_target",
                reason=f"'{target_norm}' in '{contract_norm}'",
            )
        if len(contract_norm) >= min_substring_len and contract_norm in target_norm:
            return TeamMatchResult(
                is_match=True,
                confidence=0.8,
                method="contains_contract",
                reason=f"'{contract_norm}' in '{target_norm}'",
            )

        # Method 4: Abbreviation match (75% confidence)
        target_abbr = self.get_abbreviation(target_norm)
        contract_abbr = self.get_abbreviation(contract_norm)

        # Also check if one of the inputs IS an abbreviation
        if target_abbr and contract_abbr and target_abbr == contract_abbr:
            return TeamMatchResult(
                is_match=True,
                confidence=0.75,
                method="abbreviation_match",
                reason=f"Abbr: {target_abbr} == {contract_abbr}",
            )

        # Also check nickname -> abbreviation mapping
        target_nick_abbr = self.ABBREVIATIONS.get(target_nick)
        contract_nick_abbr = self.ABBREVIATIONS.get(contract_nick)
        if target_nick_abbr and contract_nick_abbr and target_nick_abbr == contract_nick_abbr:
            return TeamMatchResult(
                is_match=True,
                confidence=0.75,
                method="abbreviation_match",
                reason=f"Nickname abbr: {target_nick_abbr} == {contract_nick_abbr}",
            )

        # No match
        return TeamMatchResult(
            is_match=False,
            confidence=0.0,
            method="no_match",
            reason=f"No match: '{target_team}' vs '{contract_team}'",
        )

    def find_best_match(
        self,
        target_team: str,
        candidates: list[str],
        min_confidence: float = 0.0,
    ) -> tuple[Optional[int], Optional[TeamMatchResult]]:
        """
        Find the best matching candidate for a target team.

        Args:
            target_team: The team name to match against
            candidates: List of candidate team names
            min_confidence: Minimum confidence threshold (0.0 to 1.0)

        Returns:
            Tuple of (best_index, best_result) or (None, None) if no match
        """
        if not target_team or not candidates:
            return None, None

        best_index: Optional[int] = None
        best_result: Optional[TeamMatchResult] = None
        best_confidence = -1.0

        for i, candidate in enumerate(candidates):
            result = self.validate_match(target_team, candidate)
            if result.is_match and result.confidence > best_confidence:
                best_confidence = result.confidence
                best_result = result
                best_index = i

        if best_result and best_confidence >= min_confidence:
            return best_index, best_result

        return None, None


# Module-level singleton for convenience
_validator = TeamValidator()


def validate_team_match(target_team: str, contract_team: str) -> TeamMatchResult:
    """
    Convenience function to validate team match.

    Args:
        target_team: The expected team (from signal)
        contract_team: The team from market data

    Returns:
        TeamMatchResult with is_match, confidence, method, and reason
    """
    return _validator.validate_match(target_team, contract_team)


def find_best_team_match(
    target_team: str,
    candidates: list[str],
    min_confidence: float = 0.0,
) -> tuple[Optional[int], Optional[TeamMatchResult]]:
    """
    Convenience function to find best matching team.

    Args:
        target_team: The team name to match against
        candidates: List of candidate team names
        min_confidence: Minimum confidence threshold

    Returns:
        Tuple of (best_index, best_result) or (None, None) if no match
    """
    return _validator.find_best_match(target_team, candidates, min_confidence)
