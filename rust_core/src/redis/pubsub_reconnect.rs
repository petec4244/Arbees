//! Redis PubSub with automatic reconnection and exponential backoff
//!
//! This module provides a reconnecting PubSub wrapper that automatically
//! handles connection failures and resubscribes to channels/patterns.

use anyhow::{Context, Result};
use futures_util::stream::{Stream, StreamExt};
use redis::{aio::PubSub, Client, Msg};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Configuration for reconnection behavior
#[derive(Clone, Debug)]
pub struct ReconnectConfig {
    /// Maximum consecutive failures before circuit breaker opens (default: 10)
    pub max_consecutive_failures: u32,
    /// Base delay in milliseconds for exponential backoff (default: 1000ms)
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (default: 60000ms = 1 minute)
    pub max_delay_ms: u64,
    /// Jitter percentage to prevent thundering herd (default: 0.1 = ±10%)
    pub jitter_pct: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ReconnectConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            max_consecutive_failures: std::env::var("REDIS_RECONNECT_MAX_FAILURES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            base_delay_ms: std::env::var("REDIS_RECONNECT_BASE_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            max_delay_ms: std::env::var("REDIS_RECONNECT_MAX_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60000),
            jitter_pct: std::env::var("REDIS_RECONNECT_JITTER_PCT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.1),
        }
    }

    /// Calculate exponential backoff delay with jitter
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let base_ms = self.base_delay_ms as f64;
        let exponential_ms = base_ms * 2f64.powi(attempt.saturating_sub(1) as i32);
        let capped_ms = exponential_ms.min(self.max_delay_ms as f64);

        // Add jitter: ±jitter_pct%
        let jitter_range = capped_ms * self.jitter_pct;
        let jitter = (rand::random::<f64>() * 2.0 - 1.0) * jitter_range;
        let final_ms = (capped_ms + jitter).max(0.0);

        Duration::from_millis(final_ms as u64)
    }
}

/// Statistics for monitoring reconnection behavior
#[derive(Debug, Default)]
pub struct ReconnectStats {
    /// Total reconnection attempts
    pub reconnect_attempts: AtomicU64,
    /// Successful reconnections
    pub successful_reconnects: AtomicU64,
    /// Failed reconnection attempts
    pub failed_reconnects: AtomicU64,
    /// Current consecutive failures
    pub consecutive_failures: AtomicU32,
}

