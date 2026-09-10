//! On-chain state representations and PDA seed derivations for settlement batches.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

use crate::error::SettlementProgramError;

/// The primary PDA account storing the on-chain Merkle root commitment and settlement state.
///
/// Derived using seeds: `[SettlementRoot::PDA_SEED, authority.as_ref()]`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SettlementRoot {
    /// The authorized ledger operator/consensus key permitted to commit settlements.
    pub authority: Pubkey,
    /// Ledger consensus epoch.
    pub epoch: u64,
    /// Monotonically increasing batch sequence number (0 upon initialization).
    pub batch_seq: u64,
    /// Merkle Mountain Range (MMR) root of the settled transfer batch.
    pub merkle_root: [u8; 32],
    /// MMR root of the preceding settlement batch, establishing a tamper-evident hash chain.
    pub previous_root: [u8; 32],
    /// Total number of individual transfers settled within this batch.
    pub transfer_count: u64,
    /// Cumulative monetary amount settled across this batch.
    pub total_settled_amount: u128,
    /// Off-chain write-ahead log / consensus chain tip hash for audit cross-referencing.
    pub chain_tip: [u8; 32],
    /// Unix timestamp (seconds) when this batch commitment was confirmed on-chain.
    pub settled_at: i64,
    /// Canonical bump seed for the Program Derived Address (PDA).
    pub bump: u8,
}

impl SettlementRoot {
    /// Seed prefix used to derive the Program Derived Address for the settlement root.
    pub const PDA_SEED: &'static [u8] = b"settlement_root";

    /// Serialized size of [`SettlementRoot`] in bytes.
    ///
    /// 32 (authority) + 8 (epoch) + 8 (batch_seq) + 32 (merkle_root) + 32 (previous_root)
    /// + 8 (transfer_count) + 16 (total_settled_amount) + 32 (chain_tip) + 8 (settled_at) + 1 (bump) = 177 bytes.
    pub const LEN: usize = 177;

    /// Derive the Program Derived Address (PDA) and bump seed for a given authority.
    #[must_use]
    pub fn find_pda(authority: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::PDA_SEED, authority.as_ref()], program_id)
    }

    /// Serialize the state into a byte slice using Borsh.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementProgramError::SerializationError`] if serialization fails,
    /// or [`SettlementProgramError::AccountDataTooSmall`] if `output.len() < Self::LEN`.
    pub fn serialize_into(&self, output: &mut [u8]) -> Result<(), SettlementProgramError> {
        if output.len() < Self::LEN {
            return Err(SettlementProgramError::AccountDataTooSmall);
        }

        let mut writer = &mut output[..];
        self.serialize(&mut writer)
            .map_err(|e| SettlementProgramError::SerializationError(e.to_string()))
    }

    /// Deserialize the state from a byte slice using Borsh.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementProgramError::DeserializationError`] if deserialization fails.
    pub fn deserialize_from(input: &[u8]) -> Result<Self, SettlementProgramError> {
        Self::try_from_slice(input)
            .map_err(|e| SettlementProgramError::DeserializationError(e.to_string()))
    }
}
