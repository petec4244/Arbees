use anyhow::{Context, Result};
use redis::{aio::ConnectionManager, Client, AsyncCommands};
use serde::Serialize;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, warn, info};

/// Statistics for monitoring Redis connection health
#[derive(Debug, Default)]
pub struct RedisBusStats {
    /// Total messages published successfully
    pub messages_published: AtomicU64,
    /// Total publish failures
    pub publish_failures: AtomicU64,
    /// Total reconnection attempts
    pub reconnect_attempts: AtomicU64,
}

impl RedisBusStats {
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.messages_published.load(Ordering::Relaxed),
            self.publish_failures.load(Ordering::Relaxed),
            self.reconnect_attempts.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone)]
pub struct RedisBus {
    client: Client,
    /// ConnectionManager provides automatic reconnection and connection pooling
    connection: ConnectionManager,
    /// Statistics for monitoring
    stats: Arc<RedisBusStats>,
}

impl RedisBus {
    /// Create a new RedisBus with ConnectionManager for automatic reconnection.
    ///
    /// ConnectionManager handles:
    /// - Automatic reconnection on connection loss
    /// - Connection pooling for concurrent access
    /// - No mutex contention (lock-free)
    pub async fn new() -> Result<Self> {
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
        let client = Client::open(redis_url.clone())?;

        // ConnectionManager automatically handles reconnection
        let connection = ConnectionManager::new(client.clone()).await
            .context("Failed to create Redis ConnectionManager")?;

        info!("Redis ConnectionManager initialized with auto-reconnect");

        Ok(Self {
            client,
            connection,
            stats: Arc::new(RedisBusStats::default()),
        })
    }

    /// Publish a serializable message to a channel.
    ///
    /// Uses ConnectionManager which automatically handles reconnection if the
    /// connection is lost. No mutex contention.
    pub async fn publish<T: Serialize>(&self, channel: &str, message: &T) -> Result<()> {
        let payload = serde_json::to_string(message)?;
        self.publish_str(channel, &payload).await
    }

    /// Publish a string message to a channel.
    ///
    /// Includes automatic retry on transient failures.
    pub async fn publish_str(&self, channel: &str, message: &str) -> Result<()> {
        let mut conn = self.connection.clone();

        // Try up to 3 times with exponential backoff
        let mut last_error = None;
        for attempt in 0..3 {
            match conn.publish::<_, _, ()>(channel, message).await {
                Ok(_) => {
                    self.stats.messages_published.fetch_add(1, Ordering::Relaxed);
                    if attempt > 0 {
                        debug!("Publish succeeded on attempt {} for channel {}", attempt + 1, channel);
                    }
                    return Ok(());
                }
                Err(e) => {
                    self.stats.publish_failures.fetch_add(1, Ordering::Relaxed);
                    last_error = Some(e);

                    if attempt < 2 {
                        let delay = std::time::Duration::from_millis(50 * (1 << attempt));
                        warn!(
                            "Redis publish failed (attempt {}), retrying in {:?}: {}",
                            attempt + 1, delay, last_error.as_ref().unwrap()
                        );
                        self.stats.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow::anyhow!(
            "Failed to publish to {} after 3 attempts: {}",
            channel,
            last_error.unwrap()
        ))
    }

    /// Subscribe to a channel. Returns a PubSub connection for receiving messages.
    ///
    /// Note: PubSub connections are separate from the ConnectionManager and
    /// need their own reconnection handling in the subscriber loop.
    pub async fn subscribe(&self, channel: &str) -> Result<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await
            .context("Failed to get connection for subscribe")?;
        let mut pubsub = conn.into_pubsub();
        pubsub.subscribe(channel).await
            .with_context(|| format!("Failed to subscribe to channel: {}", channel))?;
        Ok(pubsub)
    }

    /// Pattern subscribe. Returns a PubSub connection for receiving messages.
    pub async fn psubscribe(&self, pattern: &str) -> Result<redis::aio::PubSub> {
        let conn = self.client.get_async_connection().await
            .context("Failed to get connection for psubscribe")?;
        let mut pubsub = conn.into_pubsub();
        pubsub.psubscribe(pattern).await
            .with_context(|| format!("Failed to psubscribe to pattern: {}", pattern))?;
        Ok(pubsub)
    }

    /// Get the underlying client for advanced operations
    pub fn get_client(&self) -> Client {
        self.client.clone()
    }

    /// Get a dedicated connection for complex operations.
    ///
    /// Prefer using the ConnectionManager (via publish methods) when possible.
    pub async fn get_connection(&self) -> Result<redis::aio::Connection> {
        self.client.get_async_connection().await
            .context("Failed to get dedicated connection")
    }

    /// Get connection statistics for monitoring
    pub fn get_stats(&self) -> &RedisBusStats {
        &self.stats
    }

    /// Check if Redis is healthy by sending a PING
    pub async fn health_check(&self) -> bool {
        let mut conn = self.connection.clone();
        match redis::cmd("PING").query_async::<_, String>(&mut conn).await {
            Ok(response) => response == "PONG",
            Err(_) => false,
        }
    }
}
