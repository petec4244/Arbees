//! Database retry logic for transient failures
//!
//! Provides automatic retry with exponential backoff for database operations.

use anyhow::Result;
use std::future::Future;
use std::time::Duration;
use tracing::warn;

/// Execute a database operation with automatic retry on transient failures
///
/// # Example
/// ```ignore
/// use arbees_rust_core::db::retry::execute_with_retry;
///
/// let result = execute_with_retry(
///     || async {
///         sqlx::query("INSERT INTO table VALUES ($1)")
///             .bind(value)
///             .execute(&pool)
///             .await
///     },
///     3 // max attempts
/// ).await?;
/// ```
pub async fn execute_with_retry<F, Fut, T>(mut f: F, max_attempts: u32) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts && is_retriable_error(&e) => {
                let backoff_ms = 100_u64 * 2_u64.pow(attempt - 1);
                warn!(
                    "Database operation failed (attempt {}/{}): {}. Retrying in {}ms",
                    attempt, max_attempts, e, backoff_ms
                );
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Check if a database error is retriable
fn is_retriable_error(e: &anyhow::Error) -> bool {
    let err_str = e.to_string().to_lowercase();

    // Connection-related errors that are likely transient
    err_str.contains("connection")
        || err_str.contains("timeout")
        || err_str.contains("broken pipe")
        || err_str.contains("connection reset")
        || err_str.contains("connection refused")
        || err_str.contains("connection closed")
        || err_str.contains("connection lost")
        // PostgreSQL specific transient errors
        || err_str.contains("could not serialize")
        || err_str.contains("deadlock detected")
        || err_str.contains("too many clients")
        || err_str.contains("server closed the connection")
        || err_str.contains("ssl error")
        || err_str.contains("network error")
}

/// Execute with retry and custom backoff configuration
pub async fn execute_with_retry_custom<F, Fut, T>(
    mut f: F,
    max_attempts: u32,
    base_backoff_ms: u64,
    max_backoff_ms: u64,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts && is_retriable_error(&e) => {
                let backoff_ms = (base_backoff_ms * 2_u64.pow(attempt - 1)).min(max_backoff_ms);
                warn!(
                    "Database operation failed (attempt {}/{}): {}. Retrying in {}ms",
                    attempt, max_attempts, e, backoff_ms
                );
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_is_retriable_error() {
        // Connection errors (retriable)
        assert!(is_retriable_error(&anyhow::anyhow!("connection timeout")));
        assert!(is_retriable_error(&anyhow::anyhow!("connection refused")));
        assert!(is_retriable_error(&anyhow::anyhow!("broken pipe")));
        assert!(is_retriable_error(&anyhow::anyhow!("connection reset by peer")));

        // PostgreSQL transient errors (retriable)
        assert!(is_retriable_error(&anyhow::anyhow!("could not serialize access")));
        assert!(is_retriable_error(&anyhow::anyhow!("deadlock detected")));
        assert!(is_retriable_error(&anyhow::anyhow!("too many clients")));

        // Application errors (not retriable)
        assert!(!is_retriable_error(&anyhow::anyhow!("unique constraint violation")));
        assert!(!is_retriable_error(&anyhow::anyhow!("invalid input syntax")));
        assert!(!is_retriable_error(&anyhow::anyhow!("column does not exist")));
    }

    #[tokio::test]
    async fn test_retry_succeeds_eventually() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let result: anyhow::Result<i32> = execute_with_retry(
            || {
                let count = attempt_count_clone.clone();
                async move {
                    let current = count.fetch_add(1, Ordering::SeqCst) + 1;
                    if current < 3 {
                        Err(anyhow::anyhow!("connection timeout"))
                    } else {
                        Ok(42)
                    }
                }
            },
            3,
        )
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_fails_after_max_attempts() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let result: anyhow::Result<i32> = execute_with_retry(
            || {
                let count = attempt_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("connection timeout"))
                }
            },
            3,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_no_retry_on_non_retriable_error() {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let result: anyhow::Result<i32> = execute_with_retry(
            || {
                let count = attempt_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("unique constraint violation"))
                }
            },
            3,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1); // Should not retry
    }
}
