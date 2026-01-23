-- Migration 016: Add piggybank_balance to bankroll table
-- Piggybank system: 50% of profits are protected and not used for position sizing

ALTER TABLE bankroll
ADD COLUMN IF NOT EXISTS piggybank_balance DECIMAL(12, 2) DEFAULT 0.0;

COMMENT ON COLUMN bankroll.piggybank_balance IS
'Protected savings: 50% of each winning trade is moved here and not used for sizing';
