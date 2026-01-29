//! Crypto price database operations
//!
//! Provides functions for inserting and querying crypto price data
//! from the crypto_prices hypertable.

use crate::clients::coingecko::CoinPrice;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tracing::debug;

/// Insert a crypto price snapshot into the database
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `asset` - Crypto asset symbol (e.g., "BTC", "ETH")
/// * `price` - Price data from CoinGecko
pub async fn insert_crypto_price(pool: &PgPool, asset: &str, price: &CoinPrice) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO crypto_prices (
            asset, price_usd, market_cap, volume_24h,
            high_24h, low_24h, price_change_pct_24h, ath, atl
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(asset.to_uppercase())
    .bind(price.current_price)
    .bind(price.market_cap)
    .bind(price.total_volume)
    .bind(price.high_24h)
    .bind(price.low_24h)
    .bind(price.price_change_percentage_24h)
    .bind(price.ath)
    .bind(price.atl)
    .execute(pool)
    .await
    .context("Failed to insert crypto price")?;

    debug!(
        "Inserted {} price: ${:.2}",
        asset.to_uppercase(),
        price.current_price
    );

    Ok(())
}

/// Insert multiple crypto prices in a batch
///
/// More efficient than individual inserts when storing multiple prices at once.
pub async fn insert_crypto_prices_batch(pool: &PgPool, prices: &[(String, CoinPrice)]) -> Result<usize> {
    if prices.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for (asset, price) in prices {
        if let Err(e) = insert_crypto_price(pool, asset, price).await {
            tracing::warn!("Failed to insert {} price: {}", asset, e);
        } else {
            count += 1;
        }
    }

    Ok(count)
}

/// Get the latest price for a crypto asset
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `asset` - Crypto asset symbol
///
/// # Returns
/// Latest price record if available
pub async fn get_latest_crypto_price(pool: &PgPool, asset: &str) -> Result<Option<CryptoPriceRecord>> {
    let record = sqlx::query_as::<_, CryptoPriceRecord>(
        r#"
        SELECT asset, price_usd, market_cap, volume_24h, high_24h, low_24h,
               price_change_pct_24h, ath, atl, timestamp
        FROM crypto_prices
        WHERE asset = $1
        ORDER BY timestamp DESC
        LIMIT 1
        "#,
    )
    .bind(asset.to_uppercase())
    .fetch_optional(pool)
    .await
    .context("Failed to fetch latest crypto price")?;

    Ok(record)
}

