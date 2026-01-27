-- Migration 022: P3 Backlog Fixes
-- P3-1: Recreate Continuous Aggregates with contract_team
-- P3-2: Add Audit Triggers for Bankroll/Trade Updates
-- P3-6: Add Deletion Audit Table

-- =============================================================================
-- P3-1: RECREATE CONTINUOUS AGGREGATES WITH contract_team
-- =============================================================================

-- First, remove the existing policies (must be done before dropping views)
SELECT remove_continuous_aggregate_policy('market_prices_hourly', if_exists => true);
SELECT remove_continuous_aggregate_policy('trading_performance_daily', if_exists => true);

-- Drop existing continuous aggregates
DROP MATERIALIZED VIEW IF EXISTS market_prices_hourly CASCADE;
DROP MATERIALIZED VIEW IF EXISTS trading_performance_daily CASCADE;

-- Recreate market_prices_hourly WITH contract_team
CREATE MATERIALIZED VIEW market_prices_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    market_id,
    platform,
    game_id,
    contract_team,
    AVG(yes_bid) AS avg_yes_bid,
    AVG(yes_ask) AS avg_yes_ask,
    AVG((yes_ask - yes_bid) * 100) AS avg_spread,
    SUM(volume) AS total_volume,
    COUNT(*) AS sample_count
FROM market_prices
GROUP BY bucket, market_id, platform, game_id, contract_team;

-- Recreate trading_performance_daily (unchanged but recreating for consistency)
CREATE MATERIALIZED VIEW trading_performance_daily
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 day', time) AS bucket,
    signal_type,
    sport,
    COUNT(*) AS total_trades,
    SUM(CASE WHEN outcome = 'win' THEN 1 ELSE 0 END) AS winning_trades,
    SUM(CASE WHEN outcome = 'loss' THEN 1 ELSE 0 END) AS losing_trades,
    SUM(pnl) AS total_pnl,
    AVG(pnl) AS avg_pnl,
    AVG(edge_at_entry) AS avg_edge
FROM paper_trades
WHERE status = 'closed'
GROUP BY bucket, signal_type, sport;

-- Re-add continuous aggregate policies
SELECT add_continuous_aggregate_policy('market_prices_hourly',
    start_offset => INTERVAL '3 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour');

SELECT add_continuous_aggregate_policy('trading_performance_daily',
    start_offset => INTERVAL '3 days',
    end_offset => INTERVAL '1 day',
    schedule_interval => INTERVAL '1 day');

-- =============================================================================
-- P3-6: DELETION AUDIT TABLE
-- =============================================================================

-- Table to track all deletions for audit purposes
CREATE TABLE IF NOT EXISTS deletion_audit (
    id BIGSERIAL PRIMARY KEY,
    table_name VARCHAR(64) NOT NULL,
    record_id VARCHAR(256),
    deleted_data JSONB,
    deleted_at TIMESTAMPTZ DEFAULT NOW(),
    deleted_by VARCHAR(128) DEFAULT current_user,
    deletion_reason VARCHAR(256)
);

CREATE INDEX IF NOT EXISTS idx_deletion_audit_table ON deletion_audit(table_name, deleted_at DESC);
CREATE INDEX IF NOT EXISTS idx_deletion_audit_time ON deletion_audit(deleted_at DESC);

-- =============================================================================
-- P3-2: AUDIT TRIGGERS FOR BANKROLL AND TRADE UPDATES
-- =============================================================================

-- Bankroll audit table
CREATE TABLE IF NOT EXISTS bankroll_audit (
    id BIGSERIAL PRIMARY KEY,
    operation VARCHAR(10) NOT NULL,  -- INSERT, UPDATE, DELETE
    changed_at TIMESTAMPTZ DEFAULT NOW(),
    changed_by VARCHAR(128) DEFAULT current_user,
    old_balance DECIMAL(15, 2),
    new_balance DECIMAL(15, 2),
    old_piggybank DECIMAL(15, 2),
    new_piggybank DECIMAL(15, 2),
    old_version INTEGER,
    new_version INTEGER,
    change_source VARCHAR(128)  -- e.g., 'position_tracker', 'signal_processor'
);

