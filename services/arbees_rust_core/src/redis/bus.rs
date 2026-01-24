use anyhow::{Context, Result};
use redis::{aio::Connection, Client, AsyncCommands};
use serde::Serialize;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RedisBus {
    client: Client,
    pubsub_client: Client, // Separate client for pubsub if needed, or just reuse if library allows
    connection: Arc<Mutex<Connection>>,
}

impl RedisBus {
    pub async fn new() -> Result<Self> {
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
        let client = Client::open(redis_url.clone())?;
        
        // We need a separate client/connection for pubsub in some patterns, 
        // but for simple publish we can share.
        // For subscribing, we usually hand off the connection to a task.
        
        let connection = client.get_async_connection().await?;

        Ok(Self {
            client: client.clone(),
            pubsub_client: client,
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub async fn publish<T: Serialize>(&self, channel: &str, message: &T) -> Result<()> {
        let payload = serde_json::to_string(message)?;
        let mut conn = self.connection.lock().await;
        conn.publish::<_, _, ()>(channel, payload)
            .await
            .context("Failed to publish message")?;
        Ok(())
    }

    pub async fn publish_str(&self, channel: &str, message: &str) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.publish::<_, _, ()>(channel, message)
            .await
            .context("Failed to publish string message")?;
        Ok(())
    }

    pub async fn subscribe(&self, channel: &str) -> Result<redis::aio::PubSub> {
        let conn = self.pubsub_client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub.subscribe(channel).await?;
        Ok(pubsub)
    }

    pub async fn psubscribe(&self, pattern: &str) -> Result<redis::aio::PubSub> {
        let conn = self.pubsub_client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        pubsub.psubscribe(pattern).await?;
        Ok(pubsub)
    }

    pub fn get_client(&self) -> Client {
        self.client.clone()
    }
    
    // Helper to get a dedicated connection for complex operations
    pub async fn get_connection(&self) -> Result<Connection> {
        Ok(self.client.get_async_connection().await?)
    }
}
