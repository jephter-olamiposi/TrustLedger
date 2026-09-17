//! Idempotency guard for mutating requests.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::IdempotencyError;

/// Status of an operation associated with an idempotency key.
#[derive(Clone, Debug, PartialEq, Eq)]
enum IdempotencyStatus {
    /// Operation is currently being processed by another task.
    InProgress,
    /// Operation succeeded; holds serialized response JSON.
    Completed(String),
}

/// In-memory guard that replays a prior success and rejects concurrent writes.
#[derive(Debug, Default)]
pub struct IdempotencyStore {
    entries: RwLock<HashMap<String, IdempotencyStatus>>,
}

impl IdempotencyStore {
    /// Create a new, empty idempotency store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Try to start processing an operation with the given idempotency key.
    ///
    /// - If the key is new: returns `Ok(None)` and marks the key as `InProgress`.
    /// - If the key completed previously: returns `Ok(Some(cached_json))` with the stored result.
    /// - If the key is currently in-flight: returns [`IdempotencyError::ConcurrentRequest`].
    ///
    /// # Errors
    ///
    /// Returns [`IdempotencyError::ConcurrentRequest`] if a request with this key is currently active.
    pub fn try_acquire(&self, key: &str) -> Result<Option<String>, IdempotencyError> {
        let mut map = self
            .entries
            .write()
            .map_err(|_| IdempotencyError::ConcurrentRequest(key.to_string()))?;

        if let Some(status) = map.get(key) {
            match status {
                IdempotencyStatus::InProgress => {
                    Err(IdempotencyError::ConcurrentRequest(key.to_string()))
                }
                IdempotencyStatus::Completed(json) => Ok(Some(json.clone())),
            }
        } else {
            map.insert(key.to_string(), IdempotencyStatus::InProgress);
            Ok(None)
        }
    }

    /// Record that the operation for `key` completed successfully with the given response.
    pub fn complete(&self, key: &str, response_json: String) {
        if let Ok(mut map) = self.entries.write() {
            map.insert(key.to_string(), IdempotencyStatus::Completed(response_json));
        }
    }

    /// Remove an in-flight key if the operation failed so the client can retry.
    pub fn release_on_failure(&self, key: &str) {
        if let Ok(mut map) = self.entries.write() {
            if let Some(IdempotencyStatus::InProgress) = map.get(key) {
                map.remove(key);
            }
        }
    }
}
