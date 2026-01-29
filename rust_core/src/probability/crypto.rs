//! Crypto Probability Model
//!
//! Calculates probability of crypto price targets being hit based on:
//! - Current price vs target price
//! - Time remaining until target date
//! - Historical volatility
//! - Distance from ATH/ATL

use super::ProbabilityModel;
use crate::clients::coingecko::CoinGeckoClient;
use crate::models::{GameState, MarketType};
use crate::providers::{EventState, StateData};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Crypto probability model using Black-Scholes inspired calculation
pub struct CryptoProbabilityModel {
    /// CoinGecko client for fetching live volatility data
    coingecko: Arc<CoinGeckoClient>,
    /// Cache for volatility values
    volatility_cache: Arc<RwLock<std::collections::HashMap<String, (f64, DateTime<Utc>)>>>,
    /// Default volatility if we can't fetch (annualized)
    default_volatility: f64,
}

impl CryptoProbabilityModel {
    /// Create a new crypto probability model
    pub fn new() -> Self {
        Self::with_coingecko(Arc::new(CoinGeckoClient::new()))
    }

    /// Create with custom CoinGecko client (for testing/sharing)
    pub fn with_coingecko(coingecko: Arc<CoinGeckoClient>) -> Self {
        Self {
            coingecko,
            volatility_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            default_volatility: 0.80, // 80% annualized volatility default for crypto
        }
    }

    /// Get volatility for a coin, with caching
    async fn get_volatility(&self, coin_id: &str) -> f64 {
        // Check cache (1 hour TTL)
        {
            let cache = self.volatility_cache.read().await;
            if let Some((vol, fetched_at)) = cache.get(coin_id) {
                let age = Utc::now().signed_duration_since(*fetched_at).num_seconds();
                if age < 3600 {
                    return *vol;
                }
            }
        }

        // Fetch from CoinGecko
        match self.coingecko.calculate_volatility(coin_id, 30).await {
            Ok(vol_data) => {
                let vol = vol_data.annualized_volatility;
                // Update cache
                let mut cache = self.volatility_cache.write().await;
                cache.insert(coin_id.to_string(), (vol, Utc::now()));
                debug!("Fetched volatility for {}: {:.2}%", coin_id, vol * 100.0);
                vol
            }
            Err(e) => {
                warn!(
                    "Failed to fetch volatility for {}: {}. Using default.",
                    coin_id, e
                );
                self.default_volatility
            }
        }
    }

    /// Calculate probability of price reaching target by date
    ///
    /// Uses a log-normal model similar to Black-Scholes:
    /// P(S_T > K) = N(d2) where d2 = [ln(S/K) + (μ - σ²/2)T] / (σ√T)
    ///
    /// For simplicity, we assume μ = 0 (drift-neutral), giving:
    /// d2 = [ln(S/K) - σ²T/2] / (σ√T)
    ///
    /// When current_price >= target_price, applies a time-decay factor:
    /// This prevents overconfident signals when price is already at target but
    /// has significant time remaining (price could fall back).
    pub fn calculate_price_target_probability(
        current_price: f64,
        target_price: f64,
        days_remaining: f64,
        annualized_volatility: f64,
    ) -> f64 {
        if current_price <= 0.0 || target_price <= 0.0 || days_remaining <= 0.0 {
            return if current_price >= target_price { 1.0 } else { 0.0 };
        }

        // Convert annualized volatility to the time period
        let t = days_remaining / 365.0;
        let sigma = annualized_volatility;

        // Calculate d2 for log-normal distribution
        // d2 = [ln(S/K) - σ²T/2] / (σ√T)
        let ln_ratio = (current_price / target_price).ln();
        let sigma_sqrt_t = sigma * t.sqrt();
        let d2 = (ln_ratio - (sigma * sigma * t / 2.0)) / sigma_sqrt_t;

        // Probability of being above target = N(d2) for current > target
        // Probability of being above target = N(-d2) for current < target
        if current_price >= target_price {
            // Already above target - probability of staying above
            // Apply time-decay factor: more time remaining = higher chance of falling back
            let base_prob = normal_cdf(d2);
            let time_decay_factor = (-2.0 * t).exp(); // Decays from 1.0 at t=0 to ~0.14 at t=1 year
            base_prob * time_decay_factor
        } else {
            // Below target - probability of rising above
            normal_cdf(-d2)
        }
    }

