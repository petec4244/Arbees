-- Migration: Market Type Expansion
-- Adds support for non-sports markets (politics, economics, crypto, entertainment)
-- while maintaining full backward compatibility with existing sports markets

-- Create market_type enum
CREATE TYPE market_type_enum AS ENUM (
    'sport',
    'politics',
    'economics',
    'crypto',
    'entertainment'
);

-- Add market_type columns to games table
ALTER TABLE games
    ADD COLUMN IF NOT EXISTS market_type market_type_enum DEFAULT 'sport',
    ADD COLUMN IF NOT EXISTS market_subtype VARCHAR(64),
    ADD COLUMN IF NOT EXISTS entity_a VARCHAR(128),
    ADD COLUMN IF NOT EXISTS entity_b VARCHAR(128),
    ADD COLUMN IF NOT EXISTS event_start TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS event_end TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS resolution_criteria TEXT;

-- Backfill existing data
-- All existing games are sports markets with team entities
UPDATE games
SET
    market_type = 'sport',
    entity_a = home_team,
    entity_b = away_team,
    event_start = scheduled_time
WHERE market_type IS NULL OR entity_a IS NULL;

-- Add comments for documentation
COMMENT ON COLUMN games.market_type IS 'Universal market type discriminator (sport, politics, economics, crypto, entertainment)';
COMMENT ON COLUMN games.market_subtype IS 'Market-specific subtype (e.g., sport name, indicator type, asset symbol)';
COMMENT ON COLUMN games.entity_a IS 'Generic entity field: home_team (sports), candidate_1 (politics), indicator (economics), asset (crypto)';
COMMENT ON COLUMN games.entity_b IS 'Generic entity field: away_team (sports), candidate_2 (politics), NULL for single-entity markets';
COMMENT ON COLUMN games.event_start IS 'Event start time (replaces scheduled_time for non-sports)';
COMMENT ON COLUMN games.event_end IS 'Event end time (NULL for continuous markets like crypto)';
COMMENT ON COLUMN games.resolution_criteria IS 'Market resolution criteria for non-sports markets';

-- Create indexes for new query patterns
CREATE INDEX IF NOT EXISTS idx_games_market_type ON games(market_type);
CREATE INDEX IF NOT EXISTS idx_games_entity_a ON games(entity_a);
CREATE INDEX IF NOT EXISTS idx_games_event_start ON games(event_start) WHERE event_start IS NOT NULL;

-- Note: We keep the existing home_team, away_team, sport, scheduled_time columns
-- for backward compatibility. They will be deprecated in a future migration.
