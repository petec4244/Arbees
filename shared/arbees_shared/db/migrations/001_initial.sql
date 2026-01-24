-- Arbees Initial Database Schema
-- TimescaleDB + PostgreSQL for time-series and relational data

-- Enable TimescaleDB extension
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- =============================================================================
-- RELATIONAL TABLES (Standard PostgreSQL)
-- =============================================================================

-- Sports enum type
CREATE TYPE sport_enum AS ENUM (
    'nfl', 'nba', 'nhl', 'mlb', 'ncaaf', 'ncaab', 'mls', 'soccer', 'tennis', 'mma'
);

-- Platform enum type
CREATE TYPE platform_enum AS ENUM (
    'kalshi', 'polymarket', 'sportsbook', 'paper'
);

-- Trade status enum
CREATE TYPE trade_status_enum AS ENUM (
    'pending', 'open', 'closed', 'cancelled', 'expired'
);

-- Trade side enum
CREATE TYPE trade_side_enum AS ENUM (
    'buy', 'sell'
);

-- Trade outcome enum
CREATE TYPE trade_outcome_enum AS ENUM (
    'win', 'loss', 'push', 'pending'
);

-- Signal type enum
CREATE TYPE signal_type_enum AS ENUM (
    'cross_market_arb', 'cross_market_arb_no', 'model_edge_yes', 'model_edge_no',
    'win_prob_shift', 'scoring_play', 'turnover', 'momentum_shift',
    'mean_reversion', 'overreaction', 'lagging_market', 'liquidity_opportunity'
);

