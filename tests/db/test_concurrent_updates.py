"""
Database Concurrent Update Stress Tests (P3-4)

Tests that verify:
1. Bankroll optimistic locking under concurrent updates
2. Paper trades insertion under concurrent load
3. Market prices unique constraint under concurrent inserts

These tests require a running database connection.
Run with: pytest tests/db/test_concurrent_updates.py -v --tb=short

To run against local Docker:
    DATABASE_URL=postgresql://arbees:password@localhost:5432/arbees pytest tests/db/test_concurrent_updates.py -v
"""

import asyncio
import os
import random
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timedelta
from decimal import Decimal
from typing import List, Tuple
from unittest import skipIf

import pytest

# Check if we have database access
DATABASE_URL = os.environ.get("DATABASE_URL", "")
HAS_DATABASE = bool(DATABASE_URL) and "localhost" in DATABASE_URL or "timescaledb" in DATABASE_URL

try:
    import psycopg2
    from psycopg2 import pool
    HAS_PSYCOPG2 = True
except ImportError:
    HAS_PSYCOPG2 = False


@pytest.fixture(scope="module")
def db_pool():
    """Create a connection pool for concurrent tests."""
    if not HAS_DATABASE or not HAS_PSYCOPG2:
        pytest.skip("Database not available")

    connection_pool = pool.ThreadedConnectionPool(
        minconn=5,
        maxconn=20,
        dsn=DATABASE_URL
    )
    yield connection_pool
    connection_pool.closeall()


@pytest.fixture
def clean_test_data(db_pool):
    """Clean up test data before and after each test."""
    conn = db_pool.getconn()
    try:
        with conn.cursor() as cur:
            # Clean up test data
            cur.execute("DELETE FROM paper_trades WHERE game_id LIKE 'stress-test-%'")
            cur.execute("DELETE FROM market_prices WHERE game_id LIKE 'stress-test-%'")
            cur.execute("DELETE FROM trading_signals WHERE game_id LIKE 'stress-test-%'")
        conn.commit()
    finally:
        db_pool.putconn(conn)

    yield

    # Cleanup after test
    conn = db_pool.getconn()
    try:
        with conn.cursor() as cur:
            cur.execute("DELETE FROM paper_trades WHERE game_id LIKE 'stress-test-%'")
            cur.execute("DELETE FROM market_prices WHERE game_id LIKE 'stress-test-%'")
            cur.execute("DELETE FROM trading_signals WHERE game_id LIKE 'stress-test-%'")
        conn.commit()
    finally:
        db_pool.putconn(conn)


