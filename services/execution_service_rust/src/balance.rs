//! Balance tracking and validation module
//!
//! Maintains cached balance information for each platform and validates
//! that orders don't exceed available funds.

use anyhow::Result;
use arbees_rust_core::clients::kalshi::KalshiClient;
use chrono::{DateTime, Utc};
use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Balance buffer percentage (require 10% extra for safety)
const BALANCE_BUFFER_PCT: f64 = 0.10;

/// Cached balance for a platform
#[derive(Debug, Clone)]
pub struct PlatformBalance {
    pub balance: f64,
    pub last_updated: DateTime<Utc>,
    pub is_stale: bool,
}

impl Default for PlatformBalance {
    fn default() -> Self {
        Self {
            balance: 0.0,
            last_updated: DateTime::UNIX_EPOCH,
            is_stale: true,
        }
    }
}

/// Balance cache for all platforms
pub struct BalanceCache {
    kalshi: RwLock<PlatformBalance>,
    polymarket: RwLock<PlatformBalance>,
    /// How long before a balance is considered stale (seconds)
    stale_threshold_secs: i64,
}

impl BalanceCache {
    /// Create a new balance cache
    pub fn new(stale_threshold_secs: u64) -> Self {
        Self {
            kalshi: RwLock::new(PlatformBalance::default()),
            polymarket: RwLock::new(PlatformBalance::default()),
            stale_threshold_secs: stale_threshold_secs as i64,
        }
    }

    /// Get current Kalshi balance (from cache)
    pub async fn get_kalshi_balance(&self) -> PlatformBalance {
        self.kalshi.read().await.clone()
    }

    /// Get current Polymarket balance (from cache)
    pub async fn get_polymarket_balance(&self) -> PlatformBalance {
        self.polymarket.read().await.clone()
    }

    /// Update Kalshi balance
    pub async fn update_kalshi_balance(&self, balance: f64) {
        let mut cached = self.kalshi.write().await;
        cached.balance = balance;
        cached.last_updated = Utc::now();
        cached.is_stale = false;
        debug!("Updated Kalshi balance: ${:.2}", balance);
    }

    /// Update Polymarket balance
    pub async fn update_polymarket_balance(&self, balance: f64) {
        let mut cached = self.polymarket.write().await;
        cached.balance = balance;
        cached.last_updated = Utc::now();
        cached.is_stale = false;
        debug!("Updated Polymarket balance: ${:.2}", balance);
    }

    /// Check if Kalshi balance is stale
    pub async fn is_kalshi_stale(&self) -> bool {
        let cached = self.kalshi.read().await;
        let age = (Utc::now() - cached.last_updated).num_seconds();
        age > self.stale_threshold_secs || cached.is_stale
    }

    /// Check if Polymarket balance is stale
    pub async fn is_polymarket_stale(&self) -> bool {
        let cached = self.polymarket.read().await;
        let age = (Utc::now() - cached.last_updated).num_seconds();
        age > self.stale_threshold_secs || cached.is_stale
    }

    /// Validate that an order can be executed with current balance
    ///
    /// Returns Ok(()) if sufficient balance, Err with reason if not.
    pub async fn validate_order(
        &self,
        platform: &arbees_rust_core::models::Platform,
        order_value: f64,
    ) -> Result<(), String> {
        use arbees_rust_core::models::Platform;

        let required = order_value * (1.0 + BALANCE_BUFFER_PCT);

        match platform {
            Platform::Kalshi => {
                let cached = self.kalshi.read().await;
                if cached.is_stale {
                    warn!("Kalshi balance is stale, allowing order with warning");
                }
                if cached.balance < required {
                    return Err(format!(
                        "Insufficient Kalshi balance: ${:.2} available, ${:.2} required (including {}% buffer)",
                        cached.balance, required, (BALANCE_BUFFER_PCT * 100.0) as i32
                    ));
                }
            }
            Platform::Polymarket => {
                let cached = self.polymarket.read().await;
                if cached.is_stale {
                    warn!("Polymarket balance is stale, allowing order with warning");
                }
                if cached.balance < required {
                    return Err(format!(
                        "Insufficient Polymarket balance: ${:.2} available, ${:.2} required (including {}% buffer)",
                        cached.balance, required, (BALANCE_BUFFER_PCT * 100.0) as i32
                    ));
                }
            }
            Platform::Paper => {
                // Paper trading always has sufficient balance
                return Ok(());
            }
        }

        Ok(())
    }

    /// Mark Kalshi balance as stale (e.g., after an order is placed)
    pub async fn mark_kalshi_stale(&self) {
        let mut cached = self.kalshi.write().await;
        cached.is_stale = true;
    }