    /// Adjust probability based on distance from ATH/ATL
    ///
    /// If target is above ATH, reduce probability (resistance)
    /// If target is below ATL, reduce probability (support)
    pub fn adjust_for_ath_atl(
        base_prob: f64,
        current_price: f64,
        target_price: f64,
        ath: f64,
        atl: f64,
    ) -> f64 {
        // If target requires breaking ATH, apply resistance factor
        if target_price > ath && current_price < ath {
            let ath_distance_pct = (target_price - ath) / ath;
            // Reduce probability based on how far above ATH the target is
            let resistance_factor = 1.0 / (1.0 + ath_distance_pct * 2.0);
            return base_prob * resistance_factor;
        }

        // If target is below ATL, apply support factor (less likely to break)
        if target_price < atl && current_price > atl {
            let atl_distance_pct = (atl - target_price) / atl;
            let support_factor = 1.0 / (1.0 + atl_distance_pct * 2.0);
            return base_prob * support_factor;
        }

        base_prob
    }
}

impl Default for CryptoProbabilityModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProbabilityModel for CryptoProbabilityModel {
    async fn calculate_probability(
        &self,
        event_state: &EventState,
        for_entity_a: bool,
    ) -> Result<f64> {
        // Extract crypto state
        let StateData::Crypto(crypto_state) = &event_state.state else {
            return Err(anyhow!("Expected crypto state data"));
        };

        // Get volatility for this asset
        let volatility = self.get_volatility(&event_state.entity_a).await;

        // Calculate days remaining
        let now = Utc::now();
        let days_remaining = (crypto_state.target_date - now).num_seconds() as f64 / 86400.0;

        // Calculate base probability
        let prob = Self::calculate_price_target_probability(
            crypto_state.current_price,
            crypto_state.target_price,
            days_remaining.max(0.001), // Avoid division by zero
            volatility,
        );

        // For "entity_a wins" (price above target), return prob
        // For "entity_b wins" (price below target), return 1 - prob
        if for_entity_a {
            Ok(prob.clamp(0.01, 0.99))
        } else {
            Ok((1.0 - prob).clamp(0.01, 0.99))
        }
    }

    async fn calculate_probability_legacy(
        &self,
        game_state: &GameState,
        for_home_team: bool,
    ) -> Result<f64> {
        // Extract crypto state from market_specific if available
        let crypto_state = match &game_state.market_specific {
            Some(crate::models::MarketSpecificState::Crypto(state)) => state.clone(),
            _ => {
                return Err(anyhow!("No crypto state in legacy GameState"));
            }
        };

        // Get volatility
        let coin_id = game_state
            .entity_a
            .as_ref()
            .unwrap_or(&game_state.home_team);
        let volatility = self.get_volatility(coin_id).await;

        // Calculate days remaining
        let now = Utc::now();
        let days_remaining = (crypto_state.target_date - now).num_seconds() as f64 / 86400.0;

        let prob = Self::calculate_price_target_probability(
            crypto_state.current_price,
            crypto_state.target_price,
            days_remaining.max(0.001),
            volatility,
        );

        if for_home_team {
            Ok(prob.clamp(0.01, 0.99))
        } else {
            Ok((1.0 - prob).clamp(0.01, 0.99))
        }
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Crypto { .. })
    }

    fn model_name(&self) -> &str {
        "CryptoProbability"
    }
}

