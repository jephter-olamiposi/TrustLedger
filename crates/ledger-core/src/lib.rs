//! Deterministic double-entry ledger engine for TrustLedger.
//!
//! The crate docs include the project README so the example flow stays aligned
//! with the repository-level contract and source-of-truth behavior.
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
