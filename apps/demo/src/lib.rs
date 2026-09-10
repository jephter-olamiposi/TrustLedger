//! # TrustLedger Reference Client, Testing Layer & Demo Application
//!
//! Demonstrates how the core distributed ledger, consensus engine, Merkle proof tree,
//! and on-chain Solana settlement interact in a realistic fintech payment flow.
//!
//! ## Core Capabilities
//!
//! 1. **Payment Lifecycle State Machine:**
//!    Models real card/fintech payment lifecycles (`Authorized` $\to$ `Captured` $\to$ `Refunded` / `Voided`)
//!    backed by two-phase ledger holds (`Transfer::new_pending`, `post_pending`, `void_pending`).
//! 2. **HMAC Webhook Ingestion & Idempotency:**
//!    Accepts card processor webhooks with constant-time HMAC-SHA256 signature verification,
//!    replay expiration checks, and atomic `event_id` deduplication.
//! 3. **Multi-Rail Settlement Adapters:**
//!    Supports switchable settlement rails including simulated banking batch netting (`MockAchRail`)
//!    and cryptographic on-chain Merkle root commitment to Solana (`SolanaUsdcRail`).
//! 4. **Three-Way Reconciliation Engine:**
//!    Audits app payment state vs double-entry balance views vs Solana PDA roots to guarantee
//!    and prove zero financial drift ($\Delta = 0$).
//! 5. **Interactive Operator Dashboard:**
//!    Serves real-time status cards, transaction views, and on-chain verification modal.

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
