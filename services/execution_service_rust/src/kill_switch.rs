//! Kill switch for emergency trading halt
//!
//! Provides both Redis pub/sub and file-based kill switch mechanisms.
//! When activated, all order execution is halted until the kill switch is disabled.

use anyhow::Result;
use arbees_rust_core::redis::bus::RedisBus;
use futures_util::StreamExt;
use log::{error, info, warn};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Redis channel for kill switch commands
pub const KILL_SWITCH_CHANNEL: &str = "trading:kill_switch";
/// Redis key for kill switch status
pub const KILL_SWITCH_STATUS_KEY: &str = "trading:kill_switch_status";
/// File-based kill switch path (fallback mechanism)
pub const KILL_SWITCH_FILE: &str = "/tmp/arbees_kill_switch";

/// Reason for kill switch activation
#[derive(Debug, Clone)]
pub enum KillSwitchReason {
    /// Manual activation via Redis command
    Manual,
    /// Daily loss limit exceeded
    DailyLossExceeded,
    /// File-based kill switch detected
    FileTriggered,
    /// External system trigger
    External(String),
}

impl std::fmt::Display for KillSwitchReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KillSwitchReason::Manual => write!(f, "manual"),
            KillSwitchReason::DailyLossExceeded => write!(f, "daily_loss_exceeded"),
            KillSwitchReason::FileTriggered => write!(f, "file_triggered"),
            KillSwitchReason::External(reason) => write!(f, "external: {}", reason),
        }
    }
}

/// Kill switch controller
pub struct KillSwitch {
    /// Whether trading is halted
    enabled: Arc<AtomicBool>,
    /// Channel to send kill switch events to listeners
    event_tx: Option<mpsc::Sender<bool>>,
}

impl KillSwitch {
    /// Create a new kill switch (disabled by default)
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            event_tx: None,
        }
    }

    /// Create a new kill switch with event channel
    pub fn with_events() -> (Self, mpsc::Receiver<bool>) {
        let (tx, rx) = mpsc::channel(16);
        (
            Self {
                enabled: Arc::new(AtomicBool::new(false)),
                event_tx: Some(tx),
            },
            rx,
        )
    }

    /// Check if kill switch is enabled (trading halted)
    pub fn is_enabled(&self) -> bool {
        // Check atomic state first
        if self.enabled.load(Ordering::SeqCst) {
            return true;
        }

        // Also check file-based fallback (allows emergency halt without Redis)
        if Path::new(KILL_SWITCH_FILE).exists() {
            warn!("Kill switch file detected at {}", KILL_SWITCH_FILE);
            self.enabled.store(true, Ordering::SeqCst);
            return true;
        }

        false
    }

    /// Enable the kill switch (halt trading)
    pub fn enable(&self, reason: KillSwitchReason) {
        let was_enabled = self.enabled.swap(true, Ordering::SeqCst);
        if !was_enabled {
            error!("KILL SWITCH ENABLED: {}", reason);
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(true);
            }
        }
    }

    /// Disable the kill switch (resume trading)
    pub fn disable(&self) {
        let was_enabled = self.enabled.swap(false, Ordering::SeqCst);
        if was_enabled {
            warn!("Kill switch DISABLED - trading resumed");
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(false);
            }
        }
    }

    /// Get the atomic boolean for sharing with other tasks
    pub fn get_enabled_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.enabled)
    }

    /// Start listening for Redis kill switch commands
    ///
    /// Returns a task handle that should be spawned.
    pub async fn start_redis_listener(
        &self,
        redis: Arc<RedisBus>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        let enabled = Arc::clone(&self.enabled);
        let redis_for_status = redis.clone();

        // Publish initial status
        let _ = redis_for_status
            .publish(KILL_SWITCH_STATUS_KEY, &"disabled".to_string())
            .await;

        let handle = tokio::spawn(async move {
            loop {
                match redis.subscribe(KILL_SWITCH_CHANNEL).await {
                    Ok(mut pubsub) => {
                        info!("Kill switch listening on {}", KILL_SWITCH_CHANNEL);

                        let mut stream = pubsub.on_message();
                        while let Some(msg) = stream.next().await {
                            let payload: String = match msg.get_payload() {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!("Failed to read kill switch message: {}", e);
                                    continue;
                                }
                            };

                            match payload.to_uppercase().as_str() {
                                "ENABLE" | "ON" | "HALT" | "STOP" => {
                                    let was_enabled = enabled.swap(true, Ordering::SeqCst);
                                    if !was_enabled {
                                        error!("KILL SWITCH ENABLED via Redis command");
                                        // Publish status change
                                        let _ = redis_for_status
                                            .publish(KILL_SWITCH_STATUS_KEY, &"enabled".to_string())
                                            .await;
                                    }
                                }
                                "DISABLE" | "OFF" | "RESUME" | "START" => {
                                    let was_enabled = enabled.swap(false, Ordering::SeqCst);
                                    if was_enabled {
                                        warn!("Kill switch DISABLED via Redis command - trading resumed");
                                        // Publish status change
                                        let _ = redis_for_status
                                            .publish(KILL_SWITCH_STATUS_KEY, &"disabled".to_string())
                                            .await;
                                    }
                                }
                                _ => {
                                    warn!("Unknown kill switch command: {}", payload);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to subscribe to kill switch channel: {}", e);
                    }
                }

                // Reconnect delay
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        Ok(handle)
    }
}

impl Default for KillSwitch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kill_switch_initially_disabled() {
        let ks = KillSwitch::new();
        assert!(!ks.enabled.load(Ordering::SeqCst));
    }

    #[test]
    fn test_enable_disable() {
        let ks = KillSwitch::new();

        ks.enable(KillSwitchReason::Manual);
        assert!(ks.enabled.load(Ordering::SeqCst));

        ks.disable();
        assert!(!ks.enabled.load(Ordering::SeqCst));
    }

    #[test]
    fn test_reason_display() {
        assert_eq!(KillSwitchReason::Manual.to_string(), "manual");
        assert_eq!(
            KillSwitchReason::DailyLossExceeded.to_string(),
            "daily_loss_exceeded"
        );
        assert_eq!(
            KillSwitchReason::External("test".to_string()).to_string(),
            "external: test"
        );
    }
}
