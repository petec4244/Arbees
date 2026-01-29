//! Critical alerting infrastructure for system-wide failures
//!
//! Provides multi-channel alerting (Signal SMS, Slack, webhook, file) for
//! catastrophic failures that require immediate operator attention.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Critical alert types for system-wide failures
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CriticalAlert {
    /// All game shards are unhealthy
    AllShardsUnhealthy {
        total_shards: usize,
        timestamp: DateTime<Utc>,
    },
    /// Redis connectivity issue
    RedisConnectivityIssue {
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// Database connectivity issue
    DatabaseConnectivityIssue {
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// No market discovery services available
    NoMarketDiscoveryServices {
        timestamp: DateTime<Utc>,
    },
}

impl CriticalAlert {
    /// Get alert type name for rate limiting
    pub fn alert_type(&self) -> &'static str {
        match self {
            CriticalAlert::AllShardsUnhealthy { .. } => "all_shards_unhealthy",
            CriticalAlert::RedisConnectivityIssue { .. } => "redis_connectivity",
            CriticalAlert::DatabaseConnectivityIssue { .. } => "database_connectivity",
            CriticalAlert::NoMarketDiscoveryServices { .. } => "no_market_discovery",
        }
    }

    /// Format alert message for human consumption
    pub fn format_message(&self) -> String {
        match self {
            CriticalAlert::AllShardsUnhealthy { total_shards, timestamp } => {
                format!(
                    "🚨 CRITICAL: All {} game shards are unhealthy at {}. No live game monitoring is happening!",
                    total_shards, timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                )
            }
            CriticalAlert::RedisConnectivityIssue { reason, timestamp } => {
                format!(
                    "🚨 CRITICAL: Redis connectivity issue at {}: {}. Services cannot communicate!",
                    timestamp.format("%Y-%m-%d %H:%M:%S UTC"), reason
                )
            }
            CriticalAlert::DatabaseConnectivityIssue { reason, timestamp } => {
                format!(
                    "🚨 CRITICAL: Database connectivity issue at {}: {}. Cannot persist trades or states!",
                    timestamp.format("%Y-%m-%d %H:%M:%S UTC"), reason
                )
            }
            CriticalAlert::NoMarketDiscoveryServices { timestamp } => {
                format!(
                    "🚨 CRITICAL: No market discovery services available at {}. Cannot find markets!",
                    timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                )
            }
        }
    }
}

/// Configuration for critical alert delivery
#[derive(Clone, Debug)]
pub struct CriticalAlertConfig {
    /// Enable critical alerting
    pub enabled: bool,
    /// Slack webhook URL
    pub slack_webhook_url: Option<String>,
    /// Custom webhook URL
    pub custom_webhook_url: Option<String>,
    /// File path for alert log
    pub alert_log_path: PathBuf,
    /// Rate limit: minimum seconds between alerts of same type
    pub rate_limit_secs: u64,
}

impl Default for CriticalAlertConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl CriticalAlertConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("CRITICAL_ALERTS_ENABLED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(true),
            slack_webhook_url: std::env::var("SLACK_WEBHOOK_URL").ok(),
            custom_webhook_url: std::env::var("CUSTOM_WEBHOOK_URL").ok(),
            alert_log_path: std::env::var("ALERT_LOG_PATH")
                .unwrap_or_else(|_| "/var/log/arbees/critical_alerts.log".to_string())
                .into(),
            rate_limit_secs: std::env::var("ALERT_RATE_LIMIT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300), // 5 minutes default
        }
    }
}

/// Client for sending critical alerts via multiple channels
pub struct CriticalAlertClient {
    config: CriticalAlertConfig,
    http_client: reqwest::Client,
    /// Track last alert time per alert type for rate limiting
    last_alert_times: Arc<RwLock<HashMap<String, Instant>>>,
}

impl CriticalAlertClient {
    /// Create new critical alert client from environment configuration
    pub fn from_env() -> Self {
        let config = CriticalAlertConfig::from_env();
        Self::new(config)
    }

