//! Error types for Merkle Mountain Range operations and cryptographic proof verification.

/// Errors that can occur during Merkle Mountain Range construction, appending, or proof generation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MmrError {
    /// Requested leaf index exceeds current leaf count.
    #[error("leaf index {index} is out of bounds for leaf count {count}")]
    LeafIndexOutOfBounds {
        /// Requested leaf index.
        index: usize,
        /// Total number of leaves in the MMR.
        count: usize,
    },

    /// MMR has no leaves.
    #[error("merkle mountain range is empty")]
    EmptyMmr,

    /// Node position out of bounds in MMR node array.
    #[error("node index {0} out of bounds")]
    NodeIndexOutOfBounds(usize),

    /// Proof verification failed.
    #[error("proof verification failed: {0}")]
    Proof(#[from] ProofError),

    /// Postcard serialization or deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Corrupted tree state detected during traversal.
    #[error("corrupted tree state: {0}")]
    CorruptedTree(String),
}

/// Errors that can occur during Merkle inclusion proof verification.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The computed Merkle root does not match the expected commitment.
    #[error("root hash mismatch: expected {expected}, computed {computed}")]
    RootMismatch {
        /// Expected root hash (hex string).
        expected: String,
        /// Computed candidate root hash (hex string).
        computed: String,
    },

    /// Candidate peak derived from leaf and sibling path does not match the peak in proof.
    #[error("peak mismatch at peak {peak_index}: expected {expected}, computed {computed}")]
    PeakMismatch {
        /// Expected peak hash from the proof peaks array.
        expected: String,
        /// Computed peak hash from leaf + sibling hashes.
        computed: String,
        /// Index of the peak in the MMR peaks array.
        peak_index: usize,
    },

    /// Leaf index in proof is out of bounds for the tree size.
    #[error("proof leaf index {index} out of bounds for leaf count {count}")]
    LeafIndexOutOfBounds {
        /// Proof leaf index.
        index: usize,
        /// Total leaves declared in proof.
        count: usize,
    },

    /// Number of sibling hashes provided does not match the mountain height.
    #[error("sibling count mismatch: expected {expected}, actual {actual}")]
    SiblingCountMismatch {
        /// Expected sibling path count.
        expected: usize,
        /// Actual sibling path count.
        actual: usize,
    },

    /// The list of peaks in the proof does not match the peak count for the leaf count.
    #[error("peak count mismatch: expected {expected}, actual {actual}")]
    PeakCountMismatch {
        /// Expected number of mountain peaks.
        expected: usize,
        /// Actual number of mountain peaks.
        actual: usize,
    },

    /// Proof cannot verify against an empty tree.
    #[error("cannot verify proof on an empty tree")]
    EmptyTree,

    /// Postcard serialization or deserialization failure for proof bytes.
    #[error("proof serialization error: {0}")]
    Serialization(String),
}
