"""
ESPN data provider for live game data.

Supports: NFL, NBA, NHL, MLB, NCAAF, NCAAB, MLS, Soccer
"""

import asyncio
import logging
from datetime import datetime
from typing import AsyncIterator, Callable, Coroutine, Optional

import aiohttp
from tenacity import AsyncRetrying, stop_after_attempt, wait_exponential

from arbees_shared.models.game import GameInfo, GameState, Play, PlayType, Sport
from data_providers.espn.base import DataProvider

logger = logging.getLogger(__name__)


# ESPN API URL patterns by sport
ESPN_URLS = {
    Sport.NFL: "https://site.api.espn.com/apis/site/v2/sports/football/nfl",
    Sport.NBA: "https://site.api.espn.com/apis/site/v2/sports/basketball/nba",
    Sport.NHL: "https://site.api.espn.com/apis/site/v2/sports/hockey/nhl",
    Sport.MLB: "https://site.api.espn.com/apis/site/v2/sports/baseball/mlb",
    Sport.NCAAF: "https://site.api.espn.com/apis/site/v2/sports/football/college-football",
    Sport.NCAAB: "https://site.api.espn.com/apis/site/v2/sports/basketball/mens-college-basketball",
    Sport.MLS: "https://site.api.espn.com/apis/site/v2/sports/soccer/usa.1",
    Sport.SOCCER: "https://site.api.espn.com/apis/site/v2/sports/soccer/eng.1",
}


