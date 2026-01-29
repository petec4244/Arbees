-- Migration 025: Crypto Price History
-- Tracks crypto prices for volatility analysis and audit trail
-- Part of the multi-market expansion (crypto markets support)

-- Create crypto_prices hypertable for time-series price data
CREATE TABLE IF NOT EXISTS crypto_prices (
    id BIGSERIAL,
    asset VARCHAR(16) NOT NULL,
    price_usd NUMERIC(18, 8) NOT NULL,
    market_cap NUMERIC(24, 2),
    volume_24h NUMERIC(24, 2),
    high_24h NUMERIC(18, 8),
    low_24h NUMERIC(18, 8),
    price_change_pct_24h NUMERIC(10, 4),
    ath NUMERIC(18, 8),
    atl NUMERIC(18, 8),
    source VARCHAR(32) DEFAULT 'coingecko',
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, timestamp)
);

-- Convert to hypertable (TimescaleDB)
SELECT create_hypertable('crypto_prices', 'timestamp',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE);

-- Index for asset+time queries (most common pattern)
CREATE INDEX IF NOT EXISTS idx_crypto_prices_asset_time
    ON crypto_prices (asset, timestamp DESC);

-- Index for finding latest price per asset
CREATE INDEX IF NOT EXISTS idx_crypto_prices_asset
    ON crypto_prices (asset);

-- Retention: 90 days of raw data
-- Note: This requires timescaledb with license that supports retention policies
-- If not available, manual cleanup can be used instead
DO $$
BEGIN
    PERFORM add_retention_policy('crypto_prices', INTERVAL '90 days', if_not_exists => TRUE);
EXCEPTION
    WHEN undefined_function THEN
        RAISE NOTICE 'add_retention_policy not available, skipping retention policy';
END $$;

-- Hourly OHLCV aggregate materialized view
-- This provides efficient historical queries for volatility calculation
CREATE MATERIALIZED VIEW IF NOT EXISTS crypto_prices_hourly AS
SELECT
    time_bucket('1 hour', timestamp) AS bucket,
    asset,
    first(price_usd, timestamp) AS open,
    max(price_usd) AS high,
    min(price_usd) AS low,
    last(price_usd, timestamp) AS close,
    avg(price_usd) AS avg_price,
    count(*) AS samples
FROM crypto_prices
GROUP BY bucket, asset;

-- Index on the materialized view
CREATE INDEX IF NOT EXISTS idx_crypto_prices_hourly_asset_bucket
    ON crypto_prices_hourly (asset, bucket DESC);

-- Daily OHLCV aggregate for longer-term analysis
CREATE MATERIALIZED VIEW IF NOT EXISTS crypto_prices_daily AS
SELECT
    time_bucket('1 day', timestamp) AS bucket,
    asset,
    first(price_usd, timestamp) AS open,
    max(price_usd) AS high,
    min(price_usd) AS low,
    last(price_usd, timestamp) AS close,
    avg(price_usd) AS avg_price,
    max(market_cap) AS market_cap,
    sum(volume_24h) / count(*) AS avg_volume,
    count(*) AS samples
FROM crypto_prices
GROUP BY bucket, asset;

-- Index on daily view
CREATE INDEX IF NOT EXISTS idx_crypto_prices_daily_asset_bucket
    ON crypto_prices_daily (asset, bucket DESC);

-- Helper function to get latest price for an asset
CREATE OR REPLACE FUNCTION get_latest_crypto_price(p_asset VARCHAR)
RETURNS TABLE (
    asset VARCHAR,
    price_usd NUMERIC,
    timestamp TIMESTAMPTZ
) AS $$
BEGIN
    RETURN QUERY
    SELECT cp.asset, cp.price_usd, cp.timestamp
    FROM crypto_prices cp
    WHERE cp.asset = p_asset
    ORDER BY cp.timestamp DESC
    LIMIT 1;
END;
$$ LANGUAGE plpgsql;

-- Helper function to calculate volatility from stored prices
CREATE OR REPLACE FUNCTION calculate_crypto_volatility(
    p_asset VARCHAR,
    p_days INT DEFAULT 30
)
RETURNS NUMERIC AS $$
DECLARE
    v_volatility NUMERIC;
BEGIN
    -- Calculate annualized volatility from hourly log returns
    WITH hourly_prices AS (
        SELECT bucket, close
        FROM crypto_prices_hourly
        WHERE asset = p_asset
          AND bucket >= NOW() - (p_days || ' days')::INTERVAL
        ORDER BY bucket
    ),
    log_returns AS (
        SELECT
            LN(close / LAG(close) OVER (ORDER BY bucket)) AS log_return
        FROM hourly_prices
    )
    SELECT
        STDDEV(log_return) * SQRT(24 * 365) INTO v_volatility
    FROM log_returns
    WHERE log_return IS NOT NULL;

    RETURN COALESCE(v_volatility, 0.80); -- Default 80% if not enough data
END;
$$ LANGUAGE plpgsql;

-- Comments for documentation
COMMENT ON TABLE crypto_prices IS 'Time-series crypto price data from CoinGecko for probability calculations';
COMMENT ON COLUMN crypto_prices.asset IS 'Crypto asset symbol (BTC, ETH, SOL, etc.)';
COMMENT ON COLUMN crypto_prices.price_usd IS 'Current price in USD';
COMMENT ON COLUMN crypto_prices.ath IS 'All-time high price';
COMMENT ON COLUMN crypto_prices.atl IS 'All-time low price';
COMMENT ON MATERIALIZED VIEW crypto_prices_hourly IS 'Hourly OHLCV candles for volatility calculation';
COMMENT ON MATERIALIZED VIEW crypto_prices_daily IS 'Daily OHLCV candles for longer-term analysis';