    /// Mark Polymarket balance as stale
    pub async fn mark_polymarket_stale(&self) {
        let mut cached = self.polymarket.write().await;
        cached.is_stale = true;
    }
}

/// Start background balance refresh loop
pub fn start_balance_refresh_loop(
    cache: Arc<BalanceCache>,
    kalshi: Arc<KalshiClient>,
    refresh_interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(refresh_interval_secs));

        loop {
            interval.tick().await;

            // Refresh Kalshi balance
            if kalshi.has_credentials() {
                match kalshi.get_balance().await {
                    Ok(balance) => {
                        cache.update_kalshi_balance(balance).await;
                        info!("Balance refresh: Kalshi ${:.2}", balance);
                    }
                    Err(e) => {
                        error!("Failed to refresh Kalshi balance: {}", e);
                        cache.mark_kalshi_stale().await;
                    }
                }
            }

            // TODO: Add Polymarket balance refresh when CLOB client supports it
        }
    })
}

/// Daily P&L tracker for loss limit monitoring
pub struct DailyPnlTracker {
    /// Running P&L for today (negative = loss)
    pnl: RwLock<f64>,
    /// Date of current tracking period
    tracking_date: RwLock<chrono::NaiveDate>,
}

impl DailyPnlTracker {
    pub fn new() -> Self {
        Self {
            pnl: RwLock::new(0.0),
            tracking_date: RwLock::new(Utc::now().date_naive()),
        }
    }

    /// Record a P&L event (positive = profit, negative = loss)
    pub async fn record_pnl(&self, amount: f64) {
        let today = Utc::now().date_naive();

        {
            let mut date = self.tracking_date.write().await;
            if *date != today {
                // New day, reset P&L
                info!("New trading day, resetting daily P&L");
                *date = today;
                let mut pnl = self.pnl.write().await;
                *pnl = 0.0;
            }
        }

        let mut pnl = self.pnl.write().await;
        *pnl += amount;
        debug!("Daily P&L updated: ${:.2} (change: ${:.2})", *pnl, amount);
    }

    /// Get current daily P&L
    pub async fn get_pnl(&self) -> f64 {
        *self.pnl.read().await
    }

    /// Check if daily loss limit is exceeded
    pub async fn is_loss_limit_exceeded(&self, max_daily_loss: f64) -> bool {
        let pnl = self.get_pnl().await;
        pnl < -max_daily_loss
    }

    /// Get loss limit utilization percentage (0.0 to 1.0+)
    pub async fn get_loss_utilization(&self, max_daily_loss: f64) -> f64 {
        let pnl = self.get_pnl().await;
        if pnl >= 0.0 {
            0.0
        } else {
            (-pnl) / max_daily_loss
        }
    }
}

impl Default for DailyPnlTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbees_rust_core::models::Platform;

    #[tokio::test]
    async fn test_balance_cache_update() {
        let cache = BalanceCache::new(60);

        cache.update_kalshi_balance(1000.0).await;

        let balance = cache.get_kalshi_balance().await;
        assert_eq!(balance.balance, 1000.0);
        assert!(!balance.is_stale);
    }

    #[tokio::test]
    async fn test_balance_validation_sufficient() {
        let cache = BalanceCache::new(60);
        cache.update_kalshi_balance(1000.0).await;

        // Order for $100 should pass (requires $110 with 10% buffer)
        let result = cache.validate_order(&Platform::Kalshi, 100.0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_balance_validation_insufficient() {
        let cache = BalanceCache::new(60);
        cache.update_kalshi_balance(100.0).await;

        // Order for $100 requires $110, but only $100 available
        let result = cache.validate_order(&Platform::Kalshi, 100.0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_paper_always_sufficient() {
        let cache = BalanceCache::new(60);
        // No balance set

        let result = cache.validate_order(&Platform::Paper, 1000000.0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_daily_pnl_tracking() {
        let tracker = DailyPnlTracker::new();

        tracker.record_pnl(-50.0).await;
        assert_eq!(tracker.get_pnl().await, -50.0);

        tracker.record_pnl(30.0).await;
        assert_eq!(tracker.get_pnl().await, -20.0);
    }

    #[tokio::test]
    async fn test_loss_limit_check() {
        let tracker = DailyPnlTracker::new();

        tracker.record_pnl(-400.0).await;
        assert!(!tracker.is_loss_limit_exceeded(500.0).await);

        tracker.record_pnl(-150.0).await;
        assert!(tracker.is_loss_limit_exceeded(500.0).await);
    }
}