impl ReconnectStats {
    pub fn record_attempt(&self) {
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.successful_reconnects.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failed_reconnects.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

/// Subscription type
#[derive(Clone, Debug)]
pub enum SubscriptionType {
    /// Subscribe to specific channels
    Channels(Vec<String>),
    /// Pattern subscribe
    Pattern(String),
}

/// Redis PubSub with automatic reconnection
#[derive(Clone)]
pub struct ReconnectingPubSub {
    client: Client,
    subscription: SubscriptionType,
    config: ReconnectConfig,
    stats: Arc<ReconnectStats>,
}

impl ReconnectingPubSub {
    /// Create new ReconnectingPubSub for channel subscription
    pub fn subscribe(client: Client, channels: Vec<String>) -> Self {
        Self {
            client,
            subscription: SubscriptionType::Channels(channels),
            config: ReconnectConfig::default(),
            stats: Arc::new(ReconnectStats::default()),
        }
    }

    /// Create new ReconnectingPubSub for pattern subscription
    pub fn psubscribe(client: Client, pattern: String) -> Self {
        Self {
            client,
            subscription: SubscriptionType::Pattern(pattern),
            config: ReconnectConfig::default(),
            stats: Arc::new(ReconnectStats::default()),
        }
    }

    /// Convert into a message stream that automatically reconnects
    pub fn into_message_stream(self) -> ReconnectingMessageStream {
        ReconnectingMessageStream::new(self)
    }

    /// Establish connection and subscribe
    async fn connect_and_subscribe(&self) -> Result<PubSub> {
        // Get async connection
        let conn = self
            .client
            .get_async_connection()
            .await
            .context("Failed to get async Redis connection")?;

        let mut pubsub = conn.into_pubsub();

        // Subscribe based on type
        match &self.subscription {
            SubscriptionType::Channels(channels) => {
                for channel in channels {
                    pubsub
                        .subscribe(channel)
                        .await
                        .with_context(|| format!("Failed to subscribe to channel: {}", channel))?;
                }
                info!("Subscribed to channels: {:?}", channels);
            }
            SubscriptionType::Pattern(pattern) => {
                pubsub
                    .psubscribe(pattern)
                    .await
                    .with_context(|| format!("Failed to psubscribe to pattern: {}", pattern))?;
                info!("Subscribed to pattern: {}", pattern);
            }
        }

        Ok(pubsub)
    }

    /// Get statistics
    pub fn stats(&self) -> &Arc<ReconnectStats> {
        &self.stats
    }
}

/// Stream that automatically reconnects on error
pub struct ReconnectingMessageStream {
    receiver: mpsc::UnboundedReceiver<Msg>,
}

impl ReconnectingMessageStream {
    fn new(reconnecting_pubsub: ReconnectingPubSub) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();

        // Spawn background task to handle PubSub and reconnection
        tokio::spawn(async move {
            reconnecting_task(reconnecting_pubsub, sender).await;
        });

        Self { receiver }
    }
}

/// Background task that handles reconnection logic
async fn reconnecting_task(reconnecting_pubsub: ReconnectingPubSub, sender: mpsc::UnboundedSender<Msg>) {
    loop {
        let stats = &reconnecting_pubsub.stats;
        let config = &reconnecting_pubsub.config;

        // Check circuit breaker
        let consecutive_failures = stats.get_consecutive_failures();
        if consecutive_failures >= config.max_consecutive_failures {
            error!(
                "Circuit breaker OPENED after {} consecutive failures. Pausing reconnection attempts.",
                consecutive_failures
            );
            // Wait longer before trying again
            tokio::time::sleep(Duration::from_secs(60)).await;
            stats.consecutive_failures.store(0, Ordering::Relaxed); // Reset after cooling off
            continue;
        }

        // Attempt connection
        stats.record_attempt();
        match reconnecting_pubsub.connect_and_subscribe().await {
            Ok(mut pubsub) => {
                stats.record_success();
                info!(
                    "Successfully connected (total reconnects: {})",
                    stats.successful_reconnects.load(Ordering::Relaxed)
                );

                // Read messages from PubSub and send to channel
                let mut stream = pubsub.on_message();
                while let Some(msg) = stream.next().await {
                    if sender.send(msg).is_err() {
                        // Receiver dropped, exit task
                        info!("Receiver dropped, stopping reconnecting task");
                        return;
                    }
                }

                // Stream ended (connection lost)
                warn!("Redis PubSub stream ended, will reconnect...");
                stats.record_failure();
            }
            Err(e) => {
                stats.record_failure();
                let attempt = consecutive_failures + 1;
                let delay = config.calculate_delay(attempt);
                error!(
                    "Failed to connect (attempt {}): {}. Retrying in {:?}...",
                    attempt, e, delay
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

impl Stream for ReconnectingMessageStream {
    type Item = Msg;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let config = ReconnectConfig {
            max_consecutive_failures: 10,
            base_delay_ms: 1000,
            max_delay_ms: 60000,
            jitter_pct: 0.0, // No jitter for predictable testing
        };

        // Test exponential progression
        assert_eq!(config.calculate_delay(1), Duration::from_millis(1000)); // 1 * 2^0 = 1s
        assert_eq!(config.calculate_delay(2), Duration::from_millis(2000)); // 1 * 2^1 = 2s
        assert_eq!(config.calculate_delay(3), Duration::from_millis(4000)); // 1 * 2^2 = 4s
        assert_eq!(config.calculate_delay(4), Duration::from_millis(8000)); // 1 * 2^3 = 8s
        assert_eq!(config.calculate_delay(5), Duration::from_millis(16000)); // 1 * 2^4 = 16s
        assert_eq!(config.calculate_delay(6), Duration::from_millis(32000)); // 1 * 2^5 = 32s
        assert_eq!(config.calculate_delay(7), Duration::from_millis(60000)); // Capped at max
        assert_eq!(config.calculate_delay(10), Duration::from_millis(60000)); // Still capped
    }

    #[test]
    fn test_stats() {
        let stats = ReconnectStats::default();

        assert_eq!(stats.reconnect_attempts.load(Ordering::Relaxed), 0);
        assert_eq!(stats.consecutive_failures.load(Ordering::Relaxed), 0);

        stats.record_attempt();
        assert_eq!(stats.reconnect_attempts.load(Ordering::Relaxed), 1);

        stats.record_failure();
        assert_eq!(stats.failed_reconnects.load(Ordering::Relaxed), 1);
        assert_eq!(stats.get_consecutive_failures(), 1);

        stats.record_failure();
        assert_eq!(stats.get_consecutive_failures(), 2);

        stats.record_success();
        assert_eq!(stats.successful_reconnects.load(Ordering::Relaxed), 1);
        assert_eq!(stats.get_consecutive_failures(), 0); // Reset on success
    }
}
