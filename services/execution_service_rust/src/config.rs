//! Configuration module for execution service safeguards
//!
//! Centralizes all safeguard settings with safe defaults loaded from environment variables.

use std::env;

/// Configuration for all execution safeguards
#[derive(Debug, Clone)]
pub struct SafeguardConfig {
    // Authorization
    /// Requires explicit LIVE_TRADING_AUTHORIZED=true when PAPER_TRADING=0
    pub live_trading_authorized: bool,

    // Order Size Limits
    /// Maximum dollar value per order (default: $100)
    pub max_order_size: f64,
    /// Maximum contracts per order (default: 100)
    pub max_order_contracts: i32,
    /// Maximum position value per market (default: $200)
    pub max_position_per_market: f64,

    // Rate Limiting
    /// Maximum orders per minute (default: 20)
    pub max_orders_per_minute: usize,
    /// Maximum orders per hour (default: 100)
    pub max_orders_per_hour: usize,

    // Price Safety
    /// Minimum acceptable price (default: 0.05)
    pub min_safe_price: f64,
    /// Maximum acceptable price (default: 0.95)
    pub max_safe_price: f64,

    // Balance
    /// How often to refresh balance from exchange (default: 60 seconds)
    pub balance_refresh_secs: u64,
    /// Balance threshold for warnings (default: $100)
    pub balance_low_threshold: f64,
    /// Maximum daily loss before halt (default: $500)
    pub max_daily_loss: f64,

    // Audit
    /// Whether audit logging is enabled (default: true)
    pub audit_log_enabled: bool,
}

impl SafeguardConfig {
    /// Load configuration from environment variables with safe defaults
    pub fn from_env() -> Self {
        Self {
            live_trading_authorized: env::var("LIVE_TRADING_AUTHORIZED")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),

            max_order_size: env::var("MAX_ORDER_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100.0),

            max_order_contracts: env::var("MAX_ORDER_CONTRACTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),

            max_position_per_market: env::var("MAX_POSITION_PER_MARKET")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200.0),

            max_orders_per_minute: env::var("MAX_ORDERS_PER_MINUTE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),

            max_orders_per_hour: env::var("MAX_ORDERS_PER_HOUR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),

            min_safe_price: env::var("MIN_SAFE_PRICE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.05),

            max_safe_price: env::var("MAX_SAFE_PRICE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.95),

            balance_refresh_secs: env::var("BALANCE_REFRESH_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),

            balance_low_threshold: env::var("BALANCE_LOW_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100.0),

            max_daily_loss: env::var("MAX_DAILY_LOSS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500.0),

            audit_log_enabled: env::var("AUDIT_LOG_ENABLED")
                .map(|v| v.to_lowercase() != "false")
                .unwrap_or(true),
        }
    }

    /// Log current configuration (useful at startup)
    pub fn log_config(&self) {
        log::info!("SafeguardConfig loaded:");
        log::info!("  live_trading_authorized: {}", self.live_trading_authorized);
        log::info!("  max_order_size: ${:.2}", self.max_order_size);
        log::info!("  max_order_contracts: {}", self.max_order_contracts);
        log::info!("  max_position_per_market: ${:.2}", self.max_position_per_market);
        log::info!("  max_orders_per_minute: {}", self.max_orders_per_minute);
        log::info!("  max_orders_per_hour: {}", self.max_orders_per_hour);
        log::info!("  price_range: {:.2}-{:.2}", self.min_safe_price, self.max_safe_price);
        log::info!("  balance_refresh_secs: {}s", self.balance_refresh_secs);
        log::info!("  balance_low_threshold: ${:.2}", self.balance_low_threshold);
        log::info!("  max_daily_loss: ${:.2}", self.max_daily_loss);
        log::info!("  audit_log_enabled: {}", self.audit_log_enabled);
    }
}

impl Default for SafeguardConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SafeguardConfig::from_env();

        // Verify conservative defaults
        assert_eq!(config.max_order_size, 100.0);
        assert_eq!(config.max_order_contracts, 100);
        assert_eq!(config.max_orders_per_minute, 20);
        assert_eq!(config.max_orders_per_hour, 100);
        assert_eq!(config.min_safe_price, 0.05);
        assert_eq!(config.max_safe_price, 0.95);
        assert_eq!(config.max_daily_loss, 500.0);
        assert!(!config.live_trading_authorized); // Must be explicitly enabled
    }
}
