//! # Merkle Mountain Range (MMR) & Cryptographic Proof Engine
//!
//! An append-only, high-performance Merkle Mountain Range data structure designed for
//! auditable settlement ledgers.
//!
//! ## Mathematical Foundations
//!
//! An MMR represents an append-only log as a sequence of perfect binary Merkle trees
//! ("mountains") whose heights correspond to the binary representation of the total leaf count.
//!
//! For example, an MMR containing 11 leaves ($11 = 8 + 2 + 1 = 2^3 + 2^1 + 2^0$) consists of:
//! - Mountain 0: Height 3 ($2^3 = 8$ leaves)
//! - Mountain 1: Height 1 ($2^1 = 2$ leaves)
//! - Mountain 2: Height 0 ($2^0 = 1$ leaf)
//!
//! Adding a leaf runs in amortized $O(1)$ time, and generating or verifying an inclusion
//! proof requires $O(\log N)$ hashes (~640 bytes for 1,000,000 entries).
//!
//! ## Domain Separation (RFC 6962)
//!
//! To eliminate second pre-image attacks where intermediate node hashes are submitted as
//! leaves or peak combinations, all hashes use 1-byte domain separation prefixes:
//! - Leaves: `Sha256(0x00 || leaf_bytes)`
//! - Internal tree nodes: `Sha256(0x01 || left_child || right_child)`
//! - Peak bagging: `Sha256(0x02 || left_peak || right_peak)`

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
