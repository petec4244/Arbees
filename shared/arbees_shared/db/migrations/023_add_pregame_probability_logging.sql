-- Migration 023: Add Pregame Probability Logging
--
-- This migration adds fields to log pregame win probabilities and blending information
-- for analytics and model improvement.
--
-- Purpose: Track how pregame expectations (from betting markets, power ratings, etc.)
-- are incorporated into live win probability calculations to learn from good/bad trades.

-- =============================================================================
-- Add pregame probability fields to game_states (time-series snapshots)
-- =============================================================================

-- Add pregame probability field to track opening market expectations
ALTER TABLE game_states
ADD COLUMN IF NOT EXISTS pregame_home_prob DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS pregame_source VARCHAR(32);  -- e.g., 'opening_odds', 'power_rating', 'market'

COMMENT ON COLUMN game_states.pregame_home_prob IS 'Pre-game home team win probability from betting markets or power ratings';
COMMENT ON COLUMN game_states.pregame_source IS 'Source of pregame probability (opening_odds, power_rating, etc.)';

-- =============================================================================
-- Add pregame fields to paper_trades for analytics
-- =============================================================================

-- Track whether pregame blending was used in the trade decision
ALTER TABLE paper_trades
ADD COLUMN IF NOT EXISTS pregame_home_prob DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS pregame_blend_weight DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS model_prob_without_pregame DECIMAL(5, 4);

COMMENT ON COLUMN paper_trades.pregame_home_prob IS 'Pregame home win probability if available at trade time';
COMMENT ON COLUMN paper_trades.pregame_blend_weight IS 'Weight given to pregame prob (0.0-0.5, decays with game progress)';
COMMENT ON COLUMN paper_trades.model_prob_without_pregame IS 'Model probability calculated without pregame blending (for comparison)';

-- =============================================================================
-- Add pregame fields to trading_signals
-- =============================================================================

-- Track pregame information in signals for analytics
ALTER TABLE trading_signals
ADD COLUMN IF NOT EXISTS pregame_home_prob DECIMAL(5, 4),
ADD COLUMN IF NOT EXISTS pregame_blend_weight DECIMAL(5, 4);

COMMENT ON COLUMN trading_signals.pregame_home_prob IS 'Pregame home win probability if used in signal generation';
COMMENT ON COLUMN trading_signals.pregame_blend_weight IS 'Weight given to pregame expectation in model probability';

-- =============================================================================
-- Add game state staleness tracking
-- =============================================================================

-- Track how fresh the game state data is for debugging stale data issues
ALTER TABLE game_states
ADD COLUMN IF NOT EXISTS fetch_latency_ms INTEGER;

COMMENT ON COLUMN game_states.fetch_latency_ms IS 'Milliseconds between game event and our fetch (for staleness analysis)';

-- =============================================================================
-- Create view for pregame probability analysis
-- =============================================================================

-- View to analyze the impact of pregame probability blending on trade performance
CREATE OR REPLACE VIEW pregame_blend_analysis AS
SELECT
    t.signal_type,
    t.sport,
    -- Bucket by game progress (early/mid/late game)
    CASE
        WHEN t.pregame_blend_weight >= 0.35 THEN 'early_game'
        WHEN t.pregame_blend_weight >= 0.15 THEN 'mid_game'
        WHEN t.pregame_blend_weight > 0.0 THEN 'late_game'
        ELSE 'no_pregame'
    END as game_phase,
    COUNT(*) as trade_count,
    -- Win rate
    SUM(CASE WHEN t.outcome = 'win' THEN 1 ELSE 0 END) as wins,
    SUM(CASE WHEN t.outcome = 'loss' THEN 1 ELSE 0 END) as losses,
    ROUND(SUM(CASE WHEN t.outcome = 'win' THEN 1 ELSE 0 END)::NUMERIC / COUNT(*) * 100, 2) as win_rate_pct,
    -- P&L
    ROUND(SUM(t.pnl), 2) as total_pnl,
    ROUND(AVG(t.pnl), 2) as avg_pnl,
    -- Edge analysis
    ROUND(AVG(t.edge_at_entry), 2) as avg_edge_at_entry,
    -- Compare model with/without pregame
    ROUND(AVG(t.model_prob) * 100, 2) as avg_model_prob_with_pregame,
    ROUND(AVG(t.model_prob_without_pregame) * 100, 2) as avg_model_prob_without_pregame,
    ROUND(AVG(t.pregame_home_prob) * 100, 2) as avg_pregame_prob,
    ROUND(AVG(t.pregame_blend_weight) * 100, 2) as avg_blend_weight_pct
FROM paper_trades t
WHERE t.status = 'closed'
  AND t.pregame_home_prob IS NOT NULL  -- Only trades with pregame info
