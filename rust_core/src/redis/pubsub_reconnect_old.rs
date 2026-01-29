//! Redis PubSub with automatic reconnection and exponential backoff
//!
//! This module provides a reconnecting PubSub wrapper that automatically
//! handles connection failures and resubscribes to channels/patterns.

use anyhow::{Context, Result};
use futures_util::stream::Stream;
use redis::{aio::PubSub, Client, Msg};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{error, info, warn};

/// Configuration for reconnection behavior
#[derive(Clone, Debug)]
pub struct ReconnectConfig {
    /// Maximum consecutive failures before circuit breaker opens
    pub max_consecutive_failures: u32,
    /// Initial backoff delay in milliseconds
    pub base_delay_ms: u64,
    /// Maximum backoff delay in milliseconds
    pub max_delay_ms: u64,
    /// Jitter percentage (0.0-1.0) to prevent thundering herd
    pub jitter_pct: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 10,
            base_delay_ms: 1000,       // 1 second
            max_delay_ms: 60000,       // 60 seconds
            jitter_pct: 0.1,           // ±10%
        }
    }
}

impl ReconnectConfig {
    /// Create config from environment variables with defaults
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

    /// Calculate backoff delay for given attempt with jitter
    fn calculate_delay(&self, attempt: u32) -> Duration {
        // Exponential backoff: base_delay * 2^(attempt - 1)
        let base_delay = self.base_delay_ms.saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1)));
        let delay_ms = base_delay.min(self.max_delay_ms);

        // Add jitter: ±jitter_pct of the delay
        let jitter_range = (delay_ms as f64 * self.jitter_pct) as u64;
        let jitter = if jitter_range > 0 {
            (rand::random::<u64>() % (jitter_range * 2)).saturating_sub(jitter_range)
        } else {
            0
        };

        Duration::from_millis(delay_ms.saturating_add(jitter))
    }
}

/// Statistics for reconnecting PubSub
#[derive(Debug, Default)]
pub struct ReconnectStats {
    /// Total number of reconnection attempts
    pub reconnect_attempts: AtomicU64,
    /// Total number of successful reconnections
    pub successful_reconnects: AtomicU64,
    /// Current consecutive failures
    pub consecutive_failures: AtomicU32,
    /// Last reconnect timestamp (unix millis)
    pub last_reconnect_at: AtomicU64,
}

impl ReconnectStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_attempt(&self) {
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.successful_reconnects.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.last_reconnect_at.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    pub fn get_consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

/// Subscription type for reconnecting PubSub
#[derive(Clone, Debug)]
pub enum SubscriptionType {
    /// Subscribe to specific channels
    Channels(Vec<String>),
    /// Subscribe to pattern (psubscribe)
    Pattern(String),
}

/// Reconnecting PubSub wrapper
pub struct ReconnectingPubSub {
    client: Client,
    subscription: SubscriptionType,
    config: ReconnectConfig,
    stats: Arc<ReconnectStats>,
}

impl ReconnectingPubSub {
    /// Create a new reconnecting PubSub for specific channels
    pub fn subscribe(client: Client, channels: Vec<String>) -> Self {
        Self {
            client,
            subscription: SubscriptionType::Channels(channels),
            config: ReconnectConfig::from_env(),
            stats: Arc::new(ReconnectStats::new()),
        }
    }

    /// Create a new reconnecting PubSub for a pattern
    pub fn psubscribe(client: Client, pattern: String) -> Self {
        Self {
            client,
            subscription: SubscriptionType::Pattern(pattern),
            config: ReconnectConfig::from_env(),
            stats: Arc::new(ReconnectStats::new()),
        }
    }

