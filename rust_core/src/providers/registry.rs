//! Event Provider Registry
//!
//! Manages event providers for all market types and provides unified access
//! to event data regardless of the underlying source.

use super::{EventInfo, EventProvider, EventState};
use crate::models::{MarketType, Sport};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Registry of event providers for all market types
///
/// Manages multiple providers and routes requests to the appropriate one
/// based on market type.
pub struct EventProviderRegistry {
    /// Providers indexed by market type key
    providers: HashMap<String, Arc<dyn EventProvider>>,
}

impl EventProviderRegistry {
    /// Create an empty registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Create a registry with default providers for all supported market types
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();

        // Register ESPN providers for all sports
        for sport in [
            Sport::NBA,
            Sport::NFL,
            Sport::NHL,
            Sport::MLB,
            Sport::NCAAF,
            Sport::NCAAB,
            Sport::MLS,
            Sport::Soccer,
            Sport::Tennis,
            Sport::MMA,
        ] {
            let provider = Arc::new(super::espn::EspnEventProvider::new(sport));
            let key = format!("sport:{}", sport.as_str().to_lowercase());
            registry.providers.insert(key, provider);
        }

        // Register crypto provider
        let crypto_provider = Arc::new(super::crypto::CryptoEventProvider::new());
        registry.providers.insert("crypto".to_string(), crypto_provider);

        // Register economics provider
        let economics_provider = Arc::new(super::economics::EconomicsEventProvider::new());
        registry.providers.insert("economics".to_string(), economics_provider);

        // Register politics provider
        let politics_provider = Arc::new(super::politics::PoliticsEventProvider::new());
        registry.providers.insert("politics".to_string(), politics_provider);

        info!(
            "EventProviderRegistry initialized with {} providers",
            registry.providers.len()
        );

        registry
    }

    /// Register a custom provider
    pub fn register(&mut self, key: &str, provider: Arc<dyn EventProvider>) {
        info!("Registering provider: {}", key);
        self.providers.insert(key.to_string(), provider);
    }

    /// Get the provider key for a given market type
    fn market_type_to_key(&self, market_type: &MarketType) -> String {
        match market_type {
            MarketType::Sport { sport } => format!("sport:{}", sport.as_str().to_lowercase()),
            MarketType::Crypto { .. } => "crypto".to_string(),
            MarketType::Economics { .. } => "economics".to_string(),
            MarketType::Politics { .. } => "politics".to_string(),
            MarketType::Entertainment { .. } => "entertainment".to_string(),
        }
    }

    /// Get the provider for a given market type
    pub fn get_provider(&self, market_type: &MarketType) -> Option<Arc<dyn EventProvider>> {
        let key = self.market_type_to_key(market_type);
        self.providers.get(&key).cloned()
    }

    /// Get the provider for a given market type, returning an error if not found
    pub fn get_provider_required(
        &self,
        market_type: &MarketType,
    ) -> Result<Arc<dyn EventProvider>> {
        self.get_provider(market_type)
            .ok_or_else(|| anyhow!("No provider registered for market type: {:?}", market_type))
    }

    /// Get event state using the appropriate provider
    pub async fn get_event_state(
        &self,
        event_id: &str,
        market_type: &MarketType,
    ) -> Result<EventState> {
        let provider = self.get_provider_required(market_type)?;
        debug!(
            "Fetching event state for {} using provider {}",
            event_id,
            provider.provider_name()
        );
        provider.get_event_state(event_id).await
    }

    /// Get live events for a given market type
    pub async fn get_live_events(&self, market_type: &MarketType) -> Result<Vec<EventInfo>> {
        let provider = self.get_provider_required(market_type)?;
        provider.get_live_events().await
    }

    /// Get scheduled events for a given market type
    pub async fn get_scheduled_events(
        &self,
        market_type: &MarketType,
        days: u32,
    ) -> Result<Vec<EventInfo>> {
        let provider = self.get_provider_required(market_type)?;
        provider.get_scheduled_events(days).await
    }

    /// Get all live events across all providers
    pub async fn get_all_live_events(&self) -> Vec<EventInfo> {
        let mut all_events = Vec::new();

        for (key, provider) in &self.providers {
            match provider.get_live_events().await {
                Ok(events) => {
                    debug!(
                        "Provider {} returned {} live events",
                        key,
                        events.len()
                    );
                    all_events.extend(events);
                }
                Err(e) => {
                    warn!("Provider {} failed to fetch live events: {}", key, e);
                }
            }
        }

        all_events
    }

    /// List all registered provider keys
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Check if a provider is registered for a market type
    pub fn has_provider(&self, market_type: &MarketType) -> bool {
        self.get_provider(market_type).is_some()
    }
}

impl Default for EventProviderRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = EventProviderRegistry::new();
        assert!(registry.providers.is_empty());
    }

    #[test]
    fn test_registry_with_defaults() {
        let registry = EventProviderRegistry::with_defaults();
        // Should have 10 sports + 3 other markets = 13 providers
        assert!(registry.providers.len() >= 10);
        assert!(registry.has_provider(&MarketType::sport(Sport::NBA)));
        assert!(registry.has_provider(&MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: crate::models::CryptoPredictionType::PriceTarget,
        }));
    }

    #[test]
    fn test_market_type_to_key() {
        let registry = EventProviderRegistry::new();

        assert_eq!(
            registry.market_type_to_key(&MarketType::sport(Sport::NBA)),
            "sport:nba"
        );
        assert_eq!(
            registry.market_type_to_key(&MarketType::sport(Sport::NFL)),
            "sport:nfl"
        );
        assert_eq!(
            registry.market_type_to_key(&MarketType::Crypto {
                asset: "BTC".to_string(),
                prediction_type: crate::models::CryptoPredictionType::PriceTarget,
            }),
            "crypto"
        );
        assert_eq!(
            registry.market_type_to_key(&MarketType::Economics {
                indicator: crate::models::EconomicIndicator::CPI,
                threshold: None,
            }),
            "economics"
        );
    }

    #[test]
    fn test_list_providers() {
        let registry = EventProviderRegistry::with_defaults();
        let providers = registry.list_providers();
        assert!(providers.contains(&"sport:nba".to_string()));
        assert!(providers.contains(&"crypto".to_string()));
    }
}
