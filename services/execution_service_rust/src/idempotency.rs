//! Idempotency tracking for execution requests
//!
//! Prevents duplicate order execution by tracking idempotency keys.
//! Uses in-memory storage with automatic cleanup of expired entries.

use chrono::{DateTime, Duration, Utc};
use log::{debug, warn};
use std::collections::HashMap;
use std::sync::Mutex;

/// Maximum number of idempotency entries to keep
const MAX_ENTRIES: usize = 10_000;
/// Time-to-live for idempotency entries (5 minutes)
const ENTRY_TTL_SECS: i64 = 300;

/// Result of checking an idempotency key
#[derive(Debug, Clone)]
pub enum IdempotencyResult {
    /// Key is new, execution can proceed
    New,
    /// Key is duplicate, execution should be skipped
    Duplicate {
        original_timestamp: DateTime<Utc>,
    },
}

/// In-memory idempotency tracker
pub struct IdempotencyTracker {
    /// Map of idempotency_key -> (request_id, timestamp)
    entries: Mutex<HashMap<String, (String, DateTime<Utc>)>>,
}

impl IdempotencyTracker {
    /// Create a new idempotency tracker
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::with_capacity(1024)),
        }
    }

    /// Check if a key has been seen and record it if new
    ///
    /// Returns `IdempotencyResult::New` if this is a new key.
    /// Returns `IdempotencyResult::Duplicate` if the key was already seen.
    pub fn check_and_record(&self, idempotency_key: &str, request_id: &str) -> IdempotencyResult {
        let now = Utc::now();
        let mut entries = self.entries.lock().unwrap();

        // Check if key exists and is not expired
        if let Some((existing_request_id, timestamp)) = entries.get(idempotency_key) {
            if now - *timestamp < Duration::seconds(ENTRY_TTL_SECS) {
                debug!(
                    "Duplicate idempotency key: {} (original request: {}, age: {}s)",
                    idempotency_key,
                    existing_request_id,
                    (now - *timestamp).num_seconds()
                );
                return IdempotencyResult::Duplicate {
                    original_timestamp: *timestamp,
                };
            }
            // Entry expired, will be overwritten
        }

        // Clean up if we're at capacity
        if entries.len() >= MAX_ENTRIES {
            self.cleanup_expired_locked(&mut entries, now);

            // If still at capacity after cleanup, remove oldest entries
            if entries.len() >= MAX_ENTRIES {
                self.evict_oldest_locked(&mut entries);
            }
        }

        // Record this key
        entries.insert(
            idempotency_key.to_string(),
            (request_id.to_string(), now),
        );

        IdempotencyResult::New
    }

    /// Check if a key exists without recording (for testing/monitoring)
    pub fn contains(&self, idempotency_key: &str) -> bool {
        let now = Utc::now();
        let entries = self.entries.lock().unwrap();

        if let Some((_, timestamp)) = entries.get(idempotency_key) {
            now - *timestamp < Duration::seconds(ENTRY_TTL_SECS)
        } else {
            false
        }
    }

    /// Get the current number of tracked entries
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Check if the tracker is empty
    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap().is_empty()
    }

    /// Run cleanup of expired entries (can be called periodically)
    pub fn cleanup_expired(&self) {
        let now = Utc::now();
        let mut entries = self.entries.lock().unwrap();
        self.cleanup_expired_locked(&mut entries, now);
    }

    /// Cleanup expired entries (internal, with lock held)
    fn cleanup_expired_locked(
        &self,
        entries: &mut HashMap<String, (String, DateTime<Utc>)>,
        now: DateTime<Utc>,
    ) {
        let cutoff = now - Duration::seconds(ENTRY_TTL_SECS);
        let initial_len = entries.len();

        entries.retain(|_, (_, timestamp)| *timestamp >= cutoff);

        let removed = initial_len - entries.len();
        if removed > 0 {
            debug!("Idempotency cleanup: removed {} expired entries", removed);
        }
    }

    /// Evict oldest entries when at capacity (internal, with lock held)
    fn evict_oldest_locked(&self, entries: &mut HashMap<String, (String, DateTime<Utc>)>) {
        // Sort by timestamp and remove oldest 10%
        let mut entries_vec: Vec<_> = entries.iter()
            .map(|(k, (_, ts))| (k.clone(), *ts))
            .collect();

        entries_vec.sort_by_key(|(_, ts)| *ts);

        let to_remove = entries_vec.len() / 10;
        for (key, _) in entries_vec.iter().take(to_remove) {
            entries.remove(key);
        }

        warn!(
            "Idempotency tracker at capacity, evicted {} oldest entries",
            to_remove
        );
    }

    /// Clear all entries (for testing)
    #[cfg(test)]
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

impl Default for IdempotencyTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Start a background task to periodically clean up expired entries
pub fn start_cleanup_task(tracker: std::sync::Arc<IdempotencyTracker>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            tracker.cleanup_expired();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_key_allowed() {
        let tracker = IdempotencyTracker::new();

        let result = tracker.check_and_record("key-1", "req-1");
        assert!(matches!(result, IdempotencyResult::New));
    }

    #[test]
    fn test_duplicate_key_rejected() {
        let tracker = IdempotencyTracker::new();

        // First request succeeds
        let result1 = tracker.check_and_record("key-1", "req-1");
        assert!(matches!(result1, IdempotencyResult::New));

        // Second request with same key is duplicate
        let result2 = tracker.check_and_record("key-1", "req-2");
        assert!(matches!(result2, IdempotencyResult::Duplicate { .. }));
    }

    #[test]
    fn test_different_keys_allowed() {
        let tracker = IdempotencyTracker::new();

        let result1 = tracker.check_and_record("key-1", "req-1");
        let result2 = tracker.check_and_record("key-2", "req-2");

        assert!(matches!(result1, IdempotencyResult::New));
        assert!(matches!(result2, IdempotencyResult::New));
    }

    #[test]
    fn test_contains() {
        let tracker = IdempotencyTracker::new();

        assert!(!tracker.contains("key-1"));

        tracker.check_and_record("key-1", "req-1");

        assert!(tracker.contains("key-1"));
        assert!(!tracker.contains("key-2"));
    }

    #[test]
    fn test_len() {
        let tracker = IdempotencyTracker::new();

        assert_eq!(tracker.len(), 0);
        assert!(tracker.is_empty());

        tracker.check_and_record("key-1", "req-1");
        tracker.check_and_record("key-2", "req-2");

        assert_eq!(tracker.len(), 2);
        assert!(!tracker.is_empty());
    }
}