GROUP BY t.signal_type, t.sport, game_phase
ORDER BY t.sport, game_phase, t.signal_type;

COMMENT ON VIEW pregame_blend_analysis IS 'Analyzes trade performance by game phase and pregame blending usage';

-- =============================================================================
-- Create materialized view for daily pregame impact rollup
-- =============================================================================

CREATE MATERIALIZED VIEW pregame_impact_daily
AS
SELECT
    time_bucket('1 day', t.time) AS bucket,
    t.sport,
    CASE
        WHEN t.pregame_blend_weight >= 0.35 THEN 'early_game'
        WHEN t.pregame_blend_weight >= 0.15 THEN 'mid_game'
        WHEN t.pregame_blend_weight > 0.0 THEN 'late_game'
        ELSE 'no_pregame'
    END as game_phase,
    COUNT(*) as trade_count,
    SUM(CASE WHEN t.outcome = 'win' THEN 1 ELSE 0 END) as wins,
    ROUND(AVG(t.pnl), 4) as avg_pnl,
    ROUND(AVG(t.edge_at_entry), 3) as avg_edge,
    -- How much did pregame blending change the model probability?
    ROUND(AVG(t.model_prob - t.model_prob_without_pregame) * 100, 3) as avg_pregame_impact_pct
FROM paper_trades t
WHERE t.status = 'closed'
  AND t.pregame_home_prob IS NOT NULL
GROUP BY bucket, t.sport, game_phase;

-- Create index for faster queries
CREATE INDEX idx_pregame_impact_daily_bucket ON pregame_impact_daily(bucket DESC);
CREATE INDEX idx_pregame_impact_daily_sport ON pregame_impact_daily(sport, bucket DESC);

-- Refresh policy: update once per day
SELECT add_continuous_aggregate_policy('pregame_impact_daily',
    start_offset => INTERVAL '3 days',
    end_offset => INTERVAL '1 day',
    schedule_interval => INTERVAL '1 day');

-- =============================================================================
-- Helper function: Get pregame probability impact for a game
-- =============================================================================

CREATE OR REPLACE FUNCTION get_pregame_impact(p_game_id VARCHAR)
RETURNS TABLE (
    signal_count INTEGER,
    avg_blend_weight DECIMAL(5, 4),
    avg_pregame_prob DECIMAL(5, 4),
    avg_model_change_pct DECIMAL(6, 3),
    total_pnl DECIMAL(12, 4)
) AS $$
    SELECT
        COUNT(*)::INTEGER as signal_count,
        AVG(pregame_blend_weight) as avg_blend_weight,
        AVG(pregame_home_prob) as avg_pregame_prob,
        AVG(model_prob - model_prob_without_pregame) * 100 as avg_model_change_pct,
        SUM(pnl) as total_pnl
    FROM paper_trades
    WHERE game_id = p_game_id
      AND pregame_home_prob IS NOT NULL
      AND status = 'closed';
$$ LANGUAGE SQL STABLE;

COMMENT ON FUNCTION get_pregame_impact IS 'Analyzes the impact of pregame probability blending on trades for a specific game';

-- =============================================================================
-- Indexes for performance
-- =============================================================================

-- Index for finding trades with pregame data
CREATE INDEX IF NOT EXISTS idx_paper_trades_pregame ON paper_trades(pregame_home_prob, time DESC)
WHERE pregame_home_prob IS NOT NULL;

-- Index for game state staleness analysis
CREATE INDEX IF NOT EXISTS idx_game_states_staleness ON game_states(game_id, fetch_latency_ms, time DESC)
WHERE fetch_latency_ms IS NOT NULL;

-- =============================================================================
-- Sample queries for analytics
-- =============================================================================

-- Query 1: Does pregame blending improve early-game trade performance?
--
-- SELECT * FROM pregame_blend_analysis
-- WHERE game_phase = 'early_game'
-- ORDER BY win_rate_pct DESC;

-- Query 2: What's the average impact of pregame probability on model output?
--
-- SELECT
--     sport,
--     game_phase,
--     avg_model_prob_with_pregame - avg_model_prob_without_pregame as prob_diff_pct
-- FROM pregame_blend_analysis
-- ORDER BY ABS(prob_diff_pct) DESC;

-- Query 3: Identify games where pregame blending hurt performance
--
-- SELECT
--     game_id,
--     COUNT(*) as trades,
--     AVG(pregame_blend_weight) as avg_weight,
--     SUM(pnl) as total_pnl
-- FROM paper_trades
-- WHERE pregame_home_prob IS NOT NULL
--   AND status = 'closed'
-- GROUP BY game_id
-- HAVING SUM(pnl) < -10  -- Lost $10+ on the game
-- ORDER BY total_pnl ASC
-- LIMIT 20;
