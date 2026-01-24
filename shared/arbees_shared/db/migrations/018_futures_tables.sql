-- ================================================
-- Migration 018: Futures Tracking & Game Lifecycle Management
-- Enables pre-game monitoring, price history, and lifecycle state machine
-- ================================================

-- Lifecycle status enum for games
CREATE TYPE lifecycle_status_enum AS ENUM (
    'scheduled',           -- Just created, not yet monitored
    'futures_monitoring',  -- Being tracked by FuturesMonitor (24-48h before)
    'pre_game',           -- Handed off to Orchestrator (15min before)
    'live',               -- Game in progress
    'ended',              -- Game finished, awaiting archive
    'archived'            -- Fully archived
);

-- Add futures signal types to existing enum
ALTER TYPE signal_type_enum ADD VALUE IF NOT EXISTS 'futures_early_edge';
ALTER TYPE signal_type_enum ADD VALUE IF NOT EXISTS 'futures_line_movement';

-- Add lifecycle_status to games table
ALTER TABLE games ADD COLUMN IF NOT EXISTS lifecycle_status lifecycle_status_enum DEFAULT 'scheduled';

-- Index for lifecycle queries
CREATE INDEX IF NOT EXISTS idx_games_lifecycle_status ON games(lifecycle_status);

-- ================================================
-- Futures Games Table
-- Tracks games being monitored before they start
-- ================================================

