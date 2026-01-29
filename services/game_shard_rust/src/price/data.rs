//! Market price data structures
//!
//! Defines the core price data types used throughout the shard.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Market price data for a specific contract
#[derive(Debug, Clone)]
pub struct MarketPriceData {
    pub market_id: String,
    pub platform: String,
    pub contract_team: String,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub mid_price: f64,
    pub timestamp: DateTime<Utc>,
    /// Liquidity available at the yes bid (contracts available to sell)
    pub yes_bid_size: Option<f64>,
    /// Liquidity available at the yes ask (contracts available to buy)
    pub yes_ask_size: Option<f64>,
    /// Total liquidity in the market (if reported)
    pub total_liquidity: Option<f64>,
}

/// Incoming market price message from polymarket_monitor
#[derive(Debug, Deserialize)]
pub struct IncomingMarketPrice {
    pub market_id: String,
    pub platform: String,
    pub game_id: String,
    pub contract_team: Option<String>,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub mid_price: Option<f64>,
    pub implied_probability: Option<f64>,
    pub timestamp: Option<String>,
    /// Liquidity at the yes bid (contracts available to sell)
    pub yes_bid_size: Option<f64>,
    /// Liquidity at the yes ask (contracts available to buy)
    pub yes_ask_size: Option<f64>,
    /// Total market liquidity (optional)
    pub liquidity: Option<f64>,
}
