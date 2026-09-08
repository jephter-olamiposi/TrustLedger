//! TrustLedger's deterministic double-entry ledger engine.
//!
//! The crate-level documentation is the repo README (included verbatim below),
//! so its `rust` example is the single source of truth for the two-phase flow:
//! `cargo test --doc` compiles it directly (see ADR-0008).
#![doc = include_str!("../../../README.md")]
#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod account;
pub mod amount;
pub mod codec;
pub mod error;
pub mod id;
pub mod journal;
pub mod ledger;
pub mod transfer;

pub use account::{Account, AccountFlags, AccountType, Balance};
pub use amount::{Amount, Scale, MAX_SCALE};
pub use error::LedgerError;
pub use id::{AccountId, TransferId};
pub use journal::LedgerEvent;
pub use ledger::{BatchOp, Ledger};
pub use transfer::{Transfer, TransferState};
