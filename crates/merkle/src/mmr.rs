//! Core Merkle Mountain Range data structure supporting incremental appends and inclusion proofs.

use serde::{Deserialize, Serialize};

use crate::error::MmrError;
use crate::hash::{bag_peaks, combine_nodes, hash_leaf, Hash32};
use crate::proof::MmrProof;

/// An internal node in the Merkle Mountain Range tree structure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct MmrNode {
    /// Domain-separated SHA-256 hash of this node.
    hash: Hash32,
    /// Distance from the bottom of the tree (leaves are height 0).
    height: u32,
    /// Position of the left child in the MMR node array, if any.
    left_child: Option<usize>,
    /// Position of the right child in the MMR node array, if any.
    right_child: Option<usize>,
    /// Position of this node's parent in the MMR node array, if any.
    parent: Option<usize>,
}

/// An append-only Merkle Mountain Range (MMR).
///
/// An MMR is a forest of perfect binary Merkle trees whose heights correspond to
/// the set bits in the binary representation of the leaf count. Adding a leaf
/// runs in amortized $O(1)$ time ($O(\log N)$ worst-case merges), and generating
/// or verifying an inclusion proof requires $O(\log N)$ hashes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleMountainRange {
    /// Array of all nodes (leaves and parents) in post-order traversal sequence.
    nodes: Vec<MmrNode>,
    /// Indices of nodes that currently serve as mountain peaks (tallest to shortest).
    peaks: Vec<usize>,
    /// Maps 0-based leaf index to its node position in [`Self::nodes`].
    leaf_positions: Vec<usize>,
}

