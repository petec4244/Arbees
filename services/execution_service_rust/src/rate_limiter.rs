//! Rate limiter for order execution
//!
//! Implements sliding window rate limiting to prevent excessive order placement.
//! Uses per-minute and per-hour limits to control order frequency.

use chrono::{DateTime, Duration, Utc};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Error returned when rate limit is exceeded
#[derive(Debug, Clone)]
pub struct RateLimitExceeded {
    pub limit_type: String,
    pub current_count: usize,
    pub limit: usize,
    pub retry_after_secs: i64,
}

impl std::fmt::Display for RateLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rate limit exceeded: {} orders in {} (limit: {}). Retry after {}s",
            self.current_count, self.limit_type, self.limit, self.retry_after_secs
        )
    }
}

impl std::error::Error for RateLimitExceeded {}

/// Sliding window rate limiter
pub struct RateLimiter {
    /// Timestamps of recent orders
    order_timestamps: Mutex<VecDeque<DateTime<Utc>>>,
    /// Maximum orders per minute
    max_per_minute: usize,
    /// Maximum orders per hour
    max_per_hour: usize,
}

impl RateLimiter {
    /// Create a new rate limiter with specified limits
    pub fn new(max_per_minute: usize, max_per_hour: usize) -> Self {
        Self {
            order_timestamps: Mutex::new(VecDeque::with_capacity(max_per_hour + 10)),
            max_per_minute,
            max_per_hour,
        }
    }

    /// Check if an order can be placed and record it if allowed
    ///
    /// Returns Ok(()) if the order is allowed, or Err(RateLimitExceeded) if limits are exceeded.
    pub fn check_and_record(&self) -> Result<(), RateLimitExceeded> {
        let now = Utc::now();
        let one_minute_ago = now - Duration::minutes(1);
        let one_hour_ago = now - Duration::hours(1);

        let mut timestamps = self.order_timestamps.lock().unwrap();

        // Clean up old entries (older than 1 hour)
        while timestamps.front().map_or(false, |ts| *ts < one_hour_ago) {
            timestamps.pop_front();
        }

        // Count orders in last minute
        let minute_count = timestamps.iter().filter(|ts| **ts >= one_minute_ago).count();

        if minute_count >= self.max_per_minute {
            // Find the oldest order in the last minute to calculate retry time
            let oldest_in_minute = timestamps
                .iter()
                .filter(|ts| **ts >= one_minute_ago)
                .min()
                .cloned()
                .unwrap_or(now);
            let retry_after = (oldest_in_minute + Duration::minutes(1) - now).num_seconds().max(1);

            return Err(RateLimitExceeded {
                limit_type: "minute".to_string(),
                current_count: minute_count,
                limit: self.max_per_minute,
                retry_after_secs: retry_after,
            });
        }

        // Count orders in last hour
        let hour_count = timestamps.len(); // All remaining are within last hour due to cleanup

        if hour_count >= self.max_per_hour {
            // Find the oldest order to calculate retry time
            let oldest = timestamps.front().cloned().unwrap_or(now);
            let retry_after = (oldest + Duration::hours(1) - now).num_seconds().max(1);

            return Err(RateLimitExceeded {
                limit_type: "hour".to_string(),
                current_count: hour_count,
                limit: self.max_per_hour,
                retry_after_secs: retry_after,
            });
        }

        // Record this order
        timestamps.push_back(now);

        Ok(())
    }

    /// Get current order counts (for monitoring)
    pub fn get_counts(&self) -> (usize, usize) {
        let now = Utc::now();
        let one_minute_ago = now - Duration::minutes(1);

        let timestamps = self.order_timestamps.lock().unwrap();
        let minute_count = timestamps.iter().filter(|ts| **ts >= one_minute_ago).count();
        let hour_count = timestamps.len();

        (minute_count, hour_count)
    }

    /// Reset the rate limiter (for testing)
    #[cfg(test)]
    pub fn reset(&self) {
        let mut timestamps = self.order_timestamps.lock().unwrap();
        timestamps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_within_limit() {
        let limiter = RateLimiter::new(5, 100);

        // Should allow 5 orders
        for _ in 0..5 {
            assert!(limiter.check_and_record().is_ok());
        }
    }

    #[test]
    fn test_rejects_over_minute_limit() {
        let limiter = RateLimiter::new(5, 100);

        // First 5 should succeed
        for _ in 0..5 {
            assert!(limiter.check_and_record().is_ok());
        }

        // 6th should fail
        let result = limiter.check_and_record();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.limit_type, "minute");
        assert_eq!(err.limit, 5);
    }

    #[test]
    fn test_get_counts() {
        let limiter = RateLimiter::new(10, 100);

        for _ in 0..3 {
            limiter.check_and_record().unwrap();
        }

        let (minute, hour) = limiter.get_counts();
        assert_eq!(minute, 3);
        assert_eq!(hour, 3);
    }
}