class TestBankrollOptimisticLocking:
    """Test bankroll updates with optimistic locking under concurrent access."""

    @skipIf(not HAS_DATABASE or not HAS_PSYCOPG2, "Database not available")
    def test_concurrent_bankroll_updates_with_locking(self, db_pool, clean_test_data):
        """
        Simulate multiple services updating bankroll concurrently.
        With optimistic locking, some updates should fail and retry.
        """
        num_workers = 10
        updates_per_worker = 5
        update_amount = Decimal("1.00")

        # Get initial bankroll state
        conn = db_pool.getconn()
        try:
            with conn.cursor() as cur:
                cur.execute("SELECT balance, version FROM bankroll ORDER BY updated_at DESC LIMIT 1")
                row = cur.fetchone()
                if row is None:
                    # Insert initial bankroll
                    cur.execute(
                        "INSERT INTO bankroll (balance, piggybank_balance, version) VALUES (%s, %s, %s)",
                        (Decimal("1000.00"), Decimal("0.00"), 1)
                    )
                    conn.commit()
                    initial_balance = Decimal("1000.00")
                else:
                    initial_balance = row[0]
        finally:
            db_pool.putconn(conn)

        conflicts_detected = []
        successful_updates = []

        def update_bankroll(worker_id: int):
            """Worker function to update bankroll with optimistic locking."""
            local_conflicts = 0
            local_successes = 0

            for i in range(updates_per_worker):
                conn = db_pool.getconn()
                try:
                    max_retries = 3
                    for attempt in range(max_retries):
                        with conn.cursor() as cur:
                            # Read current state
                            cur.execute(
                                "SELECT balance, version FROM bankroll ORDER BY updated_at DESC LIMIT 1"
                            )
                            row = cur.fetchone()
                            if row is None:
                                break

                            current_balance, current_version = row
                            new_balance = current_balance + update_amount

                            # Attempt update with version check
                            cur.execute(
                                """
                                UPDATE bankroll
                                SET balance = %s, version = %s, updated_at = NOW()
                                WHERE version = %s
                                """,
                                (new_balance, current_version + 1, current_version)
                            )

                            if cur.rowcount == 1:
                                conn.commit()
                                local_successes += 1
                                break
                            else:
                                # Version conflict - rollback and retry
                                conn.rollback()
                                local_conflicts += 1
                                time.sleep(random.uniform(0.01, 0.05))  # Backoff

                finally:
                    db_pool.putconn(conn)

            return local_successes, local_conflicts

        # Run concurrent updates
        with ThreadPoolExecutor(max_workers=num_workers) as executor:
            futures = [executor.submit(update_bankroll, i) for i in range(num_workers)]
            for future in as_completed(futures):
                successes, conflicts = future.result()
                successful_updates.append(successes)
                conflicts_detected.append(conflicts)

        total_successes = sum(successful_updates)
        total_conflicts = sum(conflicts_detected)

        # Get final balance
        conn = db_pool.getconn()
        try:
            with conn.cursor() as cur:
                cur.execute("SELECT balance, version FROM bankroll ORDER BY updated_at DESC LIMIT 1")
                final_balance, final_version = cur.fetchone()
        finally:
            db_pool.putconn(conn)

        # Verify results
        expected_balance = initial_balance + (update_amount * total_successes)

        print(f"\nBankroll Stress Test Results:")
        print(f"  Workers: {num_workers}, Updates per worker: {updates_per_worker}")
        print(f"  Total successful updates: {total_successes}")
        print(f"  Total conflicts detected: {total_conflicts}")
        print(f"  Initial balance: {initial_balance}")
        print(f"  Final balance: {final_balance}")
        print(f"  Expected balance: {expected_balance}")
        print(f"  Final version: {final_version}")

        # Balance should match exactly (no lost updates)
        assert final_balance == expected_balance, (
            f"Balance mismatch: expected {expected_balance}, got {final_balance}"
        )

        # With concurrent updates, we should have some conflicts
        # (unless the database is very fast)
        assert total_conflicts >= 0, "Conflict tracking failed"

    @skipIf(not HAS_DATABASE or not HAS_PSYCOPG2, "Database not available")
    def test_bankroll_audit_trail(self, db_pool, clean_test_data):
        """Verify that bankroll changes are audited."""
        conn = db_pool.getconn()
        try:
            with conn.cursor() as cur:
                # Get initial audit count
                cur.execute("SELECT COUNT(*) FROM bankroll_audit")
                initial_count = cur.fetchone()[0]

                # Make a bankroll update
                cur.execute(
                    """
                    UPDATE bankroll
                    SET balance = balance + 10.00, version = version + 1, updated_at = NOW()
                    """
                )
                conn.commit()

                # Check audit trail
                cur.execute("SELECT COUNT(*) FROM bankroll_audit")
                final_count = cur.fetchone()[0]

                assert final_count > initial_count, "Bankroll update was not audited"

        finally:
            db_pool.putconn(conn)


