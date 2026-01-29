-- ============================================================================
-- OPTIONAL: Database-backed idempotency for crash recovery
-- ============================================================================
-- This migration creates a table for persistent idempotency tracking.
-- The in-memory tracker is sufficient for normal operation, but this provides
-- crash recovery by allowing the service to check for recently processed requests.
--
-- Note: This is OPTIONAL. The execution service uses in-memory tracking by default.
-- To enable DB-backed idempotency, set IDEMPOTENCY_USE_DB=true in environment.
-- ============================================================================

CREATE TABLE IF NOT EXISTS execution_idempotency (
    idempotency_key VARCHAR(128) PRIMARY KEY,
    request_id VARCHAR(128) NOT NULL,
    status VARCHAR(32) NOT NULL,
    processed_at TIMESTAMPTZ DEFAULT NOW()
);

-- Index for cleanup queries
CREATE INDEX IF NOT EXISTS idx_exec_idemp_processed
ON execution_idempotency (processed_at);

-- Auto-delete entries after 24 hours (if TimescaleDB retention policies available)
-- This keeps the table small while still providing crash recovery window
DO $$
BEGIN
    -- Try to add TimescaleDB retention policy
    PERFORM add_retention_policy('execution_idempotency',
        INTERVAL '24 hours', if_not_exists => TRUE);
    RAISE NOTICE 'Retention policy added for execution_idempotency';
EXCEPTION
    WHEN undefined_function THEN
        -- Not a hypertable or TimescaleDB not available
        RAISE NOTICE 'TimescaleDB retention not available, using manual cleanup';
    WHEN others THEN
        RAISE NOTICE 'Retention policy error: %', SQLERRM;
END $$;

-- Create function for manual cleanup (fallback for non-TimescaleDB)
CREATE OR REPLACE FUNCTION cleanup_old_idempotency_entries()
RETURNS INTEGER AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    DELETE FROM execution_idempotency
    WHERE processed_at < NOW() - INTERVAL '24 hours';

    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END;
$$ LANGUAGE plpgsql;

-- Comment for documentation
COMMENT ON TABLE execution_idempotency IS
    'Tracks processed execution requests for idempotency (crash recovery). Auto-cleaned after 24h.';
COMMENT ON FUNCTION cleanup_old_idempotency_entries() IS
    'Call periodically to clean up old idempotency entries if not using TimescaleDB retention.';
