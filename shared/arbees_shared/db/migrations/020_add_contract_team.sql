-- Migration 020: Add contract_team column to market_prices
-- This column is used by signal_processor_rust to match markets to teams

ALTER TABLE market_prices
ADD COLUMN IF NOT EXISTS contract_team VARCHAR(128);

-- Create index for team-based lookups
CREATE INDEX IF NOT EXISTS idx_market_prices_contract_team
ON market_prices(game_id, contract_team, time DESC)
WHERE contract_team IS NOT NULL;
