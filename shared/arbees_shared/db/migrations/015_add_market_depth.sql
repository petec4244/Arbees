-- Add market depth columns (size at top of book)
-- This allows filtering out "dust" orders that might trigger false arbitrage signals

ALTER TABLE market_prices ADD COLUMN IF NOT EXISTS yes_bid_size DECIMAL(14, 2) DEFAULT 0;
ALTER TABLE market_prices ADD COLUMN IF NOT EXISTS yes_ask_size DECIMAL(14, 2) DEFAULT 0;

-- Update the hourly aggregate view (requires dropping and recreating continuous aggregate)
-- Since we can't easily ALTER a continuous aggregate in older TimescaleDB versions without drop/recreate,
-- we'll just add the columns to the raw table for now. 
-- The aggregate view will ignore them until recreated, which is fine for now as we mostly need real-time depth.
