//! Instruction definitions and builders for the Solana settlement program.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;

use crate::error::SettlementProgramError;
use crate::state::SettlementRoot;

/// Parameters for committing a new settlement batch.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CommitParams {
    /// Consensus epoch.
    pub epoch: u64,
    /// Batch sequence number (must equal current + 1).
    pub batch_seq: u64,
    /// Merkle Mountain Range root of this batch.
    pub merkle_root: [u8; 32],
    /// MMR root of the previous batch.
    pub previous_root: [u8; 32],
    /// Number of transfers settled in this batch.
    pub transfer_count: u64,
    /// Total monetary amount settled.
    pub total_settled_amount: u128,
    /// Chain tip hash of off-chain WAL/consensus log.
    pub chain_tip: [u8; 32],
}

/// Instructions supported by the TrustLedger Solana settlement program.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SettlementInstruction {
    /// Initialize the settlement registry PDA for an authority.
    ///
    /// Accounts:
    /// 0. `[signer]` Authority account.
    /// 1. `[writable]` Settlement root PDA account.
    /// 2. `[]` System Program.
    Initialize {
        /// Canonical bump seed for the PDA.
        bump: u8,
    },

    /// Commit a newly finalized settlement batch root to the PDA.
    ///
    /// Accounts:
    /// 0. `[signer]` Authority account.
    /// 1. `[writable]` Settlement root PDA account.
    /// 2. `[]` Clock sysvar (for recording confirmed timestamp).
    CommitSettlement(CommitParams),

    /// Verify an inclusion proof against the currently committed root on-chain.
    ///
    /// Accounts:
    /// 0. `[]` Settlement root PDA account.
    VerifyInclusion {
        /// SHA-256 leaf hash of the transfer or event.
        leaf_hash: [u8; 32],
        /// Postcard-serialized `MmrProof` bytes.
        proof_bytes: Vec<u8>,
    },
}

impl SettlementInstruction {
    /// Pack instruction into Borsh bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementProgramError::SerializationError`] if serialization fails.
    pub fn pack(&self) -> Result<Vec<u8>, SettlementProgramError> {
        borsh::to_vec(self).map_err(|e| SettlementProgramError::SerializationError(e.to_string()))
    }

    /// Unpack instruction from Borsh bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementProgramError::DeserializationError`] if deserialization fails.
    pub fn unpack(input: &[u8]) -> Result<Self, SettlementProgramError> {
        Self::try_from_slice(input)
            .map_err(|e| SettlementProgramError::DeserializationError(e.to_string()))
    }
}

/// Build an [`Instruction`] to initialize the settlement root PDA.
#[must_use]
pub fn initialize(program_id: Pubkey, authority: Pubkey, system_program: Pubkey) -> Instruction {
    let (pda, bump) = SettlementRoot::find_pda(&authority, &program_id);

    let accounts = vec![
        AccountMeta::new_readonly(authority, true),
        AccountMeta::new(pda, false),
        AccountMeta::new_readonly(system_program, false),
    ];

    let instruction_data = SettlementInstruction::Initialize { bump }
        .pack()
        .expect("Borsh packing infallible for fixed enum");

    Instruction {
        program_id,
        accounts,
        data: instruction_data,
    }
}

/// Build an [`Instruction`] to commit a new settlement batch root.
#[must_use]
pub fn commit_settlement(
    program_id: Pubkey,
    authority: Pubkey,
    params: CommitParams,
    clock_sysvar: Pubkey,
) -> Instruction {
    let (pda, _) = SettlementRoot::find_pda(&authority, &program_id);

    let accounts = vec![
        AccountMeta::new_readonly(authority, true),
        AccountMeta::new(pda, false),
        AccountMeta::new_readonly(clock_sysvar, false),
    ];

    let instruction_data = SettlementInstruction::CommitSettlement(params)
        .pack()
        .expect("Borsh packing infallible");

    Instruction {
        program_id,
        accounts,
        data: instruction_data,
    }
}

/// Build an [`Instruction`] to verify an inclusion proof on-chain against the PDA root.
#[must_use]
pub fn verify_inclusion(
    program_id: Pubkey,
    authority: Pubkey,
    leaf_hash: [u8; 32],
    proof_bytes: Vec<u8>,
) -> Instruction {
    let (pda, _) = SettlementRoot::find_pda(&authority, &program_id);

    let accounts = vec![AccountMeta::new_readonly(pda, false)];

    let instruction_data = SettlementInstruction::VerifyInclusion {
        leaf_hash,
        proof_bytes,
    }
    .pack()
    .expect("Borsh packing infallible");

    Instruction {
        program_id,
        accounts,
        data: instruction_data,
    }
}