CREATE TABLE IF NOT EXISTS futures_games (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(64) NOT NULL UNIQUE,
    sport VARCHAR(20) NOT NULL,
    home_team VARCHAR(128) NOT NULL,
    away_team VARCHAR(128) NOT NULL,
    home_team_abbrev VARCHAR(16),
    away_team_abbrev VARCHAR(16),
    scheduled_time TIMESTAMPTZ NOT NULL,

    -- Market discovery
    kalshi_market_id VARCHAR(256),
    polymarket_market_id VARCHAR(256),
    market_ids_by_type JSONB DEFAULT '{}',  -- {"moneyline": {"kalshi": "...", "polymarket": "..."}, "spread": {...}}

    -- Opening and current probabilities
    opening_home_prob DECIMAL(5, 4),
    opening_away_prob DECIMAL(5, 4),
    current_home_prob DECIMAL(5, 4),
    current_away_prob DECIMAL(5, 4),

    -- Line movement tracking
    line_movement_pct DECIMAL(6, 3) DEFAULT 0,  -- Absolute movement from opening
    max_movement_pct DECIMAL(6, 3) DEFAULT 0,   -- Maximum movement seen
    movement_direction VARCHAR(10),              -- 'home', 'away', or NULL

    -- Volume/interest metrics
    total_volume_kalshi DECIMAL(14, 2) DEFAULT 0,
    total_volume_polymarket DECIMAL(14, 2) DEFAULT 0,

    -- Lifecycle tracking
    lifecycle_status lifecycle_status_enum DEFAULT 'futures_monitoring',
    monitoring_started_at TIMESTAMPTZ DEFAULT NOW(),
    markets_discovered_at TIMESTAMPTZ,
    handed_off_at TIMESTAMPTZ,

    -- Metadata
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_futures_games_sport ON futures_games(sport);
CREATE INDEX IF NOT EXISTS idx_futures_games_scheduled ON futures_games(scheduled_time);
CREATE INDEX IF NOT EXISTS idx_futures_games_status ON futures_games(lifecycle_status);
CREATE INDEX IF NOT EXISTS idx_futures_games_game_id ON futures_games(game_id);

-- Partial index for active monitoring
CREATE INDEX IF NOT EXISTS idx_futures_games_active ON futures_games(scheduled_time)
    WHERE lifecycle_status = 'futures_monitoring';

-- ================================================
-- Futures Price History (TimescaleDB Hypertable)
-- Stores price snapshots for pre-game markets
-- ================================================

CREATE TABLE IF NOT EXISTS futures_price_history (
    time TIMESTAMPTZ NOT NULL,
    game_id VARCHAR(64) NOT NULL,
    platform VARCHAR(20) NOT NULL,  -- 'kalshi', 'polymarket'
    market_type VARCHAR(32) DEFAULT 'moneyline',
    team VARCHAR(128),  -- NULL for moneyline, team name for spread/total

    -- Pricing data
    yes_bid DECIMAL(5, 4),
    yes_ask DECIMAL(5, 4),
    yes_mid DECIMAL(5, 4),  -- (bid + ask) / 2
    spread_cents DECIMAL(6, 2),  -- (ask - bid) * 100

    -- Volume and liquidity
    volume DECIMAL(14, 2) DEFAULT 0,
    volume_24h DECIMAL(14, 2) DEFAULT 0,
    open_interest DECIMAL(14, 2) DEFAULT 0,

    -- Context
    hours_until_start DECIMAL(6, 2)  -- Helpful for analysis
);

SELECT create_hypertable('futures_price_history', 'time', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_futures_price_game ON futures_price_history(game_id, time DESC);
CREATE INDEX IF NOT EXISTS idx_futures_price_platform ON futures_price_history(platform, time DESC);
CREATE INDEX IF NOT EXISTS idx_futures_price_type ON futures_price_history(market_type, time DESC);

-- ================================================
-- Futures Signals (TimescaleDB Hypertable)
-- Pre-game trading signals
-- ================================================

CREATE TABLE IF NOT EXISTS futures_signals (
    time TIMESTAMPTZ NOT NULL,
    signal_id VARCHAR(128) NOT NULL,
    game_id VARCHAR(64) NOT NULL,
    sport VARCHAR(20) NOT NULL,

    -- Signal details
    signal_type VARCHAR(50) NOT NULL,  -- 'futures_early_edge', 'futures_line_movement'
    direction VARCHAR(10) NOT NULL,     -- 'yes' or 'no'
    team VARCHAR(128),
    market_type VARCHAR(32) DEFAULT 'moneyline',

    -- Edge calculation
    model_prob DECIMAL(5, 4),
    market_prob DECIMAL(5, 4),
    edge_pct DECIMAL(6, 3) NOT NULL,
    confidence DECIMAL(5, 4),

    -- Movement context (for line_movement signals)
    opening_prob DECIMAL(5, 4),
    current_prob DECIMAL(5, 4),
    movement_pct DECIMAL(6, 3),

    -- Timing context
    hours_until_start DECIMAL(6, 2),

    -- Execution tracking
    executed BOOLEAN DEFAULT FALSE,
    executed_at TIMESTAMPTZ,
    execution_reason TEXT,

    -- Metadata
    reason TEXT,
    expires_at TIMESTAMPTZ
);

SELECT create_hypertable('futures_signals', 'time', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_futures_signals_game ON futures_signals(game_id, time DESC);
CREATE INDEX IF NOT EXISTS idx_futures_signals_type ON futures_signals(signal_type, time DESC);
CREATE INDEX IF NOT EXISTS idx_futures_signals_active ON futures_signals(time DESC)
    WHERE NOT executed;
CREATE INDEX IF NOT EXISTS idx_futures_signals_edge ON futures_signals(edge_pct DESC, time DESC)
    WHERE NOT executed;

-- ================================================
-- Retention Policies
-- Keep futures data longer for ML analysis
-- ================================================

SELECT add_retention_policy('futures_price_history', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('futures_signals', INTERVAL '90 days', if_not_exists => TRUE);

-- ================================================
-- Continuous Aggregates
-- ================================================

-- Hourly futures price aggregates
CREATE MATERIALIZED VIEW IF NOT EXISTS futures_prices_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    game_id,
    platform,
    market_type,
    AVG(yes_mid) AS avg_mid,
    MIN(yes_mid) AS min_mid,
    MAX(yes_mid) AS max_mid,
    AVG(spread_cents) AS avg_spread,
    SUM(volume) AS total_volume,
    COUNT(*) AS sample_count
FROM futures_price_history
GROUP BY bucket, game_id, platform, market_type
WITH NO DATA;

SELECT add_continuous_aggregate_policy('futures_prices_hourly',
    start_offset => INTERVAL '3 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour',
    if_not_exists => TRUE);

-- ================================================
-- Helper Functions
-- ================================================

-- Get active futures games (those being monitored)
CREATE OR REPLACE FUNCTION get_active_futures_games(p_min_hours DECIMAL DEFAULT 0, p_max_hours DECIMAL DEFAULT 48)
RETURNS SETOF futures_games AS $$
    SELECT * FROM futures_games
    WHERE lifecycle_status = 'futures_monitoring'
      AND scheduled_time > NOW() + (p_min_hours || ' hours')::INTERVAL
      AND scheduled_time < NOW() + (p_max_hours || ' hours')::INTERVAL
    ORDER BY scheduled_time ASC;
$$ LANGUAGE SQL STABLE;

-- Get games ready for handoff (15 minutes or less until start)
CREATE OR REPLACE FUNCTION get_futures_ready_for_handoff(p_handoff_minutes INTEGER DEFAULT 15)
RETURNS SETOF futures_games AS $$
    SELECT * FROM futures_games
    WHERE lifecycle_status = 'futures_monitoring'
      AND scheduled_time <= NOW() + (p_handoff_minutes || ' minutes')::INTERVAL
    ORDER BY scheduled_time ASC;
$$ LANGUAGE SQL STABLE;

-- Get recent futures signals for a game
CREATE OR REPLACE FUNCTION get_futures_signals_for_game(p_game_id VARCHAR, p_limit INTEGER DEFAULT 20)
RETURNS SETOF futures_signals AS $$
    SELECT * FROM futures_signals
    WHERE game_id = p_game_id
    ORDER BY time DESC
    LIMIT p_limit;
$$ LANGUAGE SQL STABLE;

-- Get line movement summary
CREATE OR REPLACE FUNCTION get_line_movement_summary(p_sport VARCHAR DEFAULT NULL, p_min_movement DECIMAL DEFAULT 3.0)
RETURNS TABLE (
    game_id VARCHAR(64),
    sport VARCHAR(20),
    home_team VARCHAR(128),
    away_team VARCHAR(128),
    scheduled_time TIMESTAMPTZ,
    opening_home_prob DECIMAL(5,4),
    current_home_prob DECIMAL(5,4),
    line_movement_pct DECIMAL(6,3),
    movement_direction VARCHAR(10)
) AS $$
    SELECT
        game_id,
        sport,
        home_team,
        away_team,
        scheduled_time,
        opening_home_prob,
        current_home_prob,
        line_movement_pct,
        movement_direction
    FROM futures_games
    WHERE lifecycle_status = 'futures_monitoring'
      AND ABS(line_movement_pct) >= p_min_movement
      AND (p_sport IS NULL OR sport = p_sport)
    ORDER BY ABS(line_movement_pct) DESC;
$$ LANGUAGE SQL STABLE;

-- ================================================
-- Views
-- ================================================

-- Active futures monitoring dashboard view
CREATE OR REPLACE VIEW futures_dashboard AS
SELECT
    fg.game_id,
    fg.sport,
    fg.home_team || ' vs ' || fg.away_team AS matchup,
    fg.scheduled_time,
    EXTRACT(EPOCH FROM (fg.scheduled_time - NOW())) / 3600 AS hours_until_start,
    fg.opening_home_prob,
    fg.current_home_prob,
    fg.line_movement_pct,
    fg.movement_direction,
    fg.kalshi_market_id IS NOT NULL AS has_kalshi,
    fg.polymarket_market_id IS NOT NULL AS has_polymarket,
    fg.total_volume_kalshi + fg.total_volume_polymarket AS total_volume,
    fg.lifecycle_status,
    (
        SELECT COUNT(*) FROM futures_signals fs
        WHERE fs.game_id = fg.game_id AND NOT fs.executed
    ) AS active_signals
FROM futures_games fg
WHERE fg.lifecycle_status = 'futures_monitoring'
ORDER BY fg.scheduled_time ASC;

-- Futures signals summary view
CREATE OR REPLACE VIEW futures_signals_summary AS
SELECT
    fs.signal_id,
    fs.game_id,
    fs.sport,
    fg.home_team || ' vs ' || fg.away_team AS matchup,
    fg.scheduled_time,
    fs.signal_type,
    fs.direction,
    fs.team,
    fs.edge_pct,
    fs.confidence,
    fs.hours_until_start,
    fs.executed,
    fs.time AS generated_at
FROM futures_signals fs
JOIN futures_games fg ON fs.game_id = fg.game_id
WHERE fs.time > NOW() - INTERVAL '24 hours'
ORDER BY fs.edge_pct DESC, fs.time DESC;
