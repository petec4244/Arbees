-- P0 Critical Fixes Migration
-- 1. Add unique constraint to market_prices to prevent duplicate entries
-- 2. Add version column to bankroll for optimistic locking
-- 3. Add CHECK constraints for probability and edge ranges

-- =============================================================================
-- P0-3: Add unique constraint to market_prices
-- =============================================================================
-- This prevents duplicate price entries for the same market at the same timestamp.
-- Without this, signal generation can use corrupted/duplicate data.

-- First, remove any existing duplicates (keep the latest entry)
DELETE FROM market_prices mp1
USING market_prices mp2
WHERE mp1.ctid < mp2.ctid
  AND mp1.time = mp2.time
  AND mp1.market_id = mp2.market_id
  AND mp1.platform = mp2.platform
  AND COALESCE(mp1.contract_team, '') = COALESCE(mp2.contract_team, '');

-- Create unique index (more flexible than constraint for hypertables)
CREATE UNIQUE INDEX IF NOT EXISTS idx_market_prices_unique
ON market_prices (time, market_id, platform, COALESCE(contract_team, ''));

-- =============================================================================
-- P0-4: Add version column to bankroll for optimistic locking
-- =============================================================================
-- This prevents race conditions when multiple services update bankroll concurrently.

ALTER TABLE bankroll
ADD COLUMN IF NOT EXISTS version INTEGER DEFAULT 1;

-- Create a function to atomically update bankroll with version check
CREATE OR REPLACE FUNCTION update_bankroll_atomic(
    p_account_name VARCHAR,
    p_current_balance DECIMAL,
    p_piggybank_balance DECIMAL,
    p_expected_version INTEGER
) RETURNS BOOLEAN AS $$
DECLARE
    rows_affected INTEGER;
BEGIN
    UPDATE bankroll
    SET current_balance = p_current_balance,
        piggybank_balance = p_piggybank_balance,
        peak_balance = GREATEST(peak_balance, p_current_balance + p_piggybank_balance),
        trough_balance = LEAST(trough_balance, p_current_balance),
        version = version + 1,
        updated_at = NOW()
    WHERE account_name = p_account_name
      AND version = p_expected_version;

    GET DIAGNOSTICS rows_affected = ROW_COUNT;
    RETURN rows_affected > 0;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- Add CHECK constraints for data validity
-- =============================================================================

-- Probability columns should be between 0 and 1
-- Note: Using DO blocks to handle cases where constraints already exist

DO $$
BEGIN
    -- game_states probability constraints
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_game_states_home_prob'
    ) THEN
        ALTER TABLE game_states
        ADD CONSTRAINT chk_game_states_home_prob
        CHECK (home_win_prob IS NULL OR (home_win_prob >= 0 AND home_win_prob <= 1));
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_game_states_away_prob'
    ) THEN
        ALTER TABLE game_states
        ADD CONSTRAINT chk_game_states_away_prob
        CHECK (away_win_prob IS NULL OR (away_win_prob >= 0 AND away_win_prob <= 1));
    END IF;
END $$;

DO $$
BEGIN
    -- market_prices bid/ask constraints
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_market_prices_bid'
    ) THEN
        ALTER TABLE market_prices
        ADD CONSTRAINT chk_market_prices_bid
        CHECK (yes_bid >= 0 AND yes_bid <= 1);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_market_prices_ask'
    ) THEN
        ALTER TABLE market_prices
        ADD CONSTRAINT chk_market_prices_ask
        CHECK (yes_ask >= 0 AND yes_ask <= 1);
    END IF;
END $$;

DO $$
BEGIN
    -- trading_signals edge constraint (percentages -100 to +100)
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_trading_signals_edge'
    ) THEN
        ALTER TABLE trading_signals
        ADD CONSTRAINT chk_trading_signals_edge
        CHECK (edge_pct >= -100 AND edge_pct <= 100);
    END IF;

    -- trading_signals probability constraints
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_trading_signals_model_prob'
    ) THEN
        ALTER TABLE trading_signals
        ADD CONSTRAINT chk_trading_signals_model_prob
        CHECK (model_prob IS NULL OR (model_prob >= 0 AND model_prob <= 1));
    END IF;
END $$;

DO $$
BEGIN
    -- paper_trades price constraints
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_paper_trades_entry_price'
    ) THEN
        ALTER TABLE paper_trades
        ADD CONSTRAINT chk_paper_trades_entry_price
        CHECK (entry_price >= 0 AND entry_price <= 1);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_paper_trades_exit_price'
    ) THEN
        ALTER TABLE paper_trades
        ADD CONSTRAINT chk_paper_trades_exit_price
        CHECK (exit_price IS NULL OR (exit_price >= 0 AND exit_price <= 1));
    END IF;
END $$;

-- =============================================================================
-- Add index for executed signals (supports querying completed signals)
-- =============================================================================
CREATE INDEX IF NOT EXISTS idx_trading_signals_executed
ON trading_signals(time DESC)
WHERE executed = TRUE;

-- =============================================================================
-- Comment on changes for documentation
-- =============================================================================
COMMENT ON INDEX idx_market_prices_unique IS 'P0-3: Prevents duplicate price entries for same market/time/platform/team';
COMMENT ON COLUMN bankroll.version IS 'P0-4: Version for optimistic locking to prevent concurrent update races';
COMMENT ON FUNCTION update_bankroll_atomic IS 'P0-4: Atomic bankroll update with optimistic locking';