class ESPNClient(DataProvider):
    """Multi-sport ESPN data provider."""

    def __init__(
        self,
        sport: Sport,
        timeout: float = 10.0,
        max_retries: int = 3,
    ):
        """
        Initialize ESPN client for a specific sport.

        Args:
            sport: Sport to fetch data for
            timeout: Request timeout in seconds
            max_retries: Max retry attempts
        """
        self._sport = sport
        self.base_url = ESPN_URLS.get(sport, ESPN_URLS[Sport.NFL])
        self.timeout = aiohttp.ClientTimeout(total=timeout)
        self.max_retries = max_retries
        self._session: Optional[aiohttp.ClientSession] = None

        # State caching
        self._game_states: dict[str, GameState] = {}
        self._last_play_ids: dict[str, set[str]] = {}

    @property
    def sport(self) -> Sport:
        return self._sport

    async def connect(self) -> None:
        """Create aiohttp session."""
        if self._session is None:
            connector = aiohttp.TCPConnector(limit=20)
            self._session = aiohttp.ClientSession(
                connector=connector,
                timeout=self.timeout,
            )

    async def disconnect(self) -> None:
        """Close aiohttp session."""
        if self._session:
            await self._session.close()
            self._session = None

    async def __aenter__(self) -> "ESPNClient":
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        await self.disconnect()

    def _ensure_session(self) -> aiohttp.ClientSession:
        """Ensure session exists."""
        if self._session is None:
            raise RuntimeError("Client not connected. Call connect() first.")
        return self._session

    async def _fetch(self, endpoint: str) -> dict:
        """Fetch data from ESPN API with retry."""
        session = self._ensure_session()
        url = f"{self.base_url}/{endpoint}"

        async for attempt in AsyncRetrying(
            stop=stop_after_attempt(self.max_retries),
            wait=wait_exponential(multiplier=1, min=1, max=5),
            reraise=True,
        ):
            with attempt:
                async with session.get(url) as response:
                    response.raise_for_status()
                    return await response.json()

    # ==========================================================================
    # Game Discovery
    # ==========================================================================

    async def get_live_games(self) -> list[GameInfo]:
        """Get list of currently live games."""
        try:
            data = await self._fetch("scoreboard")
            events = data.get("events", [])

            games = []
            for event in events:
                game_info = self._parse_game_info(event)
                if game_info and game_info.is_live:
                    games.append(game_info)

            return games
        except Exception as e:
            logger.error(f"Error fetching live games for {self._sport}: {e}")
            return []

    async def get_all_games_today(self) -> list[GameInfo]:
        """Get all games scheduled for today."""
        try:
            data = await self._fetch("scoreboard")
            events = data.get("events", [])

            games = []
            for event in events:
                game_info = self._parse_game_info(event)
                if game_info:
                    games.append(game_info)

            return games
        except Exception as e:
            logger.error(f"Error fetching games for {self._sport}: {e}")
            return []

    def _parse_game_info(self, event: dict) -> Optional[GameInfo]:
        """Parse ESPN event into GameInfo."""
        try:
            competitions = event.get("competitions", [])
            if not competitions:
                return None

            competition = competitions[0]
            competitors = competition.get("competitors", [])
            if len(competitors) < 2:
                return None

            # Home/away determination
            home = next((c for c in competitors if c.get("homeAway") == "home"), competitors[0])
            away = next((c for c in competitors if c.get("homeAway") == "away"), competitors[1])

            home_team = home.get("team", {})
            away_team = away.get("team", {})

            # Parse status
            status_data = competition.get("status", {})
            status_type = status_data.get("type", {})
            status_name = status_type.get("name", "").lower()

            # Map ESPN status to our status
            if status_name == "status_in_progress":
                status = "in_progress"
            elif status_name == "status_halftime":
                status = "halftime"
            elif status_name == "status_end_period":
                status = "end_period"
            elif status_name == "status_final":
                status = "final"
            elif status_name == "status_scheduled":
                status = "scheduled"
            else:
                status = status_name

            return GameInfo(
                game_id=event.get("id", ""),
                sport=self._sport,
                home_team=home_team.get("displayName", ""),
                away_team=away_team.get("displayName", ""),
                home_team_abbrev=home_team.get("abbreviation", ""),
                away_team_abbrev=away_team.get("abbreviation", ""),
                scheduled_time=datetime.fromisoformat(
                    event.get("date", "").replace("Z", "+00:00")
                ),
                venue=competition.get("venue", {}).get("fullName"),
                broadcast=self._get_broadcast(competition),
                status=status,
                home_score=int(home.get("score", 0) or 0),
                away_score=int(away.get("score", 0) or 0),
            )
        except Exception as e:
            logger.warning(f"Error parsing game info: {e}")
            return None

    def _get_broadcast(self, competition: dict) -> Optional[str]:
        """Extract broadcast info."""
        broadcasts = competition.get("broadcasts", [])
        if broadcasts and len(broadcasts) > 0:
            names = broadcasts[0].get("names", [])
            if names:
                return names[0]
        return None

    # ==========================================================================
    # Game State
    # ==========================================================================

    async def get_game_state(self, game_id: str) -> Optional[GameState]:
        """Get current state of a game."""
        try:
            data = await self._fetch(f"summary?event={game_id}")
            return self._parse_game_state(data, game_id)
        except Exception as e:
            logger.error(f"Error fetching game state for {game_id}: {e}")
            return None

    def _parse_game_state(self, data: dict, game_id: str) -> Optional[GameState]:
        """Parse ESPN summary into GameState."""
        try:
            header = data.get("header", {})
            competitions = header.get("competitions", [])
            if not competitions:
                return None

            competition = competitions[0]
            competitors = competition.get("competitors", [])
            if len(competitors) < 2:
                return None

            home = next((c for c in competitors if c.get("homeAway") == "home"), competitors[0])
            away = next((c for c in competitors if c.get("homeAway") == "away"), competitors[1])

            # Status
            status_data = competition.get("status", {})
            status_type = status_data.get("type", {})
            period = status_data.get("period", 1)
            clock = status_data.get("displayClock", "0:00")

            # Map status
            status_name = status_type.get("name", "").lower()
            if "progress" in status_name:
                status = "in_progress"
            elif "halftime" in status_name:
                status = "halftime"
            elif "final" in status_name:
                status = "final"
            else:
                status = status_name

            # Build state - sport-specific parsing
            state_kwargs = {
                "game_id": game_id,
                "sport": self._sport,
                "home_team": home.get("team", {}).get("displayName", ""),
                "away_team": away.get("team", {}).get("displayName", ""),
                "home_score": int(home.get("score", 0) or 0),
                "away_score": int(away.get("score", 0) or 0),
                "period": period,
                "time_remaining": clock,
                "status": status,
            }

            # Football-specific (NFL, NCAAF)
            if self._sport in (Sport.NFL, Sport.NCAAF):
                situation = data.get("situation", data.get("drives", {}).get("current", {}))
                if situation:
                    state_kwargs.update({
                        "possession": situation.get("possession", {}).get("displayName"),
                        "down": situation.get("down"),
                        "yards_to_go": situation.get("distance"),
                        "yard_line": situation.get("yardLine"),
                        "is_redzone": situation.get("isRedZone", False),
                    })

            # Hockey-specific
            elif self._sport == Sport.NHL:
                # Extract power play info if available
                boxscore = data.get("boxscore", {})
                teams = boxscore.get("teams", [])
                for team in teams:
                    stats = team.get("statistics", [])
                    for stat in stats:
                        if stat.get("name") == "powerPlayGoals":
                            # Could track power play state here
                            pass

            # Baseball-specific
            elif self._sport == Sport.MLB:
                situation = data.get("situation", {})
                if situation:
                    state_kwargs.update({
                        "balls": situation.get("balls"),
                        "strikes": situation.get("strikes"),
                        "outs": situation.get("outs"),
                        "runners_on_base": self._parse_runners(situation),
                    })

            return GameState(**state_kwargs)

        except Exception as e:
            logger.warning(f"Error parsing game state: {e}")
            return None

    def _parse_runners(self, situation: dict) -> Optional[list[int]]:
        """Parse runners on base for baseball."""
        runners = []
        if situation.get("onFirst"):
            runners.append(1)
        if situation.get("onSecond"):
            runners.append(2)
        if situation.get("onThird"):
            runners.append(3)
        return runners if runners else None

    # ==========================================================================
    # Plays
    # ==========================================================================

    async def get_recent_plays(
        self,
        game_id: str,
        limit: int = 10,
    ) -> list[Play]:
        """Get recent plays for a game."""
        try:
            data = await self._fetch(f"playbyplay?event={game_id}")
            plays = self._parse_plays(data, game_id)
            return plays[-limit:] if len(plays) > limit else plays
        except Exception as e:
            logger.error(f"Error fetching plays for {game_id}: {e}")
            return []

    def _parse_plays(self, data: dict, game_id: str) -> list[Play]:
        """Parse ESPN play-by-play into Play objects."""
        plays = []
        items = data.get("items", data.get("plays", []))

        for idx, item in enumerate(items):
            try:
                play = self._parse_single_play(item, game_id, idx)
                if play:
                    plays.append(play)
            except Exception as e:
                logger.debug(f"Error parsing play: {e}")

        return plays

    def _parse_single_play(
        self,
        item: dict,
        game_id: str,
        sequence: int,
    ) -> Optional[Play]:
        """Parse a single play item."""
        try:
            play_id = item.get("id", str(sequence))
            text = item.get("text", item.get("description", ""))
            play_type = self._classify_play_type(text, item)

            # Get score context
            home_score = 0
            away_score = 0
            if "homeScore" in item:
                home_score = int(item.get("homeScore", 0))
                away_score = int(item.get("awayScore", 0))
            elif "score" in item:
                score = item.get("score", {})
                home_score = int(score.get("home", 0))
                away_score = int(score.get("away", 0))

            # Time context
            period = item.get("period", {}).get("number", 1) if isinstance(item.get("period"), dict) else item.get("period", 1)
            clock = item.get("clock", {}).get("displayValue", "0:00") if isinstance(item.get("clock"), dict) else item.get("clock", "0:00")

            # Parse timestamp
            wall_clock = item.get("wallclock", item.get("created"))
            if wall_clock:
                try:
                    timestamp = datetime.fromisoformat(wall_clock.replace("Z", "+00:00"))
                except (ValueError, AttributeError):
                    timestamp = datetime.utcnow()
            else:
                timestamp = datetime.utcnow()

            # Build play
            play_kwargs = {
                "play_id": str(play_id),
                "game_id": game_id,
                "sport": self._sport,
                "play_type": play_type,
                "description": text,
                "timestamp": timestamp,
                "sequence_number": sequence,
                "home_score": home_score,
                "away_score": away_score,
                "period": period,
                "time_remaining": clock,
            }

            # Football-specific
            if self._sport in (Sport.NFL, Sport.NCAAF):
                play_kwargs.update({
                    "yards_gained": item.get("statYardage"),
                    "yard_line": item.get("end", {}).get("yardLine") if isinstance(item.get("end"), dict) else None,
                    "down": item.get("start", {}).get("down") if isinstance(item.get("start"), dict) else None,
                    "yards_to_go": item.get("start", {}).get("distance") if isinstance(item.get("start"), dict) else None,
                    "is_scoring": item.get("scoringPlay", False),
                    "is_turnover": "turnover" in text.lower() or "intercept" in text.lower() or "fumble" in text.lower(),
                })

            return Play(**play_kwargs)

        except Exception as e:
            logger.debug(f"Error parsing play: {e}")
            return None

    def _classify_play_type(self, text: str, item: dict) -> PlayType:
        """Classify play type from description."""
        text_lower = text.lower()

        # Use ESPN play type if available
        espn_type = item.get("type", {}).get("text", "").lower() if isinstance(item.get("type"), dict) else ""

        # Football plays
        if self._sport in (Sport.NFL, Sport.NCAAF):
            if "touchdown" in text_lower:
                return PlayType.TOUCHDOWN
            if "field goal" in text_lower:
                if "good" in text_lower or "made" in text_lower:
                    return PlayType.FIELD_GOAL
                return PlayType.FIELD_GOAL_MISSED
            if "interception" in text_lower:
                return PlayType.INTERCEPTION
            if "fumble" in text_lower:
                if "recovered" in text_lower:
                    return PlayType.FUMBLE_RECOVERY
                return PlayType.FUMBLE
            if "sack" in text_lower:
                return PlayType.SACK
            if "pass" in text_lower:
                if "incomplete" in text_lower:
                    return PlayType.PASS_INCOMPLETE
                return PlayType.PASS_COMPLETE
            if "rush" in text_lower or "run" in text_lower:
                return PlayType.RUSH
            if "punt" in text_lower:
                return PlayType.PUNT
            if "kickoff" in text_lower:
                return PlayType.KICKOFF

        # Basketball plays
        elif self._sport in (Sport.NBA, Sport.NCAAB):
            if "three point" in text_lower or "3-pt" in text_lower:
                return PlayType.THREE_POINTER
            if "free throw" in text_lower:
                return PlayType.FREE_THROW
            if "made" in text_lower or "makes" in text_lower:
                return PlayType.MADE_SHOT
            if "missed" in text_lower or "misses" in text_lower:
                return PlayType.MISSED_SHOT
            if "rebound" in text_lower:
                return PlayType.REBOUND
            if "turnover" in text_lower:
                return PlayType.TURNOVER
            if "steal" in text_lower:
                return PlayType.STEAL
            if "block" in text_lower:
                return PlayType.BLOCK
            if "foul" in text_lower:
                return PlayType.FOUL

        # Hockey plays
        elif self._sport == Sport.NHL:
            if "goal" in text_lower:
                return PlayType.GOAL
            if "shot" in text_lower:
                return PlayType.SHOT_ON_GOAL
            if "save" in text_lower:
                return PlayType.SAVE
            if "penalty" in text_lower:
                return PlayType.PENALTY
            if "faceoff" in text_lower:
                return PlayType.FACEOFF

        # Baseball plays
        elif self._sport == Sport.MLB:
            if "home run" in text_lower:
                return PlayType.HOME_RUN
            if "triple" in text_lower:
                return PlayType.TRIPLE
            if "double" in text_lower and "play" not in text_lower:
                return PlayType.DOUBLE
            if "single" in text_lower:
                return PlayType.SINGLE
            if "strikeout" in text_lower or "strikes out" in text_lower:
                return PlayType.STRIKEOUT
            if "walk" in text_lower:
                return PlayType.WALK
            if "ground" in text_lower and "out" in text_lower:
                return PlayType.GROUND_OUT
            if "fly" in text_lower and "out" in text_lower:
                return PlayType.FLY_OUT

        # Soccer plays
        elif self._sport in (Sport.MLS, Sport.SOCCER):
            if "goal" in text_lower:
                if "own" in text_lower:
                    return PlayType.OWN_GOAL
                return PlayType.GOAL
            if "shot" in text_lower:
                return PlayType.SHOT
            if "corner" in text_lower:
                return PlayType.CORNER
            if "yellow" in text_lower:
                return PlayType.YELLOW_CARD
            if "red" in text_lower:
                return PlayType.RED_CARD
            if "substitution" in text_lower:
                return PlayType.SUBSTITUTION

        return PlayType.UNKNOWN

    # ==========================================================================
    # Polling and Monitoring
    # ==========================================================================

    async def poll_game(
        self,
        game_id: str,
        last_state: Optional[GameState] = None,
    ) -> tuple[Optional[GameState], list[Play]]:
        """Poll for game updates."""
        # Get current state
        new_state = await self.get_game_state(game_id)

        # Detect new plays
        new_plays = []
        if new_state and (last_state is None or self._state_changed(last_state, new_state)):
            all_plays = await self.get_recent_plays(game_id, limit=50)

            # Filter to new plays only
            known_ids = self._last_play_ids.get(game_id, set())
            for play in all_plays:
                if play.play_id not in known_ids:
                    new_plays.append(play)
                    known_ids.add(play.play_id)

            self._last_play_ids[game_id] = known_ids

        # Cache state
        if new_state:
            self._game_states[game_id] = new_state

        return new_state, new_plays

    def _state_changed(self, old: GameState, new: GameState) -> bool:
        """Check if game state changed."""
        return (
            old.home_score != new.home_score or
            old.away_score != new.away_score or
            old.period != new.period or
            old.time_remaining != new.time_remaining or
            old.status != new.status
        )

    async def monitor_game(
        self,
        game_id: str,
        on_state_change: Callable[[GameState], Coroutine],
        on_play: Callable[[Play], Coroutine],
        poll_interval: float = 3.0,
    ) -> AsyncIterator[None]:
        """Monitor a game for updates."""
        last_state = self._game_states.get(game_id)

        while True:
            try:
                new_state, new_plays = await self.poll_game(game_id, last_state)

                if new_state:
                    if last_state is None or self._state_changed(last_state, new_state):
                        await on_state_change(new_state)
                    last_state = new_state

                for play in new_plays:
                    await on_play(play)

                # Check if game is final
                if new_state and new_state.status == "final":
                    logger.info(f"Game {game_id} is final")
                    break

            except Exception as e:
                logger.error(f"Error monitoring game {game_id}: {e}")

            await asyncio.sleep(poll_interval)
            yield
