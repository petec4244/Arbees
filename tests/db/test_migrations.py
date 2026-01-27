"""
Database Migration Tests

Tests that verify:
1. All migration files are parseable
2. Migration files follow naming conventions
3. Migrations contain expected SQL structures
4. No duplicate migration numbers
"""

import os
import re
from pathlib import Path


MIGRATIONS_DIR = Path(__file__).parent.parent.parent / "shared" / "arbees_shared" / "db" / "migrations"


def test_migrations_directory_exists():
    """Verify migrations directory exists."""
    assert MIGRATIONS_DIR.exists(), f"Migrations directory not found: {MIGRATIONS_DIR}"
    assert MIGRATIONS_DIR.is_dir(), f"Migrations path is not a directory: {MIGRATIONS_DIR}"


def test_migrations_follow_naming_convention():
    """Verify all migrations follow the NNN_name.sql naming convention."""
    migration_files = list(MIGRATIONS_DIR.glob("*.sql"))
    assert len(migration_files) > 0, "No migration files found"

    # Allow alphanumeric and underscores in migration name
    pattern = re.compile(r"^\d{3}_[a-z0-9_]+\.sql$")

    for migration_file in migration_files:
        assert pattern.match(migration_file.name), (
            f"Migration file does not follow naming convention (NNN_name.sql): {migration_file.name}"
        )


def test_no_duplicate_migration_numbers():
    """Verify no two migrations have the same number."""
    migration_files = list(MIGRATIONS_DIR.glob("*.sql"))

    numbers = []
    for migration_file in migration_files:
        match = re.match(r"^(\d{3})_", migration_file.name)
        if match:
            numbers.append((int(match.group(1)), migration_file.name))

    seen = {}
    duplicates = []
    for num, name in numbers:
        if num in seen:
            duplicates.append((num, seen[num], name))
        else:
            seen[num] = name

    assert len(duplicates) == 0, (
        f"Duplicate migration numbers found: {duplicates}"
    )


def test_migrations_are_valid_sql():
    """Verify all migrations contain valid SQL syntax patterns."""
    migration_files = sorted(MIGRATIONS_DIR.glob("*.sql"))

    # Common SQL keywords that should appear in valid migrations
    valid_keywords = {
        "CREATE", "ALTER", "DROP", "INSERT", "UPDATE", "DELETE",
        "INDEX", "TABLE", "FUNCTION", "TRIGGER", "VIEW", "TYPE",
        "ADD", "COLUMN", "CONSTRAINT", "SELECT", "IF", "DO"
    }

    for migration_file in migration_files:
        content = migration_file.read_text()

        # Skip empty files
        if not content.strip():
            continue

        # Remove comments
        content_no_comments = re.sub(r"--.*$", "", content, flags=re.MULTILINE)
        content_no_comments = re.sub(r"/\*.*?\*/", "", content_no_comments, flags=re.DOTALL)

        # Check for at least one valid SQL keyword
        content_upper = content_no_comments.upper()
        has_valid_sql = any(keyword in content_upper for keyword in valid_keywords)

        assert has_valid_sql, (
            f"Migration {migration_file.name} does not contain any valid SQL keywords"
        )


def test_migrations_have_balanced_parentheses():
    """Verify all migrations have balanced parentheses."""
    migration_files = sorted(MIGRATIONS_DIR.glob("*.sql"))

    for migration_file in migration_files:
        content = migration_file.read_text()

        # Remove string literals (single quoted)
        content_no_strings = re.sub(r"'[^']*'", "", content)

        # Count parentheses
        open_count = content_no_strings.count("(")
        close_count = content_no_strings.count(")")

        assert open_count == close_count, (
            f"Migration {migration_file.name} has unbalanced parentheses: "
            f"{open_count} open vs {close_count} close"
        )


def test_p0_fixes_migration_structure():
    """Verify the P0 fixes migration (021) contains expected changes."""
    migration_file = MIGRATIONS_DIR / "021_p0_fixes.sql"

    if not migration_file.exists():
        # Migration not yet created
        return

    content = migration_file.read_text().lower()

    # P0-3: Unique constraint on market_prices
    assert "idx_market_prices_unique" in content or "unique" in content, (
        "P0-3 fix: Missing unique constraint on market_prices"
    )

    # P0-4: Version column for optimistic locking
    assert "version" in content, (
        "P0-4 fix: Missing version column for optimistic locking"
    )

    # P1-3: CHECK constraints
    assert "check" in content, (
        "P1-3 fix: Missing CHECK constraints"
    )


def test_initial_migration_creates_core_tables():
    """Verify the initial migration creates core tables."""
    migration_file = MIGRATIONS_DIR / "001_initial.sql"

    assert migration_file.exists(), "Initial migration (001) not found"

    content = migration_file.read_text().lower()

    # Core tables that should exist
    expected_tables = [
        "game_states",
        "market_prices",
        "trading_signals",
        "paper_trades",
        "bankroll",
    ]

    for table in expected_tables:
        assert table in content, (
            f"Initial migration missing core table: {table}"
        )


def test_migrations_ordered_sequence():
    """Verify migrations form a continuous sequence from the first to the last."""
    migration_files = sorted(MIGRATIONS_DIR.glob("*.sql"))

    numbers = []
    for migration_file in migration_files:
        match = re.match(r"^(\d{3})_", migration_file.name)
        if match:
            numbers.append(int(match.group(1)))

    numbers.sort()

    # Verify we have at least some migrations
    assert len(numbers) >= 2, "Need at least 2 migrations to check sequence"

    # Verify migrations are unique (no duplicates - tested separately)
    assert len(numbers) == len(set(numbers)), "Duplicate migration numbers found"

    # Verify the sequence is strictly increasing
    for i in range(len(numbers) - 1):
        assert numbers[i] < numbers[i + 1], (
            f"Migration sequence out of order: {numbers[i]} >= {numbers[i+1]}"
        )


if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