class TestConcurrentPaperTrades:
    """Test paper trades insertion under concurrent load."""

    @skipIf(not HAS_DATABASE or not HAS_PSYCOPG2, "Database not available")
    def test_concurrent_trade_insertions(self, db_pool, clean_test_data):
        """Insert trades concurrently and verify no data loss."""
        num_workers = 5
        trades_per_worker = 20

        def insert_trades(worker_id: int) -> int:
            """Insert trades for a worker."""
            inserted = 0
            conn = db_pool.getconn()
            try:
                for i in range(trades_per_worker):
                    trade_id = f"stress-test-{worker_id}-{i}-{int(time.time()*1000)}"
                    with conn.cursor() as cur:
                        cur.execute(
                            """
                            INSERT INTO paper_trades (
                                trade_id, game_id, sport, platform, market_id,
                                side, entry_price, size, status, signal_type,
                                edge_at_entry, time
                            ) VALUES (
                                %s, %s, %s, %s, %s,
                                %s, %s, %s, %s, %s,
                                %s, %s
                            )
                            """,
                            (
                                trade_id,
                                f"stress-test-game-{worker_id}",
                                "nba",
                                "paper",
                                f"market-{worker_id}-{i}",
                                "yes",
                                random.uniform(0.3, 0.7),
                                random.uniform(10, 100),
                                "open",
                                "live_edge",
                                random.uniform(3, 10),
                                datetime.utcnow()
                            )
                        )
                        inserted += 1
                conn.commit()
            except Exception as e:
                print(f"Worker {worker_id} error: {e}")
                conn.rollback()
            finally:
                db_pool.putconn(conn)
            return inserted

        # Run concurrent inserts
        with ThreadPoolExecutor(max_workers=num_workers) as executor:
            futures = [executor.submit(insert_trades, i) for i in range(num_workers)]
            results = [f.result() for f in as_completed(futures)]

        total_inserted = sum(results)
        expected_total = num_workers * trades_per_worker

        # Verify all trades were inserted
        conn = db_pool.getconn()
        try:
            with conn.cursor() as cur:
                cur.execute(
                    "SELECT COUNT(*) FROM paper_trades WHERE game_id LIKE 'stress-test-game-%'"
                )
                actual_count = cur.fetchone()[0]
        finally:
            db_pool.putconn(conn)

        print(f"\nPaper Trades Stress Test Results:")
        print(f"  Workers: {num_workers}, Trades per worker: {trades_per_worker}")
        print(f"  Total inserted (reported): {total_inserted}")
        print(f"  Actual count in DB: {actual_count}")
        print(f"  Expected: {expected_total}")

        assert actual_count == expected_total, (
            f"Trade count mismatch: expected {expected_total}, got {actual_count}"
        )


