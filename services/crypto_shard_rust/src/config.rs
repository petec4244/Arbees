//! Configuration for crypto_shard_rust

use anyhow::{anyhow, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct CryptoShardConfig {
    // Identity
    pub shard_id: String,

    // ZMQ endpoints
    pub price_sub_endpoints: Vec<String>,
    pub execution_pub_endpoint: String,

    // Redis
    pub redis_url: String,

    // Risk limits
    pub min_edge_pct: f64,
    pub max_position_size: f64,
    pub max_asset_exposure: f64,
    pub max_total_exposure: f64,
    pub volatility_scaling: bool,
    pub min_liquidity: f64,

    // Probability model
    pub model_volatility_window_days: u32,
    pub model_time_decay: bool,
    pub model_min_confidence: f64,

    // Monitoring
    pub poll_interval_secs: u64,
    pub price_staleness_secs: u64,
    pub heartbeat_interval_secs: u64,

    // Database
    pub database_url: String,
}

impl CryptoShardConfig {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL")
            .map_err(|_| anyhow!("DATABASE_URL must be set"))?;

        let min_edge_pct = parse_f64("CRYPTO_MIN_EDGE_PCT", 3.0)?;
        let max_position_size = parse_f64("CRYPTO_MAX_POSITION_SIZE", 500.0)?;
        let max_asset_exposure = parse_f64("CRYPTO_MAX_ASSET_EXPOSURE", 2000.0)?;
        let max_total_exposure = parse_f64("CRYPTO_MAX_TOTAL_EXPOSURE", 5000.0)?;
        let model_min_confidence = parse_f64("CRYPTO_MODEL_MIN_CONFIDENCE", 0.60)?;
        let min_liquidity = parse_f64("CRYPTO_MIN_LIQUIDITY", 50.0)?;

        // Validate risk limits
        if max_position_size <= 0.0 {
            return Err(anyhow!("CRYPTO_MAX_POSITION_SIZE must be > 0"));
        }
        if max_asset_exposure < max_position_size {
            return Err(anyhow!("CRYPTO_MAX_ASSET_EXPOSURE must be >= CRYPTO_MAX_POSITION_SIZE"));
        }
        if max_total_exposure < max_asset_exposure {
            return Err(anyhow!("CRYPTO_MAX_TOTAL_EXPOSURE must be >= CRYPTO_MAX_ASSET_EXPOSURE"));
        }
        if min_edge_pct < 0.0 {
            return Err(anyhow!("CRYPTO_MIN_EDGE_PCT must be >= 0"));
        }
        if model_min_confidence < 0.0 || model_min_confidence > 1.0 {
            return Err(anyhow!("CRYPTO_MODEL_MIN_CONFIDENCE must be between 0 and 1"));
        }

        Ok(Self {
            shard_id: env::var("CRYPTO_SHARD_ID")
                .unwrap_or_else(|_| "crypto_1".to_string()),

            price_sub_endpoints: env::var("CRYPTO_PRICE_SUB_ENDPOINTS")
                .unwrap_or_else(|_| "tcp://localhost:5560".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),

            execution_pub_endpoint: env::var("CRYPTO_EXECUTION_PUB_ENDPOINT")
                .unwrap_or_else(|_| "tcp://*:5559".to_string()),

            redis_url: env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),

            min_edge_pct,
            max_position_size,
            max_asset_exposure,
            max_total_exposure,

            volatility_scaling: env::var("CRYPTO_VOLATILITY_SCALING")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase()
                .parse()?,

            min_liquidity,

            model_volatility_window_days: parse_u32("CRYPTO_MODEL_VOLATILITY_WINDOW_DAYS", 30)?,
            model_time_decay: env::var("CRYPTO_MODEL_TIME_DECAY")
                .unwrap_or_else(|_| "true".to_string())
                .to_lowercase()
                .parse()?,

            model_min_confidence,

            poll_interval_secs: parse_u64("CRYPTO_POLL_INTERVAL_SECS", 30)?,
            price_staleness_secs: parse_u64("CRYPTO_PRICE_STALENESS_SECS", 60)?,
            heartbeat_interval_secs: parse_u64("CRYPTO_HEARTBEAT_INTERVAL_SECS", 5)?,

            database_url,
        })
    }
}

/// Parse environment variable as f64 with default fallback
fn parse_f64(var_name: &str, default: f64) -> Result<f64> {
    match env::var(var_name) {
        Ok(val) => val.parse().map_err(|_| anyhow!("{} must be a valid f64", var_name)),
        Err(_) => Ok(default),
    }
}

/// Parse environment variable as u32 with default fallback
fn parse_u32(var_name: &str, default: u32) -> Result<u32> {
    match env::var(var_name) {
        Ok(val) => val.parse().map_err(|_| anyhow!("{} must be a valid u32", var_name)),
        Err(_) => Ok(default),
    }
}

/// Parse environment variable as u64 with default fallback
fn parse_u64(var_name: &str, default: u64) -> Result<u64> {
    match env::var(var_name) {
        Ok(val) => val.parse().map_err(|_| anyhow!("{} must be a valid u64", var_name)),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: We avoid tests that depend on environment variables due to test isolation issues.
    // Config validation will be thoroughly tested during integration testing (Phase 8).
    // The parse_* functions are trivial wrappers around std::parse() and env::var(),
    // so they're proven correct by their usage throughout the codebase.

    #[test]
    fn test_parse_f64_with_default() {
        assert_eq!(parse_f64("NON_EXISTENT_VAR_XYZ", 42.5).unwrap(), 42.5);
    }

    #[test]
    fn test_parse_u64_with_default() {
        assert_eq!(parse_u64("NON_EXISTENT_VAR_ABC", 100).unwrap(), 100);
    }
}