/// Get price history for a crypto asset
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `asset` - Crypto asset symbol
/// * `hours` - Number of hours of history to fetch
pub async fn get_crypto_price_history(
    pool: &PgPool,
    asset: &str,
    hours: i32,
) -> Result<Vec<CryptoPriceRecord>> {
    let records = sqlx::query_as::<_, CryptoPriceRecord>(
        r#"
        SELECT asset, price_usd, market_cap, volume_24h, high_24h, low_24h,
               price_change_pct_24h, ath, atl, timestamp
        FROM crypto_prices
        WHERE asset = $1
          AND timestamp >= NOW() - ($2 || ' hours')::INTERVAL
        ORDER BY timestamp DESC
        "#,
    )
    .bind(asset.to_uppercase())
    .bind(hours.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to fetch crypto price history")?;

    Ok(records)
}

/// Calculate volatility from stored prices
///
/// Uses the database function if available, otherwise calculates from raw data.
pub async fn calculate_stored_volatility(pool: &PgPool, asset: &str, days: i32) -> Result<f64> {
    // Try using the database function first
    let result = sqlx::query_scalar::<_, f64>(
        "SELECT calculate_crypto_volatility($1, $2)",
    )
    .bind(asset.to_uppercase())
    .bind(days)
    .fetch_optional(pool)
    .await;

    match result {
        Ok(Some(vol)) => Ok(vol),
        _ => {
            // Fallback: calculate from raw data
            calculate_volatility_from_prices(pool, asset, days).await
        }
    }
}

/// Calculate volatility from raw price data
async fn calculate_volatility_from_prices(pool: &PgPool, asset: &str, days: i32) -> Result<f64> {
    let prices = sqlx::query_scalar::<_, f64>(
        r#"
        SELECT price_usd
        FROM crypto_prices
        WHERE asset = $1
          AND timestamp >= NOW() - ($2 || ' days')::INTERVAL
        ORDER BY timestamp
        "#,
    )
    .bind(asset.to_uppercase())
    .bind(days.to_string())
    .fetch_all(pool)
    .await
    .context("Failed to fetch prices for volatility calculation")?;

    if prices.len() < 2 {
        // Not enough data, return default volatility
        return Ok(0.80);
    }

    // Calculate log returns
    let mut log_returns: Vec<f64> = Vec::with_capacity(prices.len() - 1);
    for i in 1..prices.len() {
        if prices[i - 1] > 0.0 && prices[i] > 0.0 {
            log_returns.push((prices[i] / prices[i - 1]).ln());
        }
    }

    if log_returns.is_empty() {
        return Ok(0.80);
    }

    // Calculate variance
    let mean: f64 = log_returns.iter().sum::<f64>() / log_returns.len() as f64;
    let variance: f64 = log_returns
        .iter()
        .map(|r| (r - mean).powi(2))
        .sum::<f64>()
        / log_returns.len() as f64;

    // Annualize (assuming roughly hourly data points)
    let periods_per_year: f64 = 365.0 * 24.0;
    let annualized_vol = variance.sqrt() * periods_per_year.sqrt();

    Ok(annualized_vol)
}

/// Refresh the hourly materialized view
///
/// Call this periodically to update the aggregated data.
pub async fn refresh_hourly_view(pool: &PgPool) -> Result<()> {
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY crypto_prices_hourly")
        .execute(pool)
        .await
        .context("Failed to refresh hourly view")?;

    Ok(())
}

/// Refresh the daily materialized view
pub async fn refresh_daily_view(pool: &PgPool) -> Result<()> {
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY crypto_prices_daily")
        .execute(pool)
        .await
        .context("Failed to refresh daily view")?;

    Ok(())
}

/// Crypto price record from database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CryptoPriceRecord {
    pub asset: String,
    pub price_usd: f64,
    pub market_cap: Option<f64>,
    pub volume_24h: Option<f64>,
    pub high_24h: Option<f64>,
    pub low_24h: Option<f64>,
    pub price_change_pct_24h: Option<f64>,
    pub ath: Option<f64>,
    pub atl: Option<f64>,
    pub timestamp: DateTime<Utc>,
}

impl CryptoPriceRecord {
    /// Convert to CoinPrice struct for compatibility with existing code
    pub fn to_coin_price(&self) -> CoinPrice {
        CoinPrice {
            id: self.asset.to_lowercase(),
            symbol: self.asset.to_lowercase(),
            name: self.asset.clone(),
            current_price: self.price_usd,
            market_cap: self.market_cap.unwrap_or(0.0),
            total_volume: self.volume_24h.unwrap_or(0.0),
            high_24h: self.high_24h.unwrap_or(self.price_usd),
            low_24h: self.low_24h.unwrap_or(self.price_usd),
            price_change_24h: 0.0, // Not stored separately
            price_change_percentage_24h: self.price_change_pct_24h.unwrap_or(0.0),
            ath: self.ath.unwrap_or(self.price_usd),
            ath_date: None,
            atl: self.atl.unwrap_or(self.price_usd),
            atl_date: None,
            last_updated: Some(self.timestamp.to_rfc3339()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crypto_price_record_to_coin_price() {
        let record = CryptoPriceRecord {
            asset: "BTC".to_string(),
            price_usd: 50000.0,
            market_cap: Some(1_000_000_000_000.0),
            volume_24h: Some(50_000_000_000.0),
            high_24h: Some(51000.0),
            low_24h: Some(49000.0),
            price_change_pct_24h: Some(2.5),
            ath: Some(69000.0),
            atl: Some(3000.0),
            timestamp: Utc::now(),
        };

        let coin_price = record.to_coin_price();
        assert_eq!(coin_price.symbol, "btc");
        assert_eq!(coin_price.current_price, 50000.0);
        assert_eq!(coin_price.ath, 69000.0);
    }
}
