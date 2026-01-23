-- Migration 017: Add cooldown_until to games table
-- Enables temporary trading halts per game (e.g., 3 mins after win, 5 mins after loss)

ALTER TABLE games ADD COLUMN IF NOT EXISTS cooldown_until TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_games_cooldown ON games(cooldown_until);
