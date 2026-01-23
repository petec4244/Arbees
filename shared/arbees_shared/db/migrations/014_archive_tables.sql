-- ================================================
-- Migration 014: Archive Tables for Game Lifecycle Management
-- Enables historical game analysis and ML performance tracking
-- ================================================

-- Archived games with computed summary statistics
CREATE TABLE IF NOT EXISTS archived_games (
    archive_id SERIAL PRIMARY KEY,
    game_id VARCHAR(64) NOT NULL UNIQUE,
    sport VARCHAR(20) NOT NULL,
    home_team VARCHAR(128) NOT NULL,
    away_team VARCHAR(128) NOT NULL,
    final_home_score INTEGER NOT NULL DEFAULT 0,
    final_away_score INTEGER NOT NULL DEFAULT 0,
    scheduled_time TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Computed summary stats
    total_trades INTEGER NOT NULL DEFAULT 0,
    winning_trades INTEGER NOT NULL DEFAULT 0,
    losing_trades INTEGER NOT NULL DEFAULT 0,
    push_trades INTEGER NOT NULL DEFAULT 0,
    total_pnl DECIMAL(12,2) NOT NULL DEFAULT 0,
    total_signals_generated INTEGER NOT NULL DEFAULT 0,
    total_signals_executed INTEGER NOT NULL DEFAULT 0,
    avg_edge_pct DECIMAL(6,4)
);

-- Performance computed columns (done in application layer for Postgres <12 compatibility)
CREATE INDEX IF NOT EXISTS idx_archived_games_sport ON archived_games(sport);
CREATE INDEX IF NOT EXISTS idx_archived_games_ended_at ON archived_games(ended_at DESC);
CREATE INDEX IF NOT EXISTS idx_archived_games_pnl ON archived_games(total_pnl DESC);
CREATE INDEX IF NOT EXISTS idx_archived_games_game_id ON archived_games(game_id);

-- Partial indexes for filtering
CREATE INDEX IF NOT EXISTS idx_archived_games_profitable ON archived_games(sport, ended_at DESC)
    WHERE total_pnl > 0;
CREATE INDEX IF NOT EXISTS idx_archived_games_losing ON archived_games(sport, ended_at DESC)
    WHERE total_pnl < 0;

-- Archived trades (historical copy from paper_trades)
CREATE TABLE IF NOT EXISTS archived_trades (
    archive_trade_id SERIAL PRIMARY KEY,
    trade_id VARCHAR(128) NOT NULL,
    archive_game_id INTEGER REFERENCES archived_games(archive_id) ON DELETE CASCADE,
    game_id VARCHAR(64) NOT NULL,
    signal_id VARCHAR(128),
    signal_type VARCHAR(50),

    -- Trade details
    platform VARCHAR(20) NOT NULL,
    market_id VARCHAR(256),
    market_type VARCHAR(20) DEFAULT 'moneyline',
    market_title TEXT,
    side VARCHAR(10) NOT NULL,  -- buy or sell
    team VARCHAR(128),

    -- Pricing
    entry_price DECIMAL(8,4) NOT NULL,
    exit_price DECIMAL(8,4),
    size DECIMAL(12,4) NOT NULL,

    -- Timing
    opened_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,

    -- Outcome
    status VARCHAR(20) NOT NULL,
    outcome VARCHAR(20),
    pnl DECIMAL(12,4),
    pnl_pct DECIMAL(8,4),

    -- Context at entry time (for ML analysis)
    edge_at_entry DECIMAL(6,4),
    model_prob_at_entry DECIMAL(6,4),
    market_prob_at_entry DECIMAL(6,4),
    game_period_at_entry VARCHAR(20),
    score_diff_at_entry INTEGER,
    time_remaining_at_entry INTEGER  -- seconds
);

CREATE INDEX IF NOT EXISTS idx_archived_trades_game ON archived_trades(archive_game_id);
CREATE INDEX IF NOT EXISTS idx_archived_trades_game_id ON archived_trades(game_id);
CREATE INDEX IF NOT EXISTS idx_archived_trades_outcome ON archived_trades(outcome);
CREATE INDEX IF NOT EXISTS idx_archived_trades_signal_type ON archived_trades(signal_type);
CREATE INDEX IF NOT EXISTS idx_archived_trades_opened_at ON archived_trades(opened_at DESC);

-- Index for ML analysis queries
CREATE INDEX IF NOT EXISTS idx_archived_trades_ml ON archived_trades(
    signal_type, outcome, game_period_at_entry
);

-- Archived signals (historical copy from trading_signals)
CREATE TABLE IF NOT EXISTS archived_signals (
    archive_signal_id SERIAL PRIMARY KEY,
    signal_id VARCHAR(128) NOT NULL,
    archive_game_id INTEGER REFERENCES archived_games(archive_id) ON DELETE CASCADE,
    game_id VARCHAR(64) NOT NULL,

    -- Signal details
    signal_type VARCHAR(50) NOT NULL,
    direction VARCHAR(10) NOT NULL,
    team VARCHAR(128),
    market_type VARCHAR(20),

    -- Probabilities
    model_prob DECIMAL(6,4),
    market_prob DECIMAL(6,4),
    edge_pct DECIMAL(6,4) NOT NULL,
    confidence DECIMAL(5,4),
    reason TEXT,

    -- Timing
    generated_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,

    -- Execution tracking
    was_executed BOOLEAN NOT NULL DEFAULT FALSE,
    execution_reason VARCHAR(200),  -- why executed or why skipped

    -- Context at signal time
    game_period VARCHAR(20),
    score_diff INTEGER,
    time_remaining INTEGER
);

