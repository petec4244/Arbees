-- Migration to add market_type to market_prices table
-- See Audit findings: multi-market arbitrage requires distinguishing Spread vs Moneyline

ALTER TABLE market_prices ADD COLUMN IF NOT EXISTS market_type text DEFAULT 'moneyline';

-- Optional: Create index if querying by market_type becomes frequent
CREATE INDEX IF NOT EXISTS idx_market_prices_type ON market_prices (market_type);
