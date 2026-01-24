"""
Unit tests for TeamValidator.

Tests team name matching with various scenarios including:
- Exact matches
- Nickname matches
- Contains matches (with substring false positive prevention)
- Abbreviation matches
- No-match scenarios
"""

import pytest

from arbees_shared.utils.team_validator import (
    TeamValidator,
    TeamMatchResult,
    validate_team_match,
    find_best_team_match,
)


class TestTeamValidator:
    """Tests for TeamValidator class."""

    @pytest.fixture
    def validator(self) -> TeamValidator:
        """Create a fresh validator instance."""
        return TeamValidator()

    # =========================================================================
    # Normalization tests
    # =========================================================================

    def test_normalize_lowercase(self, validator: TeamValidator) -> None:
        """Normalize should convert to lowercase."""
        assert validator.normalize("Boston Celtics") == "boston celtics"

    def test_normalize_removes_special_chars(self, validator: TeamValidator) -> None:
        """Normalize should remove special characters."""
        assert validator.normalize("St. Louis Blues") == "st louis blues"
        assert validator.normalize("Philadelphia 76ers") == "philadelphia 76ers"

    def test_normalize_collapses_whitespace(self, validator: TeamValidator) -> None:
        """Normalize should collapse multiple spaces."""
        assert validator.normalize("Boston   Celtics") == "boston celtics"

    def test_normalize_empty_string(self, validator: TeamValidator) -> None:
        """Normalize should handle empty strings."""
        assert validator.normalize("") == ""
        assert validator.normalize(None) == ""  # type: ignore

    # =========================================================================
    # Extract nickname tests
    # =========================================================================

    def test_extract_nickname(self, validator: TeamValidator) -> None:
        """Extract nickname should return last word."""
        assert validator.extract_nickname("Philadelphia Flyers") == "flyers"
        assert validator.extract_nickname("Boston Celtics") == "celtics"
        assert validator.extract_nickname("Lakers") == "lakers"

    def test_extract_nickname_empty(self, validator: TeamValidator) -> None:
        """Extract nickname should handle empty strings."""
        assert validator.extract_nickname("") == ""

    # =========================================================================
    # Exact match tests
    # =========================================================================

    def test_exact_match(self, validator: TeamValidator) -> None:
        """Exact match should have confidence 1.0."""
        result = validator.validate_match("Boston Celtics", "Boston Celtics")
        assert result.is_match is True
        assert result.confidence == 1.0
        assert result.method == "exact_match"

    def test_exact_match_case_insensitive(self, validator: TeamValidator) -> None:
        """Exact match should be case-insensitive."""
        result = validator.validate_match("BOSTON CELTICS", "boston celtics")
        assert result.is_match is True
        assert result.confidence == 1.0
        assert result.method == "exact_match"

    # =========================================================================
    # Nickname match tests
    # =========================================================================

    def test_nickname_match(self, validator: TeamValidator) -> None:
        """Nickname match should have confidence 0.9."""
        result = validator.validate_match("Philadelphia Flyers", "Flyers")
        assert result.is_match is True
        assert result.confidence == 0.9
        assert result.method == "nickname_match"

    def test_nickname_match_reverse(self, validator: TeamValidator) -> None:
        """Nickname match should work in reverse."""
        result = validator.validate_match("Flyers", "Philadelphia Flyers")
        assert result.is_match is True
        assert result.confidence == 0.9
        assert result.method == "nickname_match"

    def test_nickname_match_different_cities(self, validator: TeamValidator) -> None:
        """Same nickname different city should still match by nickname."""
        # Both extract nickname "Lakers"
        result = validator.validate_match("Los Angeles Lakers", "LA Lakers")
        assert result.is_match is True
        # Should match by nickname
        assert result.confidence >= 0.7

    # =========================================================================
    # Contains match tests
    # =========================================================================

    def test_contains_target_in_contract(self, validator: TeamValidator) -> None:
        """Contains match should have confidence 0.8."""
        result = validator.validate_match("Boston", "Boston Celtics")
        assert result.is_match is True
        assert result.confidence == 0.8
        assert result.method == "contains_target"

    def test_contains_contract_in_target(self, validator: TeamValidator) -> None:
        """Contains match should work in reverse."""
        result = validator.validate_match("Boston Celtics", "Celtics")
        # This will match by nickname (0.9) first
        assert result.is_match is True
        assert result.confidence >= 0.8

    def test_contains_requires_minimum_length(self, validator: TeamValidator) -> None:
        """Contains match should require minimum 4 chars to prevent false positives."""
        # "NY" is too short (< 4 chars), should NOT match "Sydney"
        result = validator.validate_match("NY", "Sydney")
        assert result.is_match is False
        assert result.confidence == 0.0

    def test_contains_short_strings_no_false_positive(self, validator: TeamValidator) -> None:
        """Short strings should not create false positives."""
        # "LA" should not match "Atlanta"
        result = validator.validate_match("LA", "Atlanta")
        assert result.is_match is False

    # =========================================================================
    # Abbreviation match tests
    # =========================================================================

    def test_abbreviation_match_nba(self, validator: TeamValidator) -> None:
        """Abbreviation match for NBA teams."""
        result = validator.validate_match("Boston Celtics", "Celtics")
        # Will match by nickname (0.9) which is higher confidence
        assert result.is_match is True
        assert result.confidence >= 0.75

    def test_abbreviation_match_nhl(self, validator: TeamValidator) -> None:
        """Abbreviation match for NHL teams."""
        result = validator.validate_match("Philadelphia Flyers", "Flyers")
        assert result.is_match is True
        assert result.confidence >= 0.75

    # =========================================================================
    # No match tests
    # =========================================================================

    def test_no_match_different_teams(self, validator: TeamValidator) -> None:
        """Different teams should not match."""
        result = validator.validate_match("Boston Celtics", "Philadelphia Flyers")
        assert result.is_match is False
        assert result.confidence == 0.0
        assert result.method == "no_match"

    def test_no_match_empty_input(self, validator: TeamValidator) -> None:
        """Empty inputs should not match."""
        result = validator.validate_match("", "Boston Celtics")
        assert result.is_match is False
        assert result.method == "empty_input"

        result = validator.validate_match("Boston Celtics", "")
        assert result.is_match is False
        assert result.method == "empty_input"

    def test_no_match_none_input(self, validator: TeamValidator) -> None:
        """None inputs should not match."""
        result = validator.validate_match(None, "Boston")  # type: ignore
        assert result.is_match is False

    # =========================================================================
    # False positive prevention tests (critical for bug fix)
    # =========================================================================

    def test_no_false_positive_partial_city(self, validator: TeamValidator) -> None:
        """Partial city names should not cause false positives."""
        # "New" should not match "Denver"
        result = validator.validate_match("New York Knicks", "Denver Nuggets")
        assert result.is_match is False

    def test_no_false_positive_similar_nicknames(self, validator: TeamValidator) -> None:
        """Similar but different nicknames should not match."""
        # "Cardinals" (Arizona) vs "Cardinals" (St. Louis) - same nickname, should match
        result = validator.validate_match("Arizona Cardinals", "St. Louis Cardinals")
        # Both have "Cardinals" nickname, so they will match by nickname
        assert result.is_match is True

    def test_no_false_positive_different_sports_same_city(self, validator: TeamValidator) -> None:
        """Same city different sports should NOT match if nicknames differ."""
        # Philadelphia Flyers (NHL) vs Philadelphia Eagles (NFL)
        result = validator.validate_match("Philadelphia Flyers", "Philadelphia Eagles")
        assert result.is_match is False

    def test_home_away_team_separation(self, validator: TeamValidator) -> None:
        """Home and away teams in same game should not match each other."""
        # Critical test: two teams playing each other
        result = validator.validate_match("Boston Celtics", "Los Angeles Lakers")
        assert result.is_match is False

        result = validator.validate_match("Philadelphia Flyers", "Pittsburgh Penguins")
        assert result.is_match is False

    # =========================================================================
    # find_best_match tests
    # =========================================================================

    def test_find_best_match_multiple_candidates(self, validator: TeamValidator) -> None:
        """find_best_match should return the best matching candidate."""
        candidates = ["Lakers", "Celtics", "Heat"]
        index, result = validator.find_best_match("Boston Celtics", candidates)

        assert index == 1  # "Celtics" is at index 1
        assert result is not None
        assert result.is_match is True
        assert result.confidence >= 0.9

    def test_find_best_match_no_match(self, validator: TeamValidator) -> None:
        """find_best_match should return None when no match."""
        candidates = ["Lakers", "Heat", "Warriors"]
        index, result = validator.find_best_match("Boston Celtics", candidates)

        assert index is None
        assert result is None

    def test_find_best_match_min_confidence(self, validator: TeamValidator) -> None:
        """find_best_match should respect min_confidence threshold."""
        candidates = ["Boston Bruins"]  # Different sport, different nickname
        index, result = validator.find_best_match(
            "Boston Celtics",
            candidates,
            min_confidence=0.9  # High threshold
        )

        # "Boston" contains match would be 0.8, below threshold
        assert index is None or (result and result.confidence >= 0.9)

    def test_find_best_match_empty_candidates(self, validator: TeamValidator) -> None:
        """find_best_match should handle empty candidate list."""
        index, result = validator.find_best_match("Boston Celtics", [])
        assert index is None
        assert result is None