impl MerkleMountainRange {
    /// Create an empty Merkle Mountain Range.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nodes: Vec::new(),
            peaks: Vec::new(),
            leaf_positions: Vec::new(),
        }
    }

    /// Return true if the MMR contains zero leaves.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leaf_positions.is_empty()
    }

    /// Return the total number of leaves appended to the MMR.
    #[inline]
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.leaf_positions.len()
    }

    /// Return the total number of nodes (leaves and internal parents) stored in the MMR.
    #[inline]
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Return the hashes of all current mountain peaks in descending order of height.
    #[must_use]
    pub fn peaks(&self) -> Vec<Hash32> {
        let mut out = Vec::with_capacity(self.peaks.len());
        for &idx in &self.peaks {
            if let Some(node) = self.nodes.get(idx) {
                out.push(node.hash);
            }
        }
        out
    }

    /// Return the overall Merkle root by bagging all current mountain peaks right-to-left.
    ///
    /// If the MMR is empty, returns [`Hash32::ZERO`].
    #[must_use]
    pub fn root(&self) -> Hash32 {
        bag_peaks(&self.peaks())
    }

    /// Append a pre-computed leaf hash to the MMR.
    ///
    /// Returns the 0-based leaf index of the newly appended leaf.
    pub fn append_leaf_hash(&mut self, leaf_hash: Hash32) -> usize {
        let leaf_node_idx = self.nodes.len();
        self.leaf_positions.push(leaf_node_idx);

        self.nodes.push(MmrNode {
            hash: leaf_hash,
            height: 0,
            left_child: None,
            right_child: None,
            parent: None,
        });

        let mut current_pos = leaf_node_idx;
        let mut current_height = 0;

        // Merge peaks of matching height into a single parent node.
        // This maintains the invariant that no two mountains share the same height.
        while let Some(&last_peak_idx) = self.peaks.last() {
            if self.nodes[last_peak_idx].height == current_height {
                self.peaks.pop();
                let left_idx = last_peak_idx;
                let right_idx = current_pos;

                let parent_hash =
                    combine_nodes(&self.nodes[left_idx].hash, &self.nodes[right_idx].hash);
                let parent_height = current_height + 1;
                let parent_idx = self.nodes.len();

                self.nodes.push(MmrNode {
                    hash: parent_hash,
                    height: parent_height,
                    left_child: Some(left_idx),
                    right_child: Some(right_idx),
                    parent: None,
                });

                self.nodes[left_idx].parent = Some(parent_idx);
                self.nodes[right_idx].parent = Some(parent_idx);

                current_pos = parent_idx;
                current_height = parent_height;
            } else {
                break;
            }
        }

        self.peaks.push(current_pos);
        self.leaf_positions.len() - 1
    }

    /// Hash arbitrary byte data with domain separation prefix `0x00` and append as a leaf.
    ///
    /// Returns the 0-based leaf index.
    pub fn append_leaf(&mut self, data: &[u8]) -> usize {
        self.append_leaf_hash(hash_leaf(data))
    }

    /// Return the leaf hash at the given 0-based leaf index.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::LeafIndexOutOfBounds`] if `leaf_index >= self.leaf_count()`.
    pub fn leaf_hash(&self, leaf_index: usize) -> Result<Hash32, MmrError> {
        let node_pos =
            self.leaf_positions
                .get(leaf_index)
                .copied()
                .ok_or(MmrError::LeafIndexOutOfBounds {
                    index: leaf_index,
                    count: self.leaf_positions.len(),
                })?;

        self.nodes
            .get(node_pos)
            .map(|n| n.hash)
            .ok_or(MmrError::NodeIndexOutOfBounds(node_pos))
    }

    /// Generate a cryptographic inclusion proof for the leaf at `leaf_index`.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::LeafIndexOutOfBounds`] if `leaf_index >= self.leaf_count()`.
    /// Returns [`MmrError::CorruptedTree`] if the parent/child links in the internal
    /// tree structure are missing or invalid.
    pub fn generate_proof(&self, leaf_index: usize) -> Result<MmrProof, MmrError> {
        let node_pos =
            self.leaf_positions
                .get(leaf_index)
                .copied()
                .ok_or(MmrError::LeafIndexOutOfBounds {
                    index: leaf_index,
                    count: self.leaf_positions.len(),
                })?;

        let mut curr = node_pos;
        let mut siblings = Vec::new();

        // Climb from the leaf to the mountain peak, collecting sibling hashes along the way.
        while let Some(parent_idx) = self.nodes.get(curr).and_then(|n| n.parent) {
            let parent = self.nodes.get(parent_idx).ok_or_else(|| {
                MmrError::CorruptedTree(format!("missing parent node at index {parent_idx}"))
            })?;

            if parent.left_child == Some(curr) {
                let sib_idx = parent.right_child.ok_or_else(|| {
                    MmrError::CorruptedTree(format!(
                        "node {parent_idx} missing right child for left child {curr}"
                    ))
                })?;
                let sib_node = self.nodes.get(sib_idx).ok_or_else(|| {
                    MmrError::CorruptedTree(format!("missing sibling node at index {sib_idx}"))
                })?;
                siblings.push(sib_node.hash);
            } else if parent.right_child == Some(curr) {
                let sib_idx = parent.left_child.ok_or_else(|| {
                    MmrError::CorruptedTree(format!(
                        "node {parent_idx} missing left child for right child {curr}"
                    ))
                })?;
                let sib_node = self.nodes.get(sib_idx).ok_or_else(|| {
                    MmrError::CorruptedTree(format!("missing sibling node at index {sib_idx}"))
                })?;
                siblings.push(sib_node.hash);
            } else {
                return Err(MmrError::CorruptedTree(format!(
                    "node {curr} points to parent {parent_idx} which does not reference it"
                )));
            }

            curr = parent_idx;
        }

        Ok(MmrProof {
            leaf_index,
            leaf_count: self.leaf_positions.len(),
            siblings,
            peaks: self.peaks(),
        })
    }

    /// Serialize the MMR state to binary using postcard.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::Serialization`] if serialization fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MmrError> {
        postcard::to_stdvec(self).map_err(|e| MmrError::Serialization(e.to_string()))
    }

    /// Reconstruct an MMR from serialized postcard bytes.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::Serialization`] if deserialization fails.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MmrError> {
        postcard::from_bytes(bytes).map_err(|e| MmrError::Serialization(e.to_string()))
    }
}
