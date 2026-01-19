"""Game-related Pydantic v2 models for multi-sport support."""

from datetime import datetime
from enum import Enum
from typing import Annotated, Optional

from pydantic import BaseModel, ConfigDict, Field, computed_field


class Sport(str, Enum):
    """Supported sports."""
    NFL = "nfl"
    NBA = "nba"
    NHL = "nhl"
    MLB = "mlb"
    NCAAF = "ncaaf"
    NCAAB = "ncaab"
    MLS = "mls"
    SOCCER = "soccer"
    TENNIS = "tennis"
    MMA = "mma"

    @property
    def total_seconds(self) -> int:
        """Total regulation time in seconds."""
        return {
            Sport.NFL: 3600,
            Sport.NCAAF: 3600,
            Sport.NBA: 2880,
            Sport.NCAAB: 2400,
            Sport.NHL: 3600,
            Sport.MLB: 10800,
            Sport.MLS: 5400,
            Sport.SOCCER: 5400,
            Sport.TENNIS: 7200,
            Sport.MMA: 1500,
        }[self]

    @property
    def periods(self) -> int:
        """Number of periods/quarters/innings."""
        return {
            Sport.NFL: 4, Sport.NCAAF: 4,
            Sport.NBA: 4, Sport.NCAAB: 2,
            Sport.NHL: 3,
            Sport.MLB: 9,
            Sport.MLS: 2, Sport.SOCCER: 2,
            Sport.TENNIS: 3,
            Sport.MMA: 5,
        }[self]

    @property
    def period_label(self) -> str:
        """Label for periods (Q, P, H, I, etc.)."""
        return {
            Sport.NFL: "Q", Sport.NCAAF: "Q",
            Sport.NBA: "Q", Sport.NCAAB: "H",
            Sport.NHL: "P",
            Sport.MLB: "I",
            Sport.MLS: "H", Sport.SOCCER: "H",
            Sport.TENNIS: "S",
            Sport.MMA: "R",
        }[self]


class PlayType(str, Enum):
    """Types of plays across all sports."""
    # Universal
    UNKNOWN = "unknown"
    TIMEOUT = "timeout"
    PENALTY = "penalty"
    CHALLENGE = "challenge"
    INJURY = "injury"
    DELAY = "delay"

    # Football (NFL/NCAAF)
    RUSH = "rush"
    PASS_COMPLETE = "pass_complete"
    PASS_INCOMPLETE = "pass_incomplete"
    SACK = "sack"
    INTERCEPTION = "interception"
    FUMBLE = "fumble"
    FUMBLE_RECOVERY = "fumble_recovery"
    TOUCHDOWN = "touchdown"
    FIELD_GOAL = "field_goal"
    FIELD_GOAL_MISSED = "field_goal_missed"
    EXTRA_POINT = "extra_point"
    EXTRA_POINT_MISSED = "extra_point_missed"
    TWO_POINT_CONVERSION = "two_point_conversion"
    SAFETY = "safety"
    PUNT = "punt"
    KICKOFF = "kickoff"
    KICKOFF_RETURN = "kickoff_return"
    PUNT_RETURN = "punt_return"
    TURNOVER_ON_DOWNS = "turnover_on_downs"

    # Basketball (NBA/NCAAB)
    MADE_SHOT = "made_shot"
    MISSED_SHOT = "missed_shot"
    THREE_POINTER = "three_pointer"
    FREE_THROW = "free_throw"
    REBOUND = "rebound"
    ASSIST = "assist"
    STEAL = "steal"
    BLOCK = "block"
    TURNOVER = "turnover"
    FOUL = "foul"
    JUMP_BALL = "jump_ball"

    # Hockey (NHL)
    GOAL = "goal"
    SHOT_ON_GOAL = "shot_on_goal"
    SAVE = "save"
    HIT = "hit"
    BLOCKED_SHOT = "blocked_shot"
    FACEOFF = "faceoff"
    ICING = "icing"
    OFFSIDE = "offside"
    POWER_PLAY = "power_play"
    PENALTY_KILL = "penalty_kill"
    EMPTY_NET = "empty_net"

    # Baseball (MLB)
    SINGLE = "single"
    DOUBLE = "double"
    TRIPLE = "triple"
    HOME_RUN = "home_run"
    STRIKEOUT = "strikeout"
    WALK = "walk"
    HIT_BY_PITCH = "hit_by_pitch"
    SACRIFICE = "sacrifice"
    DOUBLE_PLAY = "double_play"
    TRIPLE_PLAY = "triple_play"
    GROUND_OUT = "ground_out"
    FLY_OUT = "fly_out"
    LINE_OUT = "line_out"
    STOLEN_BASE = "stolen_base"
    CAUGHT_STEALING = "caught_stealing"
    WILD_PITCH = "wild_pitch"
    PASSED_BALL = "passed_ball"
    BALK = "balk"
    RUN_SCORED = "run_scored"

    # Soccer/MLS
    SHOT = "shot"
    SHOT_ON_TARGET = "shot_on_target"
    CORNER = "corner"
    FREE_KICK = "free_kick"
    THROW_IN = "throw_in"
    GOAL_KICK = "goal_kick"
    OFFSIDE_SOCCER = "offside_soccer"
    SUBSTITUTION = "substitution"
    YELLOW_CARD = "yellow_card"
    RED_CARD = "red_card"
    VAR_REVIEW = "var_review"
    OWN_GOAL = "own_goal"


