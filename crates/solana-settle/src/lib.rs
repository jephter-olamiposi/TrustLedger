//! On-chain settlement commitments for TrustLedger batch roots.
//!
//! The off-chain ledger produces MMR roots for settled batches; this crate records
//! those roots on-chain and verifies inclusion against the canonical state.

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
