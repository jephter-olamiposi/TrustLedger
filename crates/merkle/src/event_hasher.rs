//! Canonical leaf hashing for domain transfers and journal events.

use ledger_core::journal::LedgerEvent;
use ledger_core::transfer::Transfer;

use crate::error::MmrError;
use crate::hash::{hash_leaf, Hash32};
use crate::mmr::MerkleMountainRange;

/// Compute the canonical leaf hash of a [`Transfer`] using deterministic postcard serialization.
///
/// Output: `Sha256(0x00 || postcard_serialize(transfer))`.
///
/// # Errors
///
/// Returns [`MmrError::Serialization`] if postcard fails to serialize the transfer.
pub fn hash_transfer(transfer: &Transfer) -> Result<Hash32, MmrError> {
    let bytes =
        postcard::to_stdvec(transfer).map_err(|e| MmrError::Serialization(e.to_string()))?;
    Ok(hash_leaf(&bytes))
}

/// Compute the canonical leaf hash of a [`LedgerEvent`] using deterministic postcard serialization.
///
/// Output: `Sha256(0x00 || postcard_serialize(event))`.
///
/// # Errors
///
/// Returns [`MmrError::Serialization`] if postcard fails to serialize the event.
pub fn hash_ledger_event(event: &LedgerEvent) -> Result<Hash32, MmrError> {
    let bytes = postcard::to_stdvec(event).map_err(|e| MmrError::Serialization(e.to_string()))?;
    Ok(hash_leaf(&bytes))
}

impl MerkleMountainRange {
    /// Serialize a [`Transfer`] deterministically and append it as an MMR leaf.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::Serialization`] if postcard serialization fails.
    pub fn append_transfer(&mut self, transfer: &Transfer) -> Result<usize, MmrError> {
        let hash = hash_transfer(transfer)?;
        Ok(self.append_leaf_hash(hash))
    }

    /// Serialize a [`LedgerEvent`] deterministically and append it as an MMR leaf.
    ///
    /// # Errors
    ///
    /// Returns [`MmrError::Serialization`] if postcard serialization fails.
    pub fn append_ledger_event(&mut self, event: &LedgerEvent) -> Result<usize, MmrError> {
        let hash = hash_ledger_event(event)?;
        Ok(self.append_leaf_hash(hash))
    }
}