/// Standard normal CDF approximation (Abramowitz and Stegun)
fn normal_cdf(x: f64) -> f64 {
    if x < -8.0 {
        return 0.0;
    }
    if x > 8.0 {
        return 1.0;
    }

    // Constants for approximation
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs() / (2.0_f64).sqrt();

    let t = 1.0 / (1.0 + p * x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();

    0.5 * (1.0 + sign * y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CryptoPredictionType;
    use crate::providers::{CryptoStateData, EventStatus};

    #[test]
    fn test_normal_cdf() {
        // Test standard normal CDF values
        assert!((normal_cdf(0.0) - 0.5).abs() < 0.001);
        assert!((normal_cdf(-1.96) - 0.025).abs() < 0.01);
        assert!((normal_cdf(1.96) - 0.975).abs() < 0.01);
        assert!(normal_cdf(-8.0) < 0.001);
        assert!(normal_cdf(8.0) > 0.999);
    }

    #[test]
    fn test_price_target_probability() {
        // Current price = target price, should be < 50% due to time decay
        // (with 30 days remaining, price can fall back)
        let prob = CryptoProbabilityModel::calculate_price_target_probability(
            100000.0, // current
            100000.0, // target
            30.0,     // 30 days
            0.80,     // 80% annual vol
        );
        // With time decay factor: e^(-2*30/365) ≈ 0.839
        // Base probability at target ~0.5, decayed ≈ 0.42
        assert!(prob > 0.30 && prob < 0.50, "At-target prob with decay: {}", prob);

        // Current price well above target, should be high but with decay
        // Decay factor significantly reduces extreme confidence when price just barely above
        let prob = CryptoProbabilityModel::calculate_price_target_probability(
            100000.0, // current
            50000.0,  // target (50% below)
            30.0,     // 30 days
            0.80,     // 80% annual vol
        );
        // Even well above target, time decay limits extreme confidence
        assert!(prob > 0.70 && prob < 0.95, "Above-target prob with decay: {}", prob);

        // With high volatility and being below target, longer time allows more random walk
        // Both probabilities should be meaningful (not extreme)
        let prob_short = CryptoProbabilityModel::calculate_price_target_probability(
            80000.0,  // current
            100000.0, // target (+25%)
            30.0,     // 30 days
            0.80,
        );
        let prob_long = CryptoProbabilityModel::calculate_price_target_probability(
            80000.0,  // current
            100000.0, // target (+25%)
            180.0,    // 180 days
            0.80,
        );
        // Both should be reasonable probabilities (not extreme)
        assert!(
            prob_short > 0.3 && prob_short < 0.95,
            "Short term prob should be reasonable: {}",
            prob_short
        );
        assert!(
            prob_long > 0.3 && prob_long < 0.95,
            "Long term prob should be reasonable: {}",
            prob_long
        );

        // Higher volatility = more uncertainty = probability can vary
        // Both should be reasonable probabilities
        let prob_low_vol = CryptoProbabilityModel::calculate_price_target_probability(
            80000.0, 100000.0, 30.0, 0.30,
        );
        let prob_high_vol = CryptoProbabilityModel::calculate_price_target_probability(
            80000.0, 100000.0, 30.0, 1.50,
        );
        // Both should be meaningful probabilities
        assert!(
            prob_low_vol > 0.1 && prob_low_vol < 1.0,
            "Low vol prob: {}",
            prob_low_vol
        );
        assert!(
            prob_high_vol > 0.1 && prob_high_vol < 1.0,
            "High vol prob: {}",
            prob_high_vol
        );
    }

    #[test]
    fn test_ath_atl_adjustment() {
        // Base probability without adjustment
        let base_prob = 0.60;

        // Target above ATH should reduce probability
        let adjusted = CryptoProbabilityModel::adjust_for_ath_atl(
            base_prob,
            95000.0,  // current
            110000.0, // target
            100000.0, // ATH
            30000.0,  // ATL
        );
        assert!(adjusted < base_prob, "ATH resistance: {} vs {}", adjusted, base_prob);

        // Target within range should not change probability
        let adjusted = CryptoProbabilityModel::adjust_for_ath_atl(
            base_prob,
            70000.0, // current
            90000.0, // target
            100000.0, // ATH
            30000.0, // ATL
        );
        assert!((adjusted - base_prob).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_crypto_model_supports() {
        let model = CryptoProbabilityModel::new();
        assert_eq!(model.model_name(), "CryptoProbability");

        let crypto_market = MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        };
        assert!(model.supports(&crypto_market));

        let sport_market = MarketType::sport(crate::models::Sport::NBA);
        assert!(!model.supports(&sport_market));
    }

    #[tokio::test]
    async fn test_crypto_model_calculation() {
        let model = CryptoProbabilityModel::new();

        // Create event state for BTC price target
        let target_date = Utc::now() + chrono::Duration::days(60);
        let event_state = EventState {
            event_id: "btc-100k-2026".to_string(),
            market_type: MarketType::Crypto {
                asset: "BTC".to_string(),
                prediction_type: CryptoPredictionType::PriceTarget,
            },
            entity_a: "bitcoin".to_string(),
            entity_b: None,
            status: EventStatus::Live,
            state: StateData::Crypto(CryptoStateData {
                current_price: 95000.0,
                target_price: 100000.0,
                target_date,
                volatility_24h: 0.03,
                volume_24h: Some(50_000_000_000.0),
                metadata: serde_json::json!({}),
            }),
            fetched_at: Utc::now(),
        };

        let prob = model.calculate_probability(&event_state, true).await.unwrap();

        // With ~5% needed gain over 60 days, probability should be meaningful but not certain
        assert!(prob > 0.30 && prob < 0.80, "BTC prob: {}", prob);

        // Opposite side should be complement
        let prob_no = model.calculate_probability(&event_state, false).await.unwrap();
        assert!((prob + prob_no - 1.0).abs() < 0.01);
    }
}