    /// Create new critical alert client with custom configuration
    pub fn new(config: CriticalAlertConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            http_client,
            last_alert_times: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Send a critical alert via all configured channels
    pub async fn send_critical_alert(&self, alert: CriticalAlert) {
        if !self.config.enabled {
            info!("Critical alerts disabled, skipping: {}", alert.format_message());
            return;
        }

        // Check rate limiting
        if !self.should_send_alert(&alert).await {
            info!(
                "Rate limit: Skipping alert of type '{}' (sent recently)",
                alert.alert_type()
            );
            return;
        }

        let message = alert.format_message();
        info!("Sending critical alert: {}", message);

        // Try Slack webhook
        if let Some(webhook_url) = &self.config.slack_webhook_url {
            if let Err(e) = self.send_slack_alert(webhook_url, &message).await {
                error!("Failed to send Slack alert: {}", e);
            }
        }

        // Try custom webhook
        if let Some(webhook_url) = &self.config.custom_webhook_url {
            if let Err(e) = self.send_webhook_alert(webhook_url, &alert).await {
                error!("Failed to send custom webhook alert: {}", e);
            }
        }

        // Always log to file (fallback)
        if let Err(e) = self.log_to_file(&alert).await {
            error!("Failed to log alert to file: {}", e);
        }

        // Update last alert time
        self.record_alert_sent(&alert).await;
    }

    /// Check if we should send this alert based on rate limiting
    async fn should_send_alert(&self, alert: &CriticalAlert) -> bool {
        let alert_type = alert.alert_type();
        let times = self.last_alert_times.read().await;

        match times.get(alert_type) {
            Some(last_time) => {
                let elapsed = last_time.elapsed();
                elapsed.as_secs() >= self.config.rate_limit_secs
            }
            None => true, // Never sent before
        }
    }

    /// Record that we sent this alert
    async fn record_alert_sent(&self, alert: &CriticalAlert) {
        let alert_type = alert.alert_type().to_string();
        let mut times = self.last_alert_times.write().await;
        times.insert(alert_type, Instant::now());
    }

    /// Send alert to Slack webhook
    async fn send_slack_alert(&self, webhook_url: &str, message: &str) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "text": message,
            "username": "Arbees Critical Alerts",
            "icon_emoji": ":rotating_light:"
        });

        let response = self
            .http_client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Slack webhook returned {}: {}", status, body);
        }

        info!("Successfully sent Slack alert");
        Ok(())
    }

    /// Send alert to custom webhook
    async fn send_webhook_alert(&self, webhook_url: &str, alert: &CriticalAlert) -> anyhow::Result<()> {
        let response = self
            .http_client
            .post(webhook_url)
            .json(&alert)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Custom webhook returned {}: {}", status, body);
        }

        info!("Successfully sent custom webhook alert");
        Ok(())
    }

    /// Log alert to file (always succeeds, best effort)
    async fn log_to_file(&self, alert: &CriticalAlert) -> anyhow::Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = self.config.alert_log_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.alert_log_path)
            .await?;

        let log_entry = format!(
            "{} | {} | {}\n",
            Utc::now().to_rfc3339(),
            alert.alert_type(),
            serde_json::to_string(alert).unwrap_or_else(|_| "failed to serialize".to_string())
        );

        file.write_all(log_entry.as_bytes()).await?;
        file.sync_all().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_formatting() {
        let alert = CriticalAlert::AllShardsUnhealthy {
            total_shards: 3,
            timestamp: Utc::now(),
        };
        let message = alert.format_message();
        assert!(message.contains("CRITICAL"));
        assert!(message.contains("3"));
        assert!(message.contains("shards"));
    }

    #[test]
    fn test_alert_types() {
        let alert1 = CriticalAlert::AllShardsUnhealthy {
            total_shards: 3,
            timestamp: Utc::now(),
        };
        let alert2 = CriticalAlert::RedisConnectivityIssue {
            reason: "test".to_string(),
            timestamp: Utc::now(),
        };

        assert_eq!(alert1.alert_type(), "all_shards_unhealthy");
        assert_eq!(alert2.alert_type(), "redis_connectivity");
    }
}