-- Games table (relational - game metadata)
CREATE TABLE games (
    game_id VARCHAR(64) PRIMARY KEY,
    sport sport_enum NOT NULL,
    home_team VARCHAR(128) NOT NULL,
    away_team VARCHAR(128) NOT NULL,
    home_team_abbrev VARCHAR(16),
    away_team_abbrev VARCHAR(16),
    scheduled_time TIMESTAMPTZ NOT NULL,
    venue VARCHAR(256),
    broadcast VARCHAR(128),
    status VARCHAR(32) DEFAULT 'scheduled',
    final_home_score INTEGER,
    final_away_score INTEGER,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_games_sport ON games(sport);
CREATE INDEX idx_games_scheduled_time ON games(scheduled_time);
CREATE INDEX idx_games_status ON games(status);

-- Market mappings (relational - maps games to prediction markets)
CREATE TABLE market_mappings (
    id SERIAL PRIMARY KEY,
    game_id VARCHAR(64) NOT NULL REFERENCES games(game_id) ON DELETE CASCADE,
    platform platform_enum NOT NULL,
    market_id VARCHAR(256) NOT NULL,
    market_title TEXT NOT NULL,
    market_type VARCHAR(32) DEFAULT 'moneyline',
    team VARCHAR(128),
    line DECIMAL(10, 2),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(game_id, platform, market_type, team)
);

CREATE INDEX idx_market_mappings_game_id ON market_mappings(game_id);
CREATE INDEX idx_market_mappings_platform ON market_mappings(platform);
CREATE INDEX idx_market_mappings_market_id ON market_mappings(market_id);

-- Bankroll tracking (relational)
CREATE TABLE bankroll (
    id SERIAL PRIMARY KEY,
    account_name VARCHAR(64) DEFAULT 'default',
    initial_balance DECIMAL(12, 2) NOT NULL,
    current_balance DECIMAL(12, 2) NOT NULL,
    reserved_balance DECIMAL(12, 2) DEFAULT 0,
    peak_balance DECIMAL(12, 2),
    trough_balance DECIMAL(12, 2),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(account_name)
);

-- =============================================================================
-- TIME-SERIES TABLES (TimescaleDB Hypertables)
-- =============================================================================

-- Game states (time-series - frequent updates during live games)
CREATE TABLE game_states (
    time TIMESTAMPTZ NOT NULL,
    game_id VARCHAR(64) NOT NULL,
    sport sport_enum NOT NULL,
    home_score INTEGER NOT NULL DEFAULT 0,
    away_score INTEGER NOT NULL DEFAULT 0,
    period INTEGER NOT NULL DEFAULT 1,
    time_remaining VARCHAR(8),
    status VARCHAR(32),
    possession VARCHAR(128),
    -- Football specific
    down INTEGER,
    yards_to_go INTEGER,
    yard_line INTEGER,
    is_redzone BOOLEAN DEFAULT FALSE,
    -- Hockey specific
    strength VARCHAR(16),
    -- Baseball specific
    balls INTEGER,
    strikes INTEGER,
    outs INTEGER,
    runners_on_base JSONB,
    -- Calculated fields
    home_win_prob DECIMAL(5, 4),
    away_win_prob DECIMAL(5, 4)
);

SELECT create_hypertable('game_states', 'time');
CREATE INDEX idx_game_states_game_id ON game_states(game_id, time DESC);

-- Plays (time-series - individual play events)
CREATE TABLE plays (
    time TIMESTAMPTZ NOT NULL,
    play_id VARCHAR(128) NOT NULL,
    game_id VARCHAR(64) NOT NULL,
    sport sport_enum NOT NULL,
    play_type VARCHAR(64) NOT NULL,
    description TEXT,
    team VARCHAR(128),
    player VARCHAR(256),
    sequence_number INTEGER,
    home_score INTEGER,
    away_score INTEGER,
    period INTEGER,
    time_remaining VARCHAR(8),
    -- Football specific
    yards_gained INTEGER,
    yard_line INTEGER,
    down INTEGER,
    yards_to_go INTEGER,
    is_scoring BOOLEAN DEFAULT FALSE,
    is_turnover BOOLEAN DEFAULT FALSE,
    -- Basketball specific
    shot_distance INTEGER,
    shot_type VARCHAR(32),
    -- Hockey specific
    zone VARCHAR(16),
    strength VARCHAR(16),
    -- Impact analysis
    home_win_prob_before DECIMAL(5, 4),
    home_win_prob_after DECIMAL(5, 4),
    prob_change DECIMAL(5, 4)
);

SELECT create_hypertable('plays', 'time');
CREATE INDEX idx_plays_game_id ON plays(game_id, time DESC);
CREATE INDEX idx_plays_play_id ON plays(play_id);
CREATE INDEX idx_plays_significant ON plays(game_id, time DESC) WHERE ABS(prob_change) > 0.02;

-- Market prices (time-series - price snapshots)
CREATE TABLE market_prices (
    time TIMESTAMPTZ NOT NULL,
    market_id VARCHAR(256) NOT NULL,
    platform platform_enum NOT NULL,
    game_id VARCHAR(64),
    market_title TEXT,
    yes_bid DECIMAL(5, 4) NOT NULL,
    yes_ask DECIMAL(5, 4) NOT NULL,
    yes_bid_size DECIMAL(14, 2) DEFAULT 0,
    yes_ask_size DECIMAL(14, 2) DEFAULT 0,
    volume DECIMAL(14, 2) DEFAULT 0,
    open_interest DECIMAL(14, 2) DEFAULT 0,
    liquidity DECIMAL(14, 2) DEFAULT 0,
    status VARCHAR(32) DEFAULT 'open',
    last_trade_price DECIMAL(5, 4)
);

SELECT create_hypertable('market_prices', 'time');
CREATE INDEX idx_market_prices_market_id ON market_prices(market_id, time DESC);
CREATE INDEX idx_market_prices_game_id ON market_prices(game_id, time DESC) WHERE game_id IS NOT NULL;
CREATE INDEX idx_market_prices_platform ON market_prices(platform, time DESC);

-- Trading signals (time-series - generated signals)
CREATE TABLE trading_signals (
    time TIMESTAMPTZ NOT NULL,
    signal_id VARCHAR(128) NOT NULL,
    signal_type signal_type_enum NOT NULL,
    game_id VARCHAR(64),
    sport sport_enum,
    team VARCHAR(128),
    direction VARCHAR(8) NOT NULL,
    model_prob DECIMAL(5, 4),
    market_prob DECIMAL(5, 4),
    edge_pct DECIMAL(6, 3) NOT NULL,
    confidence DECIMAL(5, 4),
    platform_buy platform_enum,
    platform_sell platform_enum,
    buy_price DECIMAL(5, 4),
    sell_price DECIMAL(5, 4),
    liquidity_available DECIMAL(14, 2) DEFAULT 0,
    reason TEXT,
    play_id VARCHAR(128),
    expires_at TIMESTAMPTZ,
    executed BOOLEAN DEFAULT FALSE
);

SELECT create_hypertable('trading_signals', 'time');
CREATE INDEX idx_trading_signals_game_id ON trading_signals(game_id, time DESC) WHERE game_id IS NOT NULL;
CREATE INDEX idx_trading_signals_type ON trading_signals(signal_type, time DESC);
CREATE INDEX idx_trading_signals_active ON trading_signals(time DESC) WHERE NOT executed;

-- Arbitrage opportunities (time-series - detected opportunities)
CREATE TABLE arbitrage_opportunities (
    time TIMESTAMPTZ NOT NULL,
    opportunity_id VARCHAR(128) NOT NULL,
    opportunity_type VARCHAR(32) NOT NULL,
    event_id VARCHAR(128) NOT NULL,
    sport sport_enum,
    market_title TEXT,
    platform_buy platform_enum NOT NULL,
    platform_sell platform_enum NOT NULL,
    buy_price DECIMAL(5, 4) NOT NULL,
    sell_price DECIMAL(5, 4) NOT NULL,
    edge_pct DECIMAL(6, 3) NOT NULL,
    implied_profit DECIMAL(10, 4),
    liquidity_buy DECIMAL(14, 2) DEFAULT 0,
    liquidity_sell DECIMAL(14, 2) DEFAULT 0,
    is_risk_free BOOLEAN DEFAULT TRUE,
    status VARCHAR(32) DEFAULT 'active',
    description TEXT,
    model_probability DECIMAL(5, 4)
);

SELECT create_hypertable('arbitrage_opportunities', 'time');
CREATE INDEX idx_arb_opps_event_id ON arbitrage_opportunities(event_id, time DESC);
CREATE INDEX idx_arb_opps_edge ON arbitrage_opportunities(edge_pct DESC, time DESC) WHERE status = 'active';

-- Paper trades (time-series - simulated trades)
CREATE TABLE paper_trades (
    time TIMESTAMPTZ NOT NULL,
    trade_id VARCHAR(128) NOT NULL,
    signal_id VARCHAR(128),
    game_id VARCHAR(64),
    sport sport_enum,
    platform platform_enum NOT NULL,
    market_id VARCHAR(256) NOT NULL,
    market_title TEXT,
    side trade_side_enum NOT NULL,
    signal_type signal_type_enum,
    entry_price DECIMAL(5, 4) NOT NULL,
    exit_price DECIMAL(5, 4),
    size DECIMAL(12, 2) NOT NULL,
    model_prob DECIMAL(5, 4),
    edge_at_entry DECIMAL(6, 3),
    kelly_fraction DECIMAL(5, 4),
    entry_time TIMESTAMPTZ NOT NULL,
    exit_time TIMESTAMPTZ,
    status trade_status_enum DEFAULT 'pending',
    outcome trade_outcome_enum DEFAULT 'pending',
    entry_fees DECIMAL(10, 4) DEFAULT 0,
    exit_fees DECIMAL(10, 4) DEFAULT 0,
    pnl DECIMAL(12, 4),
    pnl_pct DECIMAL(8, 4)
);

SELECT create_hypertable('paper_trades', 'time');
CREATE INDEX idx_paper_trades_game_id ON paper_trades(game_id, time DESC) WHERE game_id IS NOT NULL;
CREATE INDEX idx_paper_trades_status ON paper_trades(status, time DESC);
CREATE INDEX idx_paper_trades_signal ON paper_trades(signal_type, time DESC);

-- Latency metrics (time-series - performance tracking)
CREATE TABLE latency_metrics (
    time TIMESTAMPTZ NOT NULL,
    game_id VARCHAR(64) NOT NULL,
    play_id VARCHAR(128),
    play_timestamp TIMESTAMPTZ,
    espn_detected_at TIMESTAMPTZ,
    market_reacted_at TIMESTAMPTZ,
    signal_generated_at TIMESTAMPTZ,
    espn_latency_ms INTEGER,
    market_latency_ms INTEGER,
    total_latency_ms INTEGER
);

SELECT create_hypertable('latency_metrics', 'time');
CREATE INDEX idx_latency_game_id ON latency_metrics(game_id, time DESC);

-- =============================================================================
-- CONTINUOUS AGGREGATES (Pre-computed rollups)
-- =============================================================================

-- Hourly market price aggregates
CREATE MATERIALIZED VIEW market_prices_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    market_id,
    platform,
    game_id,
    AVG(yes_bid) AS avg_yes_bid,
    AVG(yes_ask) AS avg_yes_ask,
    AVG((yes_ask - yes_bid) * 100) AS avg_spread,
    SUM(volume) AS total_volume,
    COUNT(*) AS sample_count
FROM market_prices
GROUP BY bucket, market_id, platform, game_id;

-- Daily trading performance aggregates
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

-- =============================================================================
-- RETENTION POLICIES
-- =============================================================================

-- Keep detailed data for 30 days, aggregates for 1 year
SELECT add_retention_policy('game_states', INTERVAL '30 days');
SELECT add_retention_policy('plays', INTERVAL '30 days');
SELECT add_retention_policy('market_prices', INTERVAL '30 days');
SELECT add_retention_policy('trading_signals', INTERVAL '30 days');
SELECT add_retention_policy('arbitrage_opportunities', INTERVAL '30 days');
SELECT add_retention_policy('latency_metrics', INTERVAL '7 days');

-- Refresh continuous aggregates
SELECT add_continuous_aggregate_policy('market_prices_hourly',
    start_offset => INTERVAL '3 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour');

SELECT add_continuous_aggregate_policy('trading_performance_daily',
    start_offset => INTERVAL '3 days',
    end_offset => INTERVAL '1 day',
    schedule_interval => INTERVAL '1 day');

-- =============================================================================
-- HELPER FUNCTIONS
-- =============================================================================

-- Function to get latest game state
CREATE OR REPLACE FUNCTION get_latest_game_state(p_game_id VARCHAR)
RETURNS game_states AS $$
    SELECT * FROM game_states
    WHERE game_id = p_game_id
    ORDER BY time DESC
    LIMIT 1;
$$ LANGUAGE SQL STABLE;

-- Function to get recent plays for a game
CREATE OR REPLACE FUNCTION get_recent_plays(p_game_id VARCHAR, p_limit INTEGER DEFAULT 10)
RETURNS SETOF plays AS $$
    SELECT * FROM plays
    WHERE game_id = p_game_id
    ORDER BY time DESC
    LIMIT p_limit;
$$ LANGUAGE SQL STABLE;

-- Function to get active arbitrage opportunities
CREATE OR REPLACE FUNCTION get_active_arbitrage(p_min_edge DECIMAL DEFAULT 1.0)
RETURNS SETOF arbitrage_opportunities AS $$
    SELECT DISTINCT ON (event_id, platform_buy, platform_sell) *
    FROM arbitrage_opportunities
    WHERE status = 'active'
      AND edge_pct >= p_min_edge
      AND time > NOW() - INTERVAL '5 minutes'
    ORDER BY event_id, platform_buy, platform_sell, time DESC;
$$ LANGUAGE SQL STABLE;
