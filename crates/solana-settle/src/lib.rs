//! # Solana On-Chain Settlement Program & Verifier Engine
//!
//! TrustLedger's settlement layer bridges off-chain double-entry ledger state
//! to the Solana blockchain with cryptographic finality.
//!
//! ## Settlement Architecture
//!
//! 1. **Authoritative Off-Chain Ledger**: High-throughput transfers are processed
//!    in micro-batches, consensus-replicated via Raft, and journaled to an append-only WAL.
//! 2. **Cryptographic MMR Roots**: For each settlement batch, an incremental Merkle
//!    Mountain Range (MMR) computes an authenticated 32-byte state root.
//! 3. **On-Chain Commitment**: The settlement operator commits the batch root to a
//!    Solana Program Derived Address (PDA) via [`instruction::commit_settlement`].
//!    This establishes an immutable, timestamped, tamper-evident chain of roots on-chain.
//! 4. **Independent Verification**: Any auditor or client can verify inclusion of their
//!    transfer against the on-chain PDA root in $O(\log N)$ steps without trusting
//!    the operator.

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