CREATE INDEX IF NOT EXISTS idx_bankroll_audit_time ON bankroll_audit(changed_at DESC);

-- Paper trades audit table
CREATE TABLE IF NOT EXISTS paper_trades_audit (
    id BIGSERIAL PRIMARY KEY,
    operation VARCHAR(10) NOT NULL,
    changed_at TIMESTAMPTZ DEFAULT NOW(),
    changed_by VARCHAR(128) DEFAULT current_user,
    trade_id VARCHAR(128),
    old_status VARCHAR(32),
    new_status VARCHAR(32),
    old_pnl DECIMAL(15, 2),
    new_pnl DECIMAL(15, 2),
    old_exit_price DECIMAL(10, 6),
    new_exit_price DECIMAL(10, 6),
    change_reason VARCHAR(256)
);

CREATE INDEX IF NOT EXISTS idx_paper_trades_audit_time ON paper_trades_audit(changed_at DESC);
CREATE INDEX IF NOT EXISTS idx_paper_trades_audit_trade ON paper_trades_audit(trade_id);

-- =============================================================================
-- AUDIT TRIGGER FUNCTIONS
-- =============================================================================

-- Bankroll audit trigger function
CREATE OR REPLACE FUNCTION audit_bankroll_changes()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO bankroll_audit (
            operation, new_balance, new_piggybank, new_version
        ) VALUES (
            'INSERT', NEW.balance, NEW.piggybank_balance, NEW.version
        );
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        -- Only log if values actually changed
        IF OLD.balance IS DISTINCT FROM NEW.balance
           OR OLD.piggybank_balance IS DISTINCT FROM NEW.piggybank_balance
           OR OLD.version IS DISTINCT FROM NEW.version THEN
            INSERT INTO bankroll_audit (
                operation,
                old_balance, new_balance,
                old_piggybank, new_piggybank,
                old_version, new_version
            ) VALUES (
                'UPDATE',
                OLD.balance, NEW.balance,
                OLD.piggybank_balance, NEW.piggybank_balance,
                OLD.version, NEW.version
            );
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        INSERT INTO bankroll_audit (
            operation, old_balance, old_piggybank, old_version
        ) VALUES (
            'DELETE', OLD.balance, OLD.piggybank_balance, OLD.version
        );
        RETURN OLD;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Paper trades audit trigger function
CREATE OR REPLACE FUNCTION audit_paper_trades_changes()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO paper_trades_audit (
            operation, trade_id, new_status, new_pnl, new_exit_price
        ) VALUES (
            'INSERT', NEW.trade_id, NEW.status, NEW.pnl, NEW.exit_price
        );
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        -- Only log status/pnl/exit_price changes
        IF OLD.status IS DISTINCT FROM NEW.status
           OR OLD.pnl IS DISTINCT FROM NEW.pnl
           OR OLD.exit_price IS DISTINCT FROM NEW.exit_price THEN
            INSERT INTO paper_trades_audit (
                operation, trade_id,
                old_status, new_status,
                old_pnl, new_pnl,
                old_exit_price, new_exit_price
            ) VALUES (
                'UPDATE', NEW.trade_id,
                OLD.status, NEW.status,
                OLD.pnl, NEW.pnl,
                OLD.exit_price, NEW.exit_price
            );
        END IF;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        INSERT INTO paper_trades_audit (
            operation, trade_id, old_status, old_pnl, old_exit_price
        ) VALUES (
            'DELETE', OLD.trade_id, OLD.status, OLD.pnl, OLD.exit_price
        );
        -- Also log to deletion_audit for full record
        INSERT INTO deletion_audit (
            table_name, record_id, deleted_data
        ) VALUES (
            'paper_trades', OLD.trade_id, row_to_json(OLD)::jsonb
        );
        RETURN OLD;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Create the triggers
