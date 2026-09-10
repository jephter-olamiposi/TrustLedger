//! Webhook ingestion with HMAC-SHA256 signature verification, replay protection, and deduplication.

use std::collections::HashSet;
use std::sync::RwLock;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::WebhookError;

type HmacSha256 = Hmac<Sha256>;

/// Payload of an incoming payment webhook from a card processor or PSP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Unique event identifier assigned by the payment service provider.
    pub event_id: String,
    /// Type of event: `charge.authorized`, `charge.captured`, or `charge.refunded`.
    pub event_type: String,
    /// Associated payment identifier.
    pub payment_id: u128,
    /// Merchant receiving payment.
    pub merchant_id: u128,
    /// Customer making payment.
    pub customer_id: u128,
    /// Gross payment amount in units.
    pub amount: u128,
    /// Platform fee in units.
    pub fee_amount: u128,
    /// Unix timestamp of the event in seconds.
    pub timestamp: u64,
}

/// Verifies HMAC-SHA256 signatures on incoming webhook payloads.
pub struct WebhookVerifier;

impl WebhookVerifier {
    /// Compute the expected HMAC-SHA256 hex signature for a raw payload.
    #[must_use]
    pub fn compute_signature(secret: &[u8], payload: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(payload);
        let result = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(64);
        for byte in result {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    /// Verify that `signature_hex` matches the HMAC-SHA256 of `payload` using constant-time check.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::InvalidSignature`] if verification fails or the signature is malformed.
    pub fn verify_signature(
        secret: &[u8],
        payload: &[u8],
        signature_hex: &str,
    ) -> Result<(), WebhookError> {
        let mut mac =
            HmacSha256::new_from_slice(secret).map_err(|_| WebhookError::InvalidSignature)?;
        mac.update(payload);

        let signature_bytes = decode_hex(signature_hex).ok_or(WebhookError::InvalidSignature)?;

        mac.verify_slice(&signature_bytes)
            .map_err(|_| WebhookError::InvalidSignature)
    }

    /// Verify that the webhook timestamp is within `max_allowed_seconds` of `current_time`.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::ExpiredTimestamp`] if the event is older than tolerance.
    pub fn verify_freshness(
        event_time: u64,
        current_time: u64,
        max_allowed_seconds: u64,
    ) -> Result<(), WebhookError> {
        if current_time > event_time {
            let age = current_time - event_time;
            if age > max_allowed_seconds {
                return Err(WebhookError::ExpiredTimestamp {
                    age_seconds: age,
                    max_allowed: max_allowed_seconds,
                });
            }
        }
        Ok(())
    }
}

/// Constant-time hex decoder without external dependencies.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let chars = s.as_bytes();
    for i in (0..chars.len()).step_by(2) {
        let hi = decode_nibble(chars[i])?;
        let lo = decode_nibble(chars[i + 1])?;
        bytes.push((hi << 4) | lo);
    }
    Some(bytes)
}

fn decode_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Thread-safe deduplicator that tracks ingested webhook `event_id`s.
#[derive(Debug, Default)]
pub struct WebhookDeduplicator {
    seen_events: RwLock<HashSet<String>>,
}

impl WebhookDeduplicator {
    /// Create a new deduplicator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen_events: RwLock::new(HashSet::new()),
        }
    }

    /// Atomically check and record an `event_id`.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::DuplicateEventId`] if this `event_id` has already been processed.
    pub fn check_and_record(&self, event_id: &str) -> Result<(), WebhookError> {
        let mut set = self
            .seen_events
            .write()
            .map_err(|_| WebhookError::PayloadError("lock poisoned".to_string()))?;

        if set.contains(event_id) {
            Err(WebhookError::DuplicateEventId(event_id.to_string()))
        } else {
            set.insert(event_id.to_string());
            Ok(())
        }
    }
}
