//! Append-only Merkle Mountain Range commitments for settlement proofs.
//!
//! The crate builds a deterministic MMR over ledger events and exposes inclusion-proof
//! helpers for audit and verification paths. Hashes use explicit domain separation so
//! the same byte sequence cannot be confused across leaf, node, and peak positions.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod error;
pub mod event_hasher;
pub mod hash;
pub mod mmr;
pub mod proof;

pub use error::{MmrError, ProofError};
pub use event_hasher::{hash_ledger_event, hash_transfer};
pub use hash::{
    bag_peaks, combine_nodes, combine_peaks, hash_leaf, Hash32, HexError, LEAF_PREFIX, NODE_PREFIX,
    PEAK_PREFIX,
};
pub use mmr::MerkleMountainRange;
pub use proof::MmrProof;