    /// Create with custom config
    pub fn with_config(mut self, config: ReconnectConfig) -> Self {
        self.config = config;
        self
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

    async fn attempt_reconnect(inner: &mut ReconnectingMessageStreamInner) -> bool {
        let stats = &inner.reconnecting_pubsub.stats;
        let config = &inner.reconnecting_pubsub.config;

        // Check circuit breaker
        let consecutive_failures = stats.get_consecutive_failures();
        if consecutive_failures >= config.max_consecutive_failures {
            if inner.reconnect_state != ReconnectState::CircuitOpen {
                error!(
                    "Circuit breaker OPENED after {} consecutive failures",
                    consecutive_failures
                );
                inner.reconnect_state = ReconnectState::CircuitOpen;
            }
            return false;
        }

        // Check if we need to wait before reconnecting
        if let Some(next_reconnect) = inner.next_reconnect_at {
            if Instant::now() < next_reconnect {
                return false;
            }
        }

        // Attempt reconnection
        inner.reconnect_state = ReconnectState::Reconnecting;
        stats.record_attempt();

        let attempt = stats.get_consecutive_failures();
        let delay = config.calculate_delay(attempt);

        warn!(
            "Attempting to reconnect (attempt {}/{}, will retry in {:?} on failure)...",
            attempt, config.max_consecutive_failures, delay
        );

        match inner.reconnecting_pubsub.connect_and_subscribe().await {
            Ok(pubsub) => {
                inner.current_pubsub = Some(pubsub);
                inner.reconnect_state = ReconnectState::Connected;
                inner.next_reconnect_at = None;
                stats.record_success();

                info!(
                    "Successfully reconnected (total reconnects: {})",
                    stats.successful_reconnects.load(Ordering::Relaxed)
                );
                true
            }
            Err(e) => {
                error!("Reconnection failed: {}", e);
                inner.next_reconnect_at = Some(Instant::now() + delay);
                false
            }
        }
    }
}

impl Stream for ReconnectingMessageStream {
    type Item = Msg;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let inner = self.inner.clone();

        // We need to use a blocking_mutex approach or spawn a task
        // For simplicity, we'll use tokio::task::block_in_place for now
        // In production, consider using a more sophisticated async approach

        let mut inner = match inner.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // Lock is held, wake and try again
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        };

        // If we don't have a stream, try to connect
        if inner.current_stream.is_none() {
            // Spawn reconnection attempt
            let inner_clone = self.inner.clone();
            let waker = cx.waker().clone();
            tokio::spawn(async move {
                let mut inner = inner_clone.lock().await;
                Self::attempt_reconnect(&mut inner).await;
                waker.wake();
            });
            return Poll::Pending;
        }

        // Poll the current stream
        if let Some(stream) = inner.current_stream.as_mut() {
            match stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(msg)) => Poll::Ready(Some(msg)),
                Poll::Ready(None) => {
                    // Stream ended (connection lost)
                    warn!("Redis PubSub stream ended, will reconnect...");
                    inner.current_stream = None;

                    // Schedule reconnection
                    let inner_clone = self.inner.clone();
                    let waker = cx.waker().clone();
                    tokio::spawn(async move {
                        let mut inner = inner_clone.lock().await;
                        Self::attempt_reconnect(&mut inner).await;
                        waker.wake();
                    });

                    Poll::Pending
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Pending
        }
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
        let stats = ReconnectStats::new();

        assert_eq!(stats.reconnect_attempts.load(Ordering::Relaxed), 0);
        assert_eq!(stats.successful_reconnects.load(Ordering::Relaxed), 0);
        assert_eq!(stats.get_consecutive_failures(), 0);

        stats.record_attempt();
        assert_eq!(stats.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(stats.get_consecutive_failures(), 1);

        stats.record_attempt();
        assert_eq!(stats.reconnect_attempts.load(Ordering::Relaxed), 2);
        assert_eq!(stats.get_consecutive_failures(), 2);

        stats.record_success();
        assert_eq!(stats.successful_reconnects.load(Ordering::Relaxed), 1);
        assert_eq!(stats.get_consecutive_failures(), 0);
    }
}
