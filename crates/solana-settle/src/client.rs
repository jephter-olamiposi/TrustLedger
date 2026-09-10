//! Client-side receipt generation, file persistence, and audit verification helpers.

use merkle::{Hash32, MmrProof, ProofError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// A self-contained, cryptographically auditable receipt proving that a transfer
/// was finalized in a specific settlement batch and committed to Solana.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferReceipt {
    /// Unique transfer ID.
    pub transfer_id: u128,
    /// Transfer amount in base decimal units.
    pub amount: u128,
    /// Account debited.
    pub debit_account: u128,
    /// Account credited.
    pub credit_account: u128,
    /// 64-char hex SHA-256 leaf hash of the transfer.
    pub leaf_hash: String,
    /// Settlement batch sequence number.
    pub batch_seq: u64,
    /// 64-char hex Merkle root committed on Solana.
    pub merkle_root: String,
    /// Cryptographic MMR inclusion proof.
    pub proof: MmrProof,
}

impl TransferReceipt {
    /// Verify this receipt independently against the contained Merkle root and leaf hash.
    ///
    /// # Errors
    ///
    /// Returns [`ProofError`] if the proof fails cryptographic verification or if
    /// the hex strings cannot be parsed.
    pub fn verify(&self) -> Result<(), ProofError> {
        let expected_root =
            Hash32::from_hex(&self.merkle_root).map_err(|e| ProofError::RootMismatch {
                expected: self.merkle_root.clone(),
                computed: format!("invalid hex: {e}"),
            })?;

        let leaf_hash =
            Hash32::from_hex(&self.leaf_hash).map_err(|e| ProofError::RootMismatch {
                expected: self.leaf_hash.clone(),
                computed: format!("invalid leaf hex: {e}"),
            })?;

        self.proof.verify(&expected_root, &leaf_hash)
    }

    /// Save the receipt to a JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be written.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load a receipt from a JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be read or JSON parsed.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