DROP TRIGGER IF EXISTS trg_bankroll_audit ON bankroll;
CREATE TRIGGER trg_bankroll_audit
    AFTER INSERT OR UPDATE OR DELETE ON bankroll
    FOR EACH ROW EXECUTE FUNCTION audit_bankroll_changes();

DROP TRIGGER IF EXISTS trg_paper_trades_audit ON paper_trades;
CREATE TRIGGER trg_paper_trades_audit
    AFTER INSERT OR UPDATE OR DELETE ON paper_trades
    FOR EACH ROW EXECUTE FUNCTION audit_paper_trades_changes();

-- =============================================================================
-- P3-5: RETENTION POLICY MONITORING TABLE
-- =============================================================================

-- Table to track retention policy executions
CREATE TABLE IF NOT EXISTS retention_policy_log (
    id BIGSERIAL PRIMARY KEY,
    executed_at TIMESTAMPTZ DEFAULT NOW(),
    table_name VARCHAR(64) NOT NULL,
    rows_deleted BIGINT,
    oldest_remaining TIMESTAMPTZ,
    execution_time_ms INTEGER,
    status VARCHAR(32) DEFAULT 'success',
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_retention_log_time ON retention_policy_log(executed_at DESC);
CREATE INDEX IF NOT EXISTS idx_retention_log_table ON retention_policy_log(table_name);

-- Function to log retention policy execution
CREATE OR REPLACE FUNCTION log_retention_execution(
    p_table_name VARCHAR,
    p_rows_deleted BIGINT,
    p_oldest_remaining TIMESTAMPTZ,
    p_execution_time_ms INTEGER
)
RETURNS void AS $$
BEGIN
    INSERT INTO retention_policy_log (
        table_name, rows_deleted, oldest_remaining, execution_time_ms
    ) VALUES (
        p_table_name, p_rows_deleted, p_oldest_remaining, p_execution_time_ms
    );
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- HELPER VIEWS FOR MONITORING
-- =============================================================================

-- View to check retention policy status
CREATE OR REPLACE VIEW v_retention_status AS
SELECT
    ht.table_name,
    rpl.last_execution,
    rpl.last_rows_deleted,
    rpl.total_rows_deleted_30d
FROM (
    SELECT DISTINCT hypertable_name AS table_name
    FROM timescaledb_information.hypertables
) ht
LEFT JOIN LATERAL (
    SELECT
        MAX(executed_at) AS last_execution,
        (SELECT rows_deleted FROM retention_policy_log r2
         WHERE r2.table_name = ht.table_name
         ORDER BY executed_at DESC LIMIT 1) AS last_rows_deleted,
        SUM(rows_deleted) AS total_rows_deleted_30d
    FROM retention_policy_log
    WHERE table_name = ht.table_name
      AND executed_at > NOW() - INTERVAL '30 days'
) rpl ON true;

-- View to check audit log summary
CREATE OR REPLACE VIEW v_audit_summary AS
SELECT
    'bankroll' AS table_name,
    COUNT(*) AS total_changes,
    COUNT(*) FILTER (WHERE operation = 'UPDATE') AS updates,
    MIN(changed_at) AS first_change,
    MAX(changed_at) AS last_change
FROM bankroll_audit
UNION ALL
SELECT
    'paper_trades' AS table_name,
    COUNT(*) AS total_changes,
    COUNT(*) FILTER (WHERE operation = 'UPDATE') AS updates,
    MIN(changed_at) AS first_change,
    MAX(changed_at) AS last_change
FROM paper_trades_audit;

-- =============================================================================
-- COMMENTS
-- =============================================================================

COMMENT ON TABLE deletion_audit IS 'P3-6: Tracks all record deletions for audit purposes';
COMMENT ON TABLE bankroll_audit IS 'P3-2: Tracks all bankroll balance changes';
COMMENT ON TABLE paper_trades_audit IS 'P3-2: Tracks paper trade status/pnl changes';
COMMENT ON TABLE retention_policy_log IS 'P3-5: Tracks retention policy executions';
-- Note: Cannot comment on continuous aggregates (they're not regular materialized views)
