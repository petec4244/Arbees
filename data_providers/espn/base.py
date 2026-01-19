"""Base data provider interface."""

from abc import ABC, abstractmethod
from typing import AsyncIterator, Callable, Coroutine, Optional

from arbees_shared.models.game import GameInfo, GameState, Play, Sport


class DataProvider(ABC):
    """Abstract base class for game data providers."""

    @property
    @abstractmethod
    def sport(self) -> Sport:
        """Get the sport this provider handles."""
        ...

    @abstractmethod
    async def get_live_games(self) -> list[GameInfo]:
        """Get list of currently live games."""
        ...

    @abstractmethod
    async def get_game_state(self, game_id: str) -> Optional[GameState]:
        """Get current state of a game."""
        ...

    @abstractmethod
    async def get_recent_plays(
        self,
        game_id: str,
        limit: int = 10,
    ) -> list[Play]:
        """Get recent plays for a game."""
        ...

    @abstractmethod
    async def poll_game(
        self,
        game_id: str,
        last_state: Optional[GameState] = None,
    ) -> tuple[Optional[GameState], list[Play]]:
        """
        Poll for game updates.

        Args:
            game_id: Game to poll
            last_state: Previous state for change detection

        Returns:
            Tuple of (new_state, new_plays)
        """
        ...

    @abstractmethod
    async def monitor_game(
        self,
        game_id: str,
        on_state_change: Callable[[GameState], Coroutine],
        on_play: Callable[[Play], Coroutine],
        poll_interval: float = 3.0,
    ) -> AsyncIterator[None]:
        """
        Monitor a game for updates.

        Args:
            game_id: Game to monitor
            on_state_change: Callback for state changes
            on_play: Callback for new plays
            poll_interval: Seconds between polls
        """
        ...

    async def connect(self) -> None:
        """Connect to data source (optional)."""
        pass

    async def disconnect(self) -> None:
        """Disconnect from data source (optional)."""
        pass
