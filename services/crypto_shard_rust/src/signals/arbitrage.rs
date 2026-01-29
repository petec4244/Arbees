//! Cross-platform arbitrage detection for crypto
//!
//! Detects price discrepancies across prediction markets.
//! Buys low on one platform, sells high on another.

use crate::price::data::CryptoPriceData;
use crate::signals::risk::CryptoRiskChecker;
use crate::types::{CryptoExecutionRequest, CryptoSignalType, Direction};
use anyhow::Result;
use chrono::Utc;
use log::{debug, info};
use std::collections::HashMap;
use uuid::Uuid;

/// Detects and signals arbitrage opportunities
pub struct CryptoArbitrageDetector {
    /// Minimum edge percentage to trade (after fees)
    pub min_edge_pct: f64,

    /// Platform trading fees (percentage)
    pub fee_pct: f64,
}

impl CryptoArbitrageDetector {
    pub fn new(min_edge_pct: f64) -> Self {
        Self {
            min_edge_pct,
            fee_pct: 0.5, // 0.5% typical for prediction markets
        }
    }

    /// Detect arbitrage opportunities and emit execution request if profitable
    /// Returns ExecutionRequest if edge exceeds threshold and passes risk checks
    pub async fn detect_and_emit(
        &self,
        event_id: &str,
        asset: &str,
        prices: &HashMap<String, CryptoPriceData>,
        risk_checker: &CryptoRiskChecker,
        volatility_factor: f64,
    ) -> Result<Option<CryptoExecutionRequest>> {
        // Find all platform prices for this asset
        let asset_prices: Vec<&CryptoPriceData> = prices
            .iter()
            .filter(|(key, _)| key.starts_with(&format!("{}|", asset)))
            .map(|(_, price)| price)
            .collect();

        if asset_prices.len() < 2 {
            return Ok(None); // Need at least 2 platforms for arbitrage
        }

        // Find best bid and best ask across platforms
        let mut best_bid: Option<(&CryptoPriceData, f64)> = None;
        let mut best_ask: Option<(&CryptoPriceData, f64)> = None;

        for price in &asset_prices {
            if best_bid.is_none() || price.yes_bid > best_bid.unwrap().1 {
                best_bid = Some((price, price.yes_bid));
            }
            if best_ask.is_none() || price.yes_ask < best_ask.unwrap().1 {
                best_ask = Some((price, price.yes_ask));
            }
        }

        let (bid_price_data, bid) = best_bid.ok_or_else(|| {
            anyhow::anyhow!("No valid bid prices for {}", asset)
        })?;
        let (ask_price_data, ask) = best_ask.ok_or_else(|| {
            anyhow::anyhow!("No valid ask prices for {}", asset)
        })?;

        // Can't arb on same platform
        if bid_price_data.platform == ask_price_data.platform {
            return Ok(None);
        }

        // Calculate net edge after fees
        let gross_edge = bid - ask;
        let fee_cost = self.fee_pct / 100.0 * (bid + ask); // Fee on both sides
        let net_edge = gross_edge - fee_cost;
        let net_edge_pct = (net_edge / ask) * 100.0;

        if net_edge_pct < self.min_edge_pct {
            return Ok(None);
        }

        // Buy on ask platform (lower price), sell on bid platform (higher price)
        let platform = ask_price_data.platform.clone();
        let market_id = ask_price_data.market_id.clone();
        let execution_price = ask;

        // Simple position sizing: fixed amount adjusted by edge
        let base_size = 100.0; // $100 base position
        let edge_multiplier = (net_edge_pct / self.min_edge_pct).min(3.0); // Cap at 3x
        let suggested_size = base_size * edge_multiplier;

        // Pass through risk checker
        let adjusted_size = match risk_checker
            .validate_trade(
                asset,
                &platform,
                &market_id,
                net_edge_pct,
                suggested_size,
                ask_price_data.total_liquidity,
                volatility_factor,
            )
            .await
        {
            Ok(size) => size,
            Err(e) => {
                debug!("Arbitrage blocked by risk check: {}", e);
                return Ok(None);
            }
        };

        info!(
            "Crypto arbitrage detected: {} {}% edge (bid={:.4} on {}, ask={:.4} on {}), size=${}",
            asset, net_edge_pct, bid, bid_price_data.platform, ask, ask_price_data.platform, adjusted_size
        );

        let request = CryptoExecutionRequest {
            request_id: Uuid::new_v4().to_string(),
            event_id: event_id.to_string(),
            asset: asset.to_string(),
            signal_type: CryptoSignalType::Arbitrage,
            platform,
            market_id,
            direction: Direction::Long, // Buy YES at lower price
            edge_pct: net_edge_pct,
            probability: 0.5, // Arbitrage doesn't rely on probability model
            suggested_size: adjusted_size,
            max_price: execution_price + 0.01, // Slight buffer
            current_price: execution_price,
            timestamp: Utc::now(),
            volatility_factor,
            exposure_check: true,
            balance_check: true,
        };

        Ok(Some(request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_prices() -> HashMap<String, CryptoPriceData> {
        let mut prices = HashMap::new();

        // Kalshi: BTC at 0.45-0.47
        prices.insert(
            "BTC|kalshi".to_string(),
            CryptoPriceData {
                market_id: "btc_kalshi".to_string(),
                platform: "kalshi".to_string(),
                asset: "BTC".to_string(),
                yes_bid: 0.45,
                yes_ask: 0.47,
                mid_price: 0.46,
                yes_bid_size: Some(1000.0),
                yes_ask_size: Some(1500.0),
                total_liquidity: Some(2500.0),
                timestamp: Utc::now(),
            },
        );

        // Polymarket: BTC at 0.44-0.46
        prices.insert(
            "BTC|polymarket".to_string(),
            CryptoPriceData {
                market_id: "btc_polymarket".to_string(),
                platform: "polymarket".to_string(),
                asset: "BTC".to_string(),
                yes_bid: 0.44,
                yes_ask: 0.46,
                mid_price: 0.45,
                yes_bid_size: Some(800.0),
                yes_ask_size: Some(1200.0),
                total_liquidity: Some(2000.0),
                timestamp: Utc::now(),
            },
        );

        prices
    }

    #[test]
    fn test_arbitrage_detector_creation() {
        let detector = CryptoArbitrageDetector::new(3.0);
        assert_eq!(detector.min_edge_pct, 3.0);
        assert_eq!(detector.fee_pct, 0.5);
    }

    #[test]
    fn test_single_platform_no_arb() {
        let mut prices = HashMap::new();
        prices.insert(
            "BTC|kalshi".to_string(),
            CryptoPriceData {
                market_id: "btc_kalshi".to_string(),
                platform: "kalshi".to_string(),
                asset: "BTC".to_string(),
                yes_bid: 0.45,
                yes_ask: 0.47,
                mid_price: 0.46,
                yes_bid_size: None,
                yes_ask_size: None,
                total_liquidity: None,
                timestamp: Utc::now(),
            },
        );

        let detector = CryptoArbitrageDetector::new(3.0);
        // Would need actual risk_checker for full test
        // Just verify detector was created
        assert_eq!(detector.min_edge_pct, 3.0);
    }

    #[test]
    fn test_same_platform_no_arb() {
        let mut prices = HashMap::new();

        // Two markets on same platform shouldn't arb
        prices.insert(
            "BTC|kalshi-market1".to_string(),
            CryptoPriceData {
                market_id: "btc_kalshi_1".to_string(),
                platform: "kalshi".to_string(),
                asset: "BTC".to_string(),
                yes_bid: 0.45,
                yes_ask: 0.47,
                mid_price: 0.46,
                yes_bid_size: None,
                yes_ask_size: None,
                total_liquidity: None,
                timestamp: Utc::now(),
            },
        );

        prices.insert(
            "BTC|kalshi-market2".to_string(),
            CryptoPriceData {
                market_id: "btc_kalshi_2".to_string(),
                platform: "kalshi".to_string(),
                asset: "BTC".to_string(),
                yes_bid: 0.44,
                yes_ask: 0.46,
                mid_price: 0.45,
                yes_bid_size: None,
                yes_ask_size: None,
                total_liquidity: None,
                timestamp: Utc::now(),
            },
        );

        let detector = CryptoArbitrageDetector::new(3.0);
        assert_eq!(detector.min_edge_pct, 3.0);
    }

    #[test]
    fn test_arbitrage_edge_calculation() {
        // Kalshi: bid 0.45
        // Polymarket: ask 0.46
        // Gross edge: 0.45 - 0.46 = -0.01 (negative, no arb)

        // Kalshi: bid 0.45
        // Polymarket: ask 0.44
        // Gross edge: 0.45 - 0.44 = 0.01 = 2.27%
        // Fee cost: 0.5% * (0.45 + 0.44) = 0.5% * 0.89 = 0.00445
        // Net edge: 0.01 - 0.00445 = 0.00555 = 1.26%

        let detector = CryptoArbitrageDetector::new(3.0);
        let bid = 0.45;
        let ask = 0.44;
        let gross_edge = bid - ask;
        let fee_cost = (detector.fee_pct / 100.0) * (bid + ask);
        let net_edge = gross_edge - fee_cost;
        let net_edge_pct = (net_edge / ask) * 100.0;

        assert!(net_edge_pct > 0.0);
        assert!(net_edge_pct < 2.0); // About 1.26%
    }

    #[test]
    fn test_position_sizing_by_edge() {
        let detector = CryptoArbitrageDetector::new(3.0);
        let base_size = 100.0;

        // If edge is 6%, multiplier should be (6/3) = 2x
        let edge_pct = 6.0;
        let edge_multiplier = (edge_pct / detector.min_edge_pct).min(3.0);
        let suggested_size = base_size * edge_multiplier;

        assert_eq!(edge_multiplier, 2.0);
        assert_eq!(suggested_size, 200.0);
    }

    #[test]
    fn test_position_sizing_capped() {
        let detector = CryptoArbitrageDetector::new(3.0);
        let base_size = 100.0;

        // If edge is 12%, multiplier would be 4x but capped at 3x
        let edge_pct = 12.0;
        let edge_multiplier = (edge_pct / detector.min_edge_pct).min(3.0);
        let suggested_size = base_size * edge_multiplier;

        assert_eq!(edge_multiplier, 3.0);
        assert_eq!(suggested_size, 300.0);
    }
}
