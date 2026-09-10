//! Domain error types for the TrustLedger reference client and demo application.

use thiserror::Error;

/// Root error type for the demo application.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DemoError {
    /// Errors originating from payment state machine operations.
    #[error("payment error: {0}")]
    Payment(#[from] PaymentError),

    /// Errors originating from webhook ingestion and signature validation.
    #[error("webhook error: {0}")]
    Webhook(#[from] WebhookError),

    /// Errors originating from idempotency key management.
    #[error("idempotency conflict: {0}")]
    Idempotency(#[from] IdempotencyError),

    /// Errors originating from settlement rail adapters.
    #[error("settlement rail error: {0}")]
    Rail(#[from] RailError),

    /// Errors originating from three-way reconciliation audits.
    #[error("reconciliation error: {0}")]
    Reconciliation(#[from] ReconciliationError),
}

/// Errors originating from payment state machine operations.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PaymentError {
    /// Requested transition is illegal according to the payment lifecycle.
    #[error("illegal payment transition from {from:?} to {to:?} for payment {payment_id}")]
    InvalidStateTransition {
        /// Payment identifier.
        payment_id: u128,
        /// Current lifecycle state.
        from: String,
        /// Attempted target state.
        to: String,
    },

    /// The requested payment identifier was not found.
    #[error("payment {0} not found")]
    PaymentNotFound(u128),

    /// An error returned by the underlying double-entry ledger.
    #[error("ledger core error: {0}")]
    Ledger(#[from] ledger_core::error::LedgerError),

    /// Transfer amount arithmetic overflowed.
    #[error("amount arithmetic overflow")]
    AmountOverflow,
}

/// Errors originating from webhook ingestion and verification.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WebhookError {
    /// The required HMAC signature header was missing from the request.
    #[error("missing webhook signature header")]
    MissingSignature,

    /// The computed HMAC-SHA256 signature does not match the provided header.
    #[error("invalid webhook signature: payload verification failed")]
    InvalidSignature,

    /// Webhook timestamp is outside the allowed replay tolerance window.
    #[error("expired webhook timestamp: event is {age_seconds}s old (max allowed {max_allowed}s)")]
    ExpiredTimestamp {
        /// Calculated age of the webhook in seconds.
        age_seconds: u64,
        /// Maximum allowed window in seconds.
        max_allowed: u64,
    },

    /// A webhook with this `event_id` has already been ingested.
    #[error("duplicate webhook event_id: {0}")]
    DuplicateEventId(String),

    /// Webhook payload serialization or deserialization failed.
    #[error("payload parsing error: {0}")]
    PayloadError(String),
}

/// Errors originating from idempotency locking.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum IdempotencyError {
    /// A concurrent operation with the same idempotency key is currently in-flight.
    #[error("concurrent operation with idempotency key '{0}' in-flight")]
    ConcurrentRequest(String),
}

/// Errors originating from settlement rail adapters.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RailError {
    /// Batch submission to the settlement rail failed.
    #[error("rail submission failed: {0}")]
    SubmissionFailed(String),

    /// On-chain settlement transaction rejected.
    #[error("solana settlement error: {0}")]
    Solana(#[from] solana_settle::error::SettlementProgramError),

    /// Merkle inclusion proof generation failed.
    #[error("merkle proof generation error: {0}")]
    Merkle(#[from] merkle::error::MmrError),
}

/// Errors originating from three-way reconciliation audits.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ReconciliationError {
    /// A discrepancy was found between app events, ledger journal, and on-chain roots.
    #[error(
        "reconciliation drift detected: app={app_amount}, ledger={ledger_amount}, chain={chain_amount}, drift={drift}"
    )]
    DriftDetected {
        /// Total settled amount according to app payment records.
        app_amount: u128,
        /// Total settled amount according to double-entry ledger balances.
        ledger_amount: u128,
        /// Total settled amount committed on Solana.
        chain_amount: u128,
        /// Calculated discrepancy (drift != 0).
        drift: i128,
    },

    /// Account not found in ledger during reconciliation audit.
    #[error("account {0} not found in ledger")]
    AccountNotFound(u128),
}