class Play(BaseModel):
    """A single play/event in a game."""
    model_config = ConfigDict(frozen=True)

    play_id: str
    game_id: str
    sport: Sport
    play_type: PlayType
    description: str
    team: Optional[str] = None
    player: Optional[str] = None
    timestamp: datetime
    sequence_number: int

    # Score context
    home_score: int
    away_score: int

    # Time context
    period: int
    time_remaining: str  # MM:SS format

    # Football specific
    yards_gained: Optional[int] = None
    yard_line: Optional[int] = None
    down: Optional[int] = None
    yards_to_go: Optional[int] = None
    is_scoring: bool = False
    is_turnover: bool = False

    # Basketball specific
    shot_distance: Optional[int] = None
    shot_type: Optional[str] = None

    # Hockey specific
    zone: Optional[str] = None  # offensive, defensive, neutral
    strength: Optional[str] = None  # even, power_play, penalty_kill

    @computed_field
    @property
    def time_remaining_seconds(self) -> int:
        """Convert MM:SS to total seconds."""
        try:
            parts = self.time_remaining.split(":")
            if len(parts) == 2:
                return int(parts[0]) * 60 + int(parts[1])
            return 0
        except (ValueError, IndexError):
            return 0


class GameState(BaseModel):
    """Current state of a live game."""
    model_config = ConfigDict(frozen=True)

    game_id: str
    sport: Sport
    home_team: str
    away_team: str
    home_score: int = 0
    away_score: int = 0
    period: int = 1
    time_remaining: str = "00:00"
    status: str = "scheduled"  # scheduled, in_progress, halftime, final
    possession: Optional[str] = None
    last_play_id: Optional[str] = None
    updated_at: datetime = Field(default_factory=datetime.utcnow)

    # Football specific
    down: Optional[int] = None
    yards_to_go: Optional[int] = None
    yard_line: Optional[int] = None
    is_redzone: bool = False

    # Hockey specific
    strength: Optional[str] = None  # even, 5on4, 4on5, etc.

    # Baseball specific
    balls: Optional[int] = None
    strikes: Optional[int] = None
    outs: Optional[int] = None
    runners_on_base: Optional[list[int]] = None  # [1, 2, 3] for bases

    @computed_field
    @property
    def time_remaining_seconds(self) -> int:
        """Convert MM:SS to total seconds."""
        try:
            parts = self.time_remaining.split(":")
            if len(parts) == 2:
                return int(parts[0]) * 60 + int(parts[1])
            return 0
        except (ValueError, IndexError):
            return 0

    @computed_field
    @property
    def total_time_remaining_seconds(self) -> int:
        """Total seconds remaining in regulation."""
        period_length = self.sport.total_seconds // self.sport.periods
        periods_remaining = max(0, self.sport.periods - self.period)
        return self.time_remaining_seconds + (periods_remaining * period_length)

    @computed_field
    @property
    def game_progress(self) -> float:
        """Fraction of game completed (0.0 to 1.0)."""
        total = self.sport.total_seconds
        remaining = self.total_time_remaining_seconds
        return 1.0 - (remaining / total) if total > 0 else 1.0

    @computed_field
    @property
    def score_diff(self) -> int:
        """Score differential (positive = home winning)."""
        return self.home_score - self.away_score


class GameInfo(BaseModel):
    """Basic game information for discovery/listing."""
    model_config = ConfigDict(frozen=True)

    game_id: str
    sport: Sport
    home_team: str
    away_team: str
    home_team_abbrev: str
    away_team_abbrev: str
    scheduled_time: datetime
    venue: Optional[str] = None
    broadcast: Optional[str] = None
    status: str = "scheduled"
    home_score: int = 0
    away_score: int = 0

    @computed_field
    @property
    def display_name(self) -> str:
        """Human-readable game name."""
        return f"{self.away_team_abbrev} @ {self.home_team_abbrev}"

    @computed_field
    @property
    def is_live(self) -> bool:
        """Whether the game is currently in progress."""
        return self.status in ("in_progress", "halftime", "end_period")
