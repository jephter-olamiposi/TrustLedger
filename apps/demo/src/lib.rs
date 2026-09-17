//! Demo app for TrustLedger payment flows and settlement verification.
//!
//! It exercises the ledger lifecycle used by the reference application:
//! pending holds, settlement, rails, webhook verification, and reconciliation
//! against the on-chain root.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod api;
pub mod dashboard;
pub mod error;
pub mod idempotency;
pub mod payment;
pub mod rail;
pub mod reconciliation;
pub mod server;
pub mod webhook;

pub use api::{router, AppState};
pub use error::{DemoError, PaymentError, RailError, ReconciliationError, WebhookError};
pub use idempotency::IdempotencyStore;
pub use payment::{AccountDirectory, Payment, PaymentState};
pub use rail::{MockAchRail, RailSubmission, SettlementRail, SolanaUsdcRail};
pub use reconciliation::{ReconciliationEngine, ReconciliationReport, ReconciliationStatus};
pub use server::{run_default_server, start_server};
pub use webhook::{WebhookDeduplicator, WebhookPayload, WebhookVerifier};
