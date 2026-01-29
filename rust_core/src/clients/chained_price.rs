//! Chained Price Provider
//!
//! Implements a fallback chain of crypto price providers.
//! Default chain: Coinbase → Binance → CoinGecko
//!
//! The chain tries providers in order, skipping those that are rate-limited or unavailable.

use super::binance::BinanceClient;
use super::coinbase::CoinbaseClient;
use super::coingecko::CoinGeckoClient;
use super::crypto_price::{CryptoPrice, CryptoPriceProvider, ProviderStatus, VolatilityResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// A price provider that chains multiple providers with fallback logic
pub struct ChainedPriceProvider {
    /// Ordered list of providers to try
    providers: Vec<Arc<dyn CryptoPriceProvider>>,
    /// Provider name for logging
    name: String,
}

impl ChainedPriceProvider {
    /// Create a new chained provider with the given list of providers
    pub fn new(providers: Vec<Arc<dyn CryptoPriceProvider>>) -> Self {
        let names: Vec<&str> = providers.iter().map(|p| p.provider_name()).collect();
        let name = format!("ChainedProvider({})", names.join(" → "));

        Self { providers, name }
    }

    /// Create the default chain: Coinbase → Binance → CoinGecko
    ///
    /// This order prioritizes:
    /// 1. Coinbase: Most accurate US pricing, lower latency
    /// 2. Binance: Highest liquidity, most trading pairs
    /// 3. CoinGecko: Best metadata (market cap, names), slower but comprehensive
    pub fn new_default() -> Self {
        let providers: Vec<Arc<dyn CryptoPriceProvider>> = vec![
            Arc::new(CoinbaseClient::new()),
            Arc::new(BinanceClient::new()),
            Arc::new(CoinGeckoClientAdapter::new()),
        ];

        Self::new(providers)
    }

    /// Create a chain with just exchange providers (no aggregators)
    pub fn new_exchanges_only() -> Self {
        let providers: Vec<Arc<dyn CryptoPriceProvider>> = vec![
            Arc::new(CoinbaseClient::new()),
            Arc::new(BinanceClient::new()),
        ];

        Self::new(providers)
    }

    /// Check if a provider should be skipped based on status
    fn should_skip(&self, provider: &dyn CryptoPriceProvider) -> bool {
        matches!(
            provider.status(),
            ProviderStatus::RateLimited | ProviderStatus::Unavailable
        )
    }
}

#[async_trait]
impl CryptoPriceProvider for ChainedPriceProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn status(&self) -> ProviderStatus {
        // Return healthy if at least one provider is healthy
        for provider in &self.providers {
            if provider.status() == ProviderStatus::Healthy {
                return ProviderStatus::Healthy;
            }
        }

        // Return rate limited if all are rate limited
        let all_rate_limited = self
            .providers
            .iter()
            .all(|p| p.status() == ProviderStatus::RateLimited);
        if all_rate_limited {
            return ProviderStatus::RateLimited;
        }

        // Otherwise return error
        ProviderStatus::Error
    }

    fn symbol_to_id(&self, symbol: &str) -> String {
        // Use the first provider's format (Coinbase by default)
        self.providers
            .first()
            .map(|p| p.symbol_to_id(symbol))
            .unwrap_or_else(|| symbol.to_string())
    }

    async fn get_price(&self, coin_id: &str) -> Result<CryptoPrice> {
        let mut last_error: Option<anyhow::Error> = None;

        for provider in &self.providers {
            // Skip providers with bad status
            if self.should_skip(provider.as_ref()) {
                debug!(
                    "Skipping {} (status: {:?})",
                    provider.provider_name(),
                    provider.status()
                );
                continue;
            }

            match provider.get_price(coin_id).await {
                Ok(price) => {
                    debug!(
                        "Got price for {} from {} (${:.2})",
                        coin_id,
                        provider.provider_name(),
                        price.current_price
                    );
                    return Ok(price);
                }
                Err(e) => {
                    warn!(
                        "{} failed for {}: {}",
                        provider.provider_name(),
                        coin_id,
                        e
                    );
                    last_error = Some(e);
                    // Continue to next provider
                }
            }
        }

        // All providers failed
        Err(last_error.unwrap_or_else(|| anyhow!("No providers available for {}", coin_id)))
    }

    async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CryptoPrice>> {
        let mut all_prices = Vec::new();
        let mut remaining: Vec<&str> = coin_ids.to_vec();

        for provider in &self.providers {
            if remaining.is_empty() {
                break;
            }

            if self.should_skip(provider.as_ref()) {
                continue;
            }

            match provider.get_prices(&remaining).await {
                Ok(prices) => {
                    // Remove fetched coins from remaining
                    let fetched_symbols: Vec<String> =
                        prices.iter().map(|p| p.symbol.to_uppercase()).collect();

                    remaining.retain(|id| {
                        !fetched_symbols.contains(&id.to_uppercase())
                    });

                    info!(
                        "Got {} prices from {}, {} remaining",
                        prices.len(),
                        provider.provider_name(),
                        remaining.len()
                    );

                    all_prices.extend(prices);
                }
                Err(e) => {
                    warn!(
                        "{} batch fetch failed: {}",
                        provider.provider_name(),
                        e
                    );
                }
            }
        }

        if all_prices.is_empty() && !coin_ids.is_empty() {
            return Err(anyhow!("Failed to fetch any prices"));
        }

        Ok(all_prices)
    }

    async fn get_historical_prices(
        &self,
        coin_id: &str,
        days: u32,
    ) -> Result<Vec<(DateTime<Utc>, f64)>> {
        let mut last_error: Option<anyhow::Error> = None;

        for provider in &self.providers {
            if self.should_skip(provider.as_ref()) {
                continue;
            }

            match provider.get_historical_prices(coin_id, days).await {
                Ok(prices) => {
                    debug!(
                        "Got {} historical prices for {} from {}",
                        prices.len(),
                        coin_id,
                        provider.provider_name()
                    );
                    return Ok(prices);
                }
                Err(e) => {
                    warn!(
                        "{} historical fetch failed for {}: {}",
                        provider.provider_name(),
                        coin_id,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow!("No providers available for historical data: {}", coin_id)
        }))
    }

    async fn calculate_volatility(&self, coin_id: &str, days: u32) -> Result<VolatilityResult> {
        let mut last_error: Option<anyhow::Error> = None;

        for provider in &self.providers {
            if self.should_skip(provider.as_ref()) {
                continue;
            }

            match provider.calculate_volatility(coin_id, days).await {
                Ok(vol) => {
                    debug!(
                        "Got volatility for {} from {}: {:.2}%",
                        coin_id,
                        provider.provider_name(),
                        vol.annualized_volatility * 100.0
                    );
                    return Ok(vol);
                }
                Err(e) => {
                    warn!(
                        "{} volatility calc failed for {}: {}",
                        provider.provider_name(),
                        coin_id,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow!("No providers available for volatility: {}", coin_id)
        }))
    }
}

/// Adapter to make CoinGeckoClient implement CryptoPriceProvider trait
pub struct CoinGeckoClientAdapter {
    client: CoinGeckoClient,
    status: std::sync::RwLock<ProviderStatus>,
}

impl CoinGeckoClientAdapter {
    pub fn new() -> Self {
        Self {
            client: CoinGeckoClient::new(),
            status: std::sync::RwLock::new(ProviderStatus::Healthy),
        }
    }
}

impl Default for CoinGeckoClientAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CryptoPriceProvider for CoinGeckoClientAdapter {
    fn provider_name(&self) -> &str {
        "CoinGecko"
    }

    fn status(&self) -> ProviderStatus {
        *self.status.read().unwrap()
    }

    fn symbol_to_id(&self, symbol: &str) -> String {
        CoinGeckoClient::symbol_to_id(symbol).to_string()
    }

    async fn get_price(&self, coin_id: &str) -> Result<CryptoPrice> {
        match self.client.get_price(coin_id).await {
            Ok(price) => {
                *self.status.write().unwrap() = ProviderStatus::Healthy;
                Ok(CryptoPrice {
                    id: price.id,
                    symbol: price.symbol.to_uppercase(),
                    name: price.name,
                    current_price: price.current_price,
                    high_24h: price.high_24h,
                    low_24h: price.low_24h,
                    price_change_24h: price.price_change_24h,
                    price_change_percentage_24h: price.price_change_percentage_24h,
                    volume_24h: price.total_volume,
                    market_cap: Some(price.market_cap),
                    last_updated: Utc::now(),
                    source: "CoinGecko".to_string(),
                })
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("429") || err_str.contains("rate") {
                    *self.status.write().unwrap() = ProviderStatus::RateLimited;
                } else {
                    *self.status.write().unwrap() = ProviderStatus::Error;
                }
                Err(e)
            }
        }
    }

    async fn get_prices(&self, coin_ids: &[&str]) -> Result<Vec<CryptoPrice>> {
        match self.client.get_prices(coin_ids).await {
            Ok(prices) => {
                *self.status.write().unwrap() = ProviderStatus::Healthy;
                Ok(prices
                    .into_iter()
                    .map(|p| CryptoPrice {
                        id: p.id,
                        symbol: p.symbol.to_uppercase(),
                        name: p.name,
                        current_price: p.current_price,
                        high_24h: p.high_24h,
                        low_24h: p.low_24h,
                        price_change_24h: p.price_change_24h,
                        price_change_percentage_24h: p.price_change_percentage_24h,
                        volume_24h: p.total_volume,
                        market_cap: Some(p.market_cap),
                        last_updated: Utc::now(),
                        source: "CoinGecko".to_string(),
                    })
                    .collect())
            }
            Err(e) => {
                *self.status.write().unwrap() = ProviderStatus::Error;
                Err(e)
            }
        }
    }

    async fn get_historical_prices(
        &self,
        coin_id: &str,
        days: u32,
    ) -> Result<Vec<(DateTime<Utc>, f64)>> {
        let chart = self.client.get_market_chart(coin_id, days).await?;

        let prices: Vec<(DateTime<Utc>, f64)> = chart
            .prices
            .into_iter()
            .filter_map(|(ts_ms, price)| {
                let dt = chrono::Utc.timestamp_millis_opt(ts_ms as i64).single()?;
                Some((dt, price))
            })
            .collect();

        Ok(prices)
    }

    async fn calculate_volatility(&self, coin_id: &str, days: u32) -> Result<VolatilityResult> {
        let vol = self.client.calculate_volatility(coin_id, days).await?;

        Ok(VolatilityResult {
            coin_id: vol.coin_id,
            annualized_volatility: vol.annualized_volatility,
            daily_volatility: vol.daily_volatility,
            periods_used: vol.days_used,
            calculated_at: vol.calculated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_chain_creation() {
        let chain = ChainedPriceProvider::new_default();
        assert!(chain.provider_name().contains("Coinbase"));
        assert!(chain.provider_name().contains("Binance"));
        assert!(chain.provider_name().contains("CoinGecko"));
    }

    #[tokio::test]
    async fn test_status_aggregation() {
        let chain = ChainedPriceProvider::new_default();
        // Default status should be healthy
        assert_eq!(chain.status(), ProviderStatus::Healthy);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_fallback_price_fetch() {
        let chain = ChainedPriceProvider::new_default();
        let price = chain.get_price("BTC").await.unwrap();
        assert!(price.current_price > 0.0);
        println!(
            "BTC price: ${:.2} from {}",
            price.current_price, price.source
        );
    }
}
