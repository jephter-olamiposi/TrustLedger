//! Error types for the Solana settlement program and client verification.

use solana_program::program_error::ProgramError;
use thiserror::Error;

/// Domain errors that can occur during settlement program execution or proof verification.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SettlementProgramError {
    /// The caller is not the authorized settlement authority signer.
    #[error("unauthorized signer: transaction must be signed by settlement authority")]
    UnauthorizedSigner,

    /// The provided PDA does not match the canonical seeds and bump.
    #[error("invalid PDA: address does not match canonical seeds")]
    InvalidPda,

    /// A batch was submitted with a non-sequential batch number.
    #[error("invalid batch sequence: expected {expected}, got {actual}")]
    InvalidBatchSequence {
        /// Expected next batch sequence number.
        expected: u64,
        /// Actual submitted batch sequence number.
        actual: u64,
    },

    /// The `previous_root` declared in the batch does not match the currently committed root.
    #[error("previous root mismatch: expected {expected}, got {actual}")]
    PreviousRootMismatch {
        /// Expected root hash (hex string).
        expected: String,
        /// Actual root hash (hex string).
        actual: String,
    },

    /// A batch declared zero transfers, which is prohibited.
    #[error("empty settlement batch: transfer count must be greater than zero")]
    EmptyBatch,

    /// The Merkle inclusion proof failed verification against the committed batch root.
    #[error("merkle inclusion proof failed: {0}")]
    InclusionProofFailed(String),

    /// Deserialization of instruction data or account state failed.
    #[error("deserialization error: {0}")]
    DeserializationError(String),

    /// Serialization of account state failed.
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// The account data buffer is too small to hold the settlement state.
    #[error("account data buffer is too small")]
    AccountDataTooSmall,
}

impl From<SettlementProgramError> for ProgramError {
    fn from(err: SettlementProgramError) -> Self {
        match err {
            SettlementProgramError::UnauthorizedSigner => Self::MissingRequiredSignature,
            SettlementProgramError::InvalidPda => Self::InvalidSeeds,
            SettlementProgramError::InvalidBatchSequence { .. } => Self::Custom(1001),
            SettlementProgramError::PreviousRootMismatch { .. } => Self::Custom(1002),
            SettlementProgramError::EmptyBatch => Self::Custom(1003),
            SettlementProgramError::InclusionProofFailed(_) => Self::Custom(1004),
            SettlementProgramError::DeserializationError(_) => Self::InvalidInstructionData,
            SettlementProgramError::SerializationError(_) => Self::AccountDataTooSmall,
            SettlementProgramError::AccountDataTooSmall => Self::AccountDataTooSmall,
        }
    }
}
