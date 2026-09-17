//! Solana settlement commitments for TrustLedger batch roots.
//!
//! The off-chain ledger produces MMR roots for settled batches and the program in this
//! crate records those roots on-chain for independent verification. The public API is
//! intentionally narrow: initialize the PDA, commit a new root, and verify inclusion.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod client;
pub mod entrypoint;
pub mod error;
pub mod instruction;
pub mod processor;
pub mod state;

pub use client::TransferReceipt;
pub use error::SettlementProgramError;
pub use instruction::{
    commit_settlement, initialize, verify_inclusion, CommitParams, SettlementInstruction,
};
pub use processor::Processor;
pub use state::SettlementRoot;
