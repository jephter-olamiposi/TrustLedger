//! Cryptographic inclusion proofs and verification for Merkle Mountain Ranges.

use serde::{Deserialize, Serialize};

use crate::error::ProofError;
use crate::hash::{bag_peaks, combine_nodes, Hash32};

/// An inclusion proof demonstrating that a specific leaf exists in a Merkle Mountain Range.
///
/// The proof consists of:
/// 1. The path of sibling hashes from the leaf to the peak of its mountain.
/// 2. The list of all mountain peaks, which are bagged together to form the MMR root.
///
/// A verifier requires only this proof, the leaf hash, and the expected MMR root to verify
/// inclusion without needing access to the full ledger or tree history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmrProof {
    /// 0-based index of the leaf within the MMR.
    pub leaf_index: usize,
    /// Total number of leaves in the MMR at the time this proof was generated.
    pub leaf_count: usize,
    /// Hashes of sibling nodes along the path from the leaf to its mountain peak.
    pub siblings: Vec<Hash32>,
    /// Hashes of all mountain peaks in the MMR (ordered from tallest/left to shortest/right).
    pub peaks: Vec<Hash32>,
}

impl MmrProof {
    /// Verify that this proof demonstrates inclusion of `leaf_hash` in an MMR with root `expected_root`.
    ///
    /// # Errors
    ///
    /// - Returns [`ProofError::EmptyTree`] if `leaf_count` is 0.
    /// - Returns [`ProofError::LeafIndexOutOfBounds`] if `leaf_index >= leaf_count`.
    /// - Returns [`ProofError::PeakCountMismatch`] if `peaks.len()` does not equal the number
    ///   of set bits in `leaf_count`.
    /// - Returns [`ProofError::SiblingCountMismatch`] if `siblings.len()` does not equal the
    ///   height of the containing mountain.
    /// - Returns [`ProofError::PeakMismatch`] if the candidate peak computed from the leaf and
    ///   sibling path does not match the peak recorded in `peaks`.
    /// - Returns [`ProofError::RootMismatch`] if bagging the peaks does not match `expected_root`.
    pub fn verify(&self, expected_root: &Hash32, leaf_hash: &Hash32) -> Result<(), ProofError> {
        if self.leaf_count == 0 {
            return Err(ProofError::EmptyTree);
        }

        if self.leaf_index >= self.leaf_count {
            return Err(ProofError::LeafIndexOutOfBounds {
                index: self.leaf_index,
                count: self.leaf_count,
            });
        }

        // Each mountain corresponds to a set bit in the binary representation of leaf_count.
        let expected_peaks = self.leaf_count.count_ones() as usize;
        if self.peaks.len() != expected_peaks {
            return Err(ProofError::PeakCountMismatch {
                expected: expected_peaks,
                actual: self.peaks.len(),
            });
        }

        // Decompose leaf_count into powers of 2 (tallest to shortest mountain) to locate
        // which mountain contains this leaf index.
        let mut rem = self.leaf_count;
        let mut offset = 0;
        let mut target_peak_idx = None;
        let mut target_mountain_size = 0;
        let mut current_peak_idx = 0;

        while rem > 0 {
            let power = 63 - rem.leading_zeros() as usize;
            let size = 1usize << power;
            if self.leaf_index >= offset && self.leaf_index < offset + size {
                target_peak_idx = Some(current_peak_idx);
                target_mountain_size = size;
                break;
            }
            offset += size;
            rem -= size;
            current_peak_idx += 1;
        }

        // Safe: leaf_index < leaf_count was verified above, so it is strictly bounded.
        let peak_idx = match target_peak_idx {
            Some(idx) => idx,
            None => {
                return Err(ProofError::LeafIndexOutOfBounds {
                    index: self.leaf_index,
                    count: self.leaf_count,
                })
            }
        };

        // A mountain of size 2^k has height k and requires exactly k sibling hashes.
        let expected_siblings = 63 - target_mountain_size.leading_zeros() as usize;
        if self.siblings.len() != expected_siblings {
            return Err(ProofError::SiblingCountMismatch {
                expected: expected_siblings,
                actual: self.siblings.len(),
            });
        }

        // Reconstruct the candidate peak from the bottom up.
        // In a perfect binary tree of size 2^k, the binary representation of local_index
        // dictates whether the node is a left child (bit=0) or right child (bit=1) at height h.
        let local_index = self.leaf_index - offset;
        let mut curr = *leaf_hash;

        for (h, sibling) in self.siblings.iter().enumerate() {
            let is_right_child = ((local_index >> h) & 1) == 1;
            if is_right_child {
                // Sibling is to our left: combine(sibling, curr)
                curr = combine_nodes(sibling, &curr);
            } else {
                // Sibling is to our right: combine(curr, sibling)
                curr = combine_nodes(&curr, sibling);
            }
        }

        // Verify the candidate peak matches the peak recorded in the proof.
        let expected_peak = self.peaks[peak_idx];
        if curr != expected_peak {
            return Err(ProofError::PeakMismatch {
                expected: expected_peak.to_hex(),
                computed: curr.to_hex(),
                peak_index: peak_idx,
            });
        }

        // Verify that bagging the peaks matches the expected root commitment.
        let computed_root = bag_peaks(&self.peaks);
        if computed_root != *expected_root {
            return Err(ProofError::RootMismatch {
                expected: expected_root.to_hex(),
                computed: computed_root.to_hex(),
            });
        }

        Ok(())
    }

    /// Serialize this proof into a compact binary representation using postcard.
    ///
    /// # Errors
    ///
    /// Returns [`ProofError::Serialization`] if postcard serialization fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProofError> {
        postcard::to_stdvec(self).map_err(|e| ProofError::Serialization(e.to_string()))
    }

    /// Deserialize a proof from binary bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ProofError::Serialization`] if postcard deserialization fails.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProofError> {
        postcard::from_bytes(bytes).map_err(|e| ProofError::Serialization(e.to_string()))
    }
}