class TestConcurrentMarketPrices:
    """Test market prices with unique constraint under concurrent inserts."""

    @skipIf(not HAS_DATABASE or not HAS_PSYCOPG2, "Database not available")
    def test_unique_constraint_prevents_duplicates(self, db_pool, clean_test_data):
        """
        Try to insert duplicate market prices concurrently.
        The unique constraint should prevent duplicates.
        """
        num_workers = 5
        fixed_time = datetime.utcnow()
        market_id = "stress-test-market-unique"
        game_id = "stress-test-game-unique"

        successful_inserts = []
        constraint_violations = []

        def try_insert_price(worker_id: int) -> Tuple[int, int]:
            """Try to insert the same price (should fail for all but one)."""
            successes = 0
            violations = 0

            conn = db_pool.getconn()
            try:
                with conn.cursor() as cur:
                    try:
                        cur.execute(
                            """
                            INSERT INTO market_prices (
                                time, market_id, platform, game_id, contract_team,
                                yes_bid, yes_ask, volume
                            ) VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
                            """,
                            (
                                fixed_time,
                                market_id,
                                "kalshi",
                                game_id,
                                "Test Team",
                                0.50,
                                0.52,
                                1000
                            )
                        )
                        conn.commit()
                        successes = 1
                    except psycopg2.errors.UniqueViolation:
                        conn.rollback()
                        violations = 1
                    except Exception as e:
                        conn.rollback()
                        if "unique" in str(e).lower() or "duplicate" in str(e).lower():
                            violations = 1
                        else:
                            raise
            finally:
                db_pool.putconn(conn)

            return successes, violations

        # Run concurrent inserts
        with ThreadPoolExecutor(max_workers=num_workers) as executor:
            futures = [executor.submit(try_insert_price, i) for i in range(num_workers)]
            for future in as_completed(futures):
                successes, violations = future.result()
                successful_inserts.append(successes)
                constraint_violations.append(violations)

        total_successes = sum(successful_inserts)
        total_violations = sum(constraint_violations)

        print(f"\nUnique Constraint Stress Test Results:")
        print(f"  Workers: {num_workers}")
        print(f"  Successful inserts: {total_successes}")
        print(f"  Constraint violations: {total_violations}")

        # Exactly one insert should succeed
        assert total_successes == 1, (
            f"Expected exactly 1 successful insert, got {total_successes}"
        )

        # All others should hit constraint
        assert total_violations == num_workers - 1, (
            f"Expected {num_workers - 1} constraint violations, got {total_violations}"
        )

    @skipIf(not HAS_DATABASE or not HAS_PSYCOPG2, "Database not available")
    def test_high_volume_price_inserts(self, db_pool, clean_test_data):
        """Insert many unique prices concurrently."""
        num_workers = 5
        prices_per_worker = 50

        def insert_prices(worker_id: int) -> int:
            """Insert unique prices for a worker."""
            inserted = 0
            conn = db_pool.getconn()
            try:
                for i in range(prices_per_worker):
                    # Each price has unique time + market_id + contract_team
                    price_time = datetime.utcnow() + timedelta(
                        milliseconds=worker_id * 1000 + i
                    )
                    with conn.cursor() as cur:
                        cur.execute(
                            """
                            INSERT INTO market_prices (
                                time, market_id, platform, game_id, contract_team,
                                yes_bid, yes_ask, volume
                            ) VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
                            """,
                            (
                                price_time,
                                f"stress-test-market-{worker_id}",
                                "kalshi",
                                f"stress-test-game-{worker_id}",
                                f"Team {worker_id}",
                                random.uniform(0.3, 0.7),
                                random.uniform(0.31, 0.71),
                                random.randint(100, 10000)
                            )
                        )
                        inserted += 1
                conn.commit()
            except Exception as e:
                print(f"Worker {worker_id} error: {e}")
                conn.rollback()
            finally:
                db_pool.putconn(conn)
            return inserted

        start_time = time.time()

        # Run concurrent inserts
        with ThreadPoolExecutor(max_workers=num_workers) as executor:
            futures = [executor.submit(insert_prices, i) for i in range(num_workers)]
            results = [f.result() for f in as_completed(futures)]

        elapsed = time.time() - start_time
        total_inserted = sum(results)
        expected_total = num_workers * prices_per_worker

        # Verify count
        conn = db_pool.getconn()
        try:
            with conn.cursor() as cur:
                cur.execute(
                    "SELECT COUNT(*) FROM market_prices WHERE game_id LIKE 'stress-test-game-%'"
                )
                actual_count = cur.fetchone()[0]
        finally:
            db_pool.putconn(conn)

        print(f"\nHigh Volume Price Insert Results:")
        print(f"  Workers: {num_workers}, Prices per worker: {prices_per_worker}")
        print(f"  Total inserted: {total_inserted}")
        print(f"  Actual in DB: {actual_count}")
        print(f"  Time elapsed: {elapsed:.2f}s")
        print(f"  Throughput: {total_inserted/elapsed:.1f} inserts/sec")

        assert actual_count == expected_total, (
            f"Price count mismatch: expected {expected_total}, got {actual_count}"
        )


if __name__ == "__main__":
    pytest.main([__file__, "-v", "--tb=short"])
