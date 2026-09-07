//! Deterministic double-entry ledger engine for TrustLedger.
//!
//! Enforces conservation of balance (total debits equal total credits),
//! fixed-point integer math, two-phase transfers, and deterministic replay.
//!
//! # Examples
//!
//! Two-phase reservation followed by settlement capture:
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use ledger_core::account::{AccountFlags, AccountType};
//! use ledger_core::amount::{Amount, Scale};
//! use ledger_core::id::{AccountId, TransferId};
//! use ledger_core::transfer::Transfer;
//! use ledger_core::Ledger;
//!
//! let mut ledger = Ledger::new(Scale::usdc());
//! let vault = AccountId::new(1);
//! let alice = AccountId::new(2);
//!
//! ledger.create_account(vault, AccountType::Asset, AccountFlags::bank_asset(), Scale::usdc(), 0)?;
//! ledger.create_account(alice, AccountType::Liability, AccountFlags::customer(), Scale::usdc(), 0)?;
//!
//! let hold = Transfer::new_pending(TransferId::new(1), vault, alice, Amount::new(1_000_000), 0)?;
//! ledger.create_pending(hold)?;
//! ledger.post_pending(TransferId::new(1), TransferId::new(2), Amount::new(1_000_000), 0)?;
//!
//! ledger.verify_invariants()?;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod account;
pub mod amount;
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
pub use ledger::Ledger;
pub use transfer::{Transfer, TransferState};