CREATE INDEX IF NOT EXISTS idx_archived_signals_game ON archived_signals(archive_game_id);
CREATE INDEX IF NOT EXISTS idx_archived_signals_game_id ON archived_signals(game_id);
CREATE INDEX IF NOT EXISTS idx_archived_signals_executed ON archived_signals(was_executed);
CREATE INDEX IF NOT EXISTS idx_archived_signals_type ON archived_signals(signal_type);

-- ML analysis reports (nightly hot wash)
CREATE TABLE IF NOT EXISTS ml_analysis_reports (
    report_id SERIAL PRIMARY KEY,
    report_date DATE NOT NULL UNIQUE,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Summary stats
    total_games INTEGER NOT NULL DEFAULT 0,
    total_trades INTEGER NOT NULL DEFAULT 0,
    total_pnl DECIMAL(12,2) NOT NULL DEFAULT 0,
    win_rate DECIMAL(5,4),

    -- Top performers
    best_sport VARCHAR(20),
    best_sport_win_rate DECIMAL(5,4),
    best_market_type VARCHAR(20),
    best_market_type_win_rate DECIMAL(5,4),
    best_edge_range VARCHAR(20),

    -- Weaknesses
    worst_sport VARCHAR(20),
    worst_sport_win_rate DECIMAL(5,4),
    worst_market_type VARCHAR(20),
    worst_market_type_win_rate DECIMAL(5,4),

    -- Opportunities
    signals_generated INTEGER NOT NULL DEFAULT 0,
    signals_executed INTEGER NOT NULL DEFAULT 0,
    missed_opportunity_reasons JSONB,

    -- Recommendations (array of objects)
    recommendations JSONB,

    -- Model metrics
    model_accuracy DECIMAL(5,4),
    feature_importance JSONB,

    -- Full report content
    report_markdown TEXT,
    report_html TEXT
);

CREATE INDEX IF NOT EXISTS idx_ml_reports_date ON ml_analysis_reports(report_date DESC);

-- Parameter history (track changes over time for ML tuning)
CREATE TABLE IF NOT EXISTS parameter_history (
    history_id SERIAL PRIMARY KEY,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    parameter_name VARCHAR(100) NOT NULL,
    old_value DECIMAL(12,4),
    new_value DECIMAL(12,4) NOT NULL,
    change_reason VARCHAR(200),
    recommended_by VARCHAR(50),  -- 'ML' or 'manual'
    approved_by VARCHAR(100),
    applied BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_param_history_name ON parameter_history(parameter_name, changed_at DESC);

-- Add archive status to games table
ALTER TABLE games ADD COLUMN IF NOT EXISTS archived BOOLEAN DEFAULT FALSE;
ALTER TABLE games ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

-- Index for finding games to archive
CREATE INDEX IF NOT EXISTS idx_games_archive_status ON games(status, archived)
    WHERE status IN ('final', 'complete') AND archived = FALSE;

-- ================================================
-- Helper Functions
-- ================================================

-- Function to get win rate for archived games
CREATE OR REPLACE FUNCTION calc_win_rate(winning INT, total INT)
RETURNS DECIMAL(5,4) AS $$
BEGIN
    IF total = 0 THEN
        RETURN 0;
    END IF;
    RETURN winning::DECIMAL / total;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Function to get capture rate (signals executed / generated)
CREATE OR REPLACE FUNCTION calc_capture_rate(executed INT, generated INT)
RETURNS DECIMAL(5,4) AS $$
BEGIN
    IF generated = 0 THEN
        RETURN 0;
    END IF;
    RETURN executed::DECIMAL / generated;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- View for easy querying of archived games with computed rates
CREATE OR REPLACE VIEW archived_games_with_rates AS
SELECT
    ag.*,
    calc_win_rate(ag.winning_trades, ag.total_trades) AS win_rate,
    calc_capture_rate(ag.total_signals_executed, ag.total_signals_generated) AS capture_rate
FROM archived_games ag;

-- View for daily performance summary
CREATE OR REPLACE VIEW daily_archive_summary AS
SELECT
    DATE(ended_at) AS game_date,
    COUNT(*) AS games_count,
    SUM(total_trades) AS total_trades,
    SUM(winning_trades) AS total_wins,
    SUM(losing_trades) AS total_losses,
    SUM(total_pnl) AS total_pnl,
    AVG(calc_win_rate(winning_trades, total_trades)) AS avg_win_rate
FROM archived_games
GROUP BY DATE(ended_at)
ORDER BY game_date DESC;