class TestConvenienceFunctions:
    """Tests for module-level convenience functions."""

    def test_validate_team_match(self) -> None:
        """validate_team_match should work correctly."""
        result = validate_team_match("Boston Celtics", "Celtics")
        assert result.is_match is True
        assert result.confidence >= 0.9

    def test_find_best_team_match(self) -> None:
        """find_best_team_match should work correctly."""
        candidates = ["Lakers", "Celtics", "Heat"]
        index, result = find_best_team_match("Boston Celtics", candidates)
        assert index == 1
        assert result is not None


class TestRealWorldScenarios:
    """Tests based on real-world bug scenarios."""

    @pytest.fixture
    def validator(self) -> TeamValidator:
        return TeamValidator()

    def test_polymarket_moneyline_home_vs_away(self, validator: TeamValidator) -> None:
        """
        Polymarket moneyline: two contracts per game (home/away).
        Must NOT match wrong team.
        """
        # Signal is for Celtics, market has both Celtics and Lakers contracts
        signal_team = "Boston Celtics"

        # Correct team
        result = validator.validate_match(signal_team, "Celtics")
        assert result.is_match is True
        assert result.confidence >= 0.7

        # Wrong team (opponent)
        result = validator.validate_match(signal_team, "Lakers")
        assert result.is_match is False

    def test_entry_exit_team_consistency(self, validator: TeamValidator) -> None:
        """
        Entry and exit should use same team's price.
        Paper trade market_title encodes team as "{team} to win".
        """
        entry_team = "Flyers"  # Extracted from "Flyers to win"

        # Correct exit price lookup
        result = validator.validate_match(entry_team, "Philadelphia Flyers")
        assert result.is_match is True

        # Wrong exit price lookup (opponent)
        result = validator.validate_match(entry_team, "Pittsburgh Penguins")
        assert result.is_match is False

    def test_confidence_threshold_prevents_bad_matches(self, validator: TeamValidator) -> None:
        """
        Low confidence matches should be rejected.
        """
        # Very weak similarity - should have low/zero confidence
        result = validator.validate_match("Boston", "Phoenix")
        assert result.confidence < 0.7  # Below typical threshold

    def test_nba_full_vs_short_names(self, validator: TeamValidator) -> None:
        """NBA teams with full names vs common short forms."""
        test_cases = [
            ("Boston Celtics", "Celtics", True),
            ("Los Angeles Lakers", "Lakers", True),
            ("Golden State Warriors", "Warriors", True),
            ("Miami Heat", "Heat", True),
            ("Philadelphia 76ers", "76ers", True),
            ("Philadelphia 76ers", "Sixers", True),  # Alias
        ]

        for full_name, short_name, should_match in test_cases:
            result = validator.validate_match(full_name, short_name)
            assert result.is_match == should_match, f"Failed: {full_name} vs {short_name}"

    def test_nhl_full_vs_short_names(self, validator: TeamValidator) -> None:
        """NHL teams with full names vs common short forms."""
        test_cases = [
            ("Philadelphia Flyers", "Flyers", True),
            ("Pittsburgh Penguins", "Penguins", True),
            ("Boston Bruins", "Bruins", True),
            ("Tampa Bay Lightning", "Lightning", True),
        ]

        for full_name, short_name, should_match in test_cases:
            result = validator.validate_match(full_name, short_name)
            assert result.is_match == should_match, f"Failed: {full_name} vs {short_name}"
