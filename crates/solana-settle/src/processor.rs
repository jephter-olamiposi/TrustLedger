//! Instruction processor implementing settlement commitment and inclusion proof verification.

use merkle::{Hash32, MmrProof};
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::clock::Clock;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::pubkey::Pubkey;
use solana_program::sysvar::Sysvar;

use crate::error::SettlementProgramError;
use crate::instruction::{CommitParams, SettlementInstruction};
use crate::state::SettlementRoot;

/// Program state processor.
pub struct Processor;

impl Processor {
    /// Main entrypoint that routes instructions to their respective handler functions.
    ///
    /// # Errors
    ///
    /// Returns [`solana_program::program_error::ProgramError`] if instruction parsing fails or execution invariants are violated.
    pub fn process(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        instruction_data: &[u8],
    ) -> ProgramResult {
        let instruction = SettlementInstruction::unpack(instruction_data)?;

        match instruction {
            SettlementInstruction::Initialize { bump } => {
                Self::process_initialize(program_id, accounts, bump)
            }
            SettlementInstruction::CommitSettlement(params) => {
                Self::process_commit_settlement(program_id, accounts, params)
            }
            SettlementInstruction::VerifyInclusion {
                leaf_hash,
                proof_bytes,
            } => Self::process_verify_inclusion(accounts, leaf_hash, &proof_bytes),
        }
    }

    /// Process [`SettlementInstruction::Initialize`].
    ///
    /// # Errors
    ///
    /// Returns [`solana_program::program_error::ProgramError::MissingRequiredSignature`] if the authority did not sign.
    /// Returns [`solana_program::program_error::ProgramError::InvalidSeeds`] if the PDA does not match canonical seeds.
    pub fn process_initialize(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        bump: u8,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let authority_info = next_account_info(account_info_iter)?;
        let pda_info = next_account_info(account_info_iter)?;

        // Invariant: The authority must be a signer.
        if !authority_info.is_signer {
            return Err(SettlementProgramError::UnauthorizedSigner.into());
        }

        // Invariant: PDA address must match canonical seeds.
        let expected_pda = Pubkey::create_program_address(
            &[
                SettlementRoot::PDA_SEED,
                authority_info.key.as_ref(),
                &[bump],
            ],
            program_id,
        )
        .map_err(|_| SettlementProgramError::InvalidPda)?;

        if expected_pda != *pda_info.key {
            return Err(SettlementProgramError::InvalidPda.into());
        }

        let initial_root = SettlementRoot {
            authority: *authority_info.key,
            epoch: 0,
            batch_seq: 0,
            merkle_root: [0u8; 32],
            previous_root: [0u8; 32],
            transfer_count: 0,
            total_settled_amount: 0,
            chain_tip: [0u8; 32],
            settled_at: 0,
            bump,
        };

        let mut data = pda_info.try_borrow_mut_data()?;
        initial_root.serialize_into(&mut data)?;

        msg!(
            "TrustLedger settlement registry initialized for authority: {:?}",
            authority_info.key
        );
        Ok(())
    }

    /// Process [`SettlementInstruction::CommitSettlement`].
    ///
    /// Enforces:
    /// 1. Signer authorization by the registered authority.
    /// 2. Sequential batch ordering (`batch_seq == previous + 1`).
    /// 3. Tamper-evident hash chaining (`previous_root == current.merkle_root`).
    /// 4. Non-empty transfer batch (`transfer_count > 0`).
    ///
    /// # Errors
    ///
    /// Returns [`solana_program::program_error::ProgramError`] if any settlement constraint or authorization check fails.
    pub fn process_commit_settlement(
        _program_id: &Pubkey,
        accounts: &[AccountInfo],
        params: CommitParams,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let authority_info = next_account_info(account_info_iter)?;
        let pda_info = next_account_info(account_info_iter)?;

        // Optional clock account for timestamping
        let clock_info = next_account_info(account_info_iter).ok();

        if !authority_info.is_signer {
            return Err(SettlementProgramError::UnauthorizedSigner.into());
        }

        if params.transfer_count == 0 {
            return Err(SettlementProgramError::EmptyBatch.into());
        }

        let mut data = pda_info.try_borrow_mut_data()?;
        let mut current_root = SettlementRoot::deserialize_from(&data)?;

        // Ensure authority matches registered key
        if current_root.authority != *authority_info.key {
            return Err(SettlementProgramError::UnauthorizedSigner.into());
        }

        // Enforce sequential batch numbers
        let expected_seq = current_root.batch_seq + 1;
        if params.batch_seq != expected_seq {
            return Err(SettlementProgramError::InvalidBatchSequence {
                expected: expected_seq,
                actual: params.batch_seq,
            }
            .into());
        }

        // Enforce tamper-evident chain of roots (except on the very first batch if previous is empty)
        if current_root.batch_seq > 0 && params.previous_root != current_root.merkle_root {
            return Err(SettlementProgramError::PreviousRootMismatch {
                expected: Hash32(current_root.merkle_root).to_hex(),
                actual: Hash32(params.previous_root).to_hex(),
            }
            .into());
        }

        // Get current timestamp if clock sysvar was provided, else fallback to 0
        let timestamp = clock_info
            .and_then(|info| Clock::from_account_info(info).ok())
            .map(|clock| clock.unix_timestamp)
            .unwrap_or(0);

        // Update state
        current_root.epoch = params.epoch;
        current_root.batch_seq = params.batch_seq;
        current_root.merkle_root = params.merkle_root;
        current_root.previous_root = params.previous_root;
        current_root.transfer_count = params.transfer_count;
        current_root.total_settled_amount = current_root
            .total_settled_amount
            .saturating_add(params.total_settled_amount);
        current_root.chain_tip = params.chain_tip;
        current_root.settled_at = timestamp;

        current_root.serialize_into(&mut data)?;

        msg!(
            "Settlement batch committed: seq={}, transfers={}, total_amount={}, root={}",
            params.batch_seq,
            params.transfer_count,
            params.total_settled_amount,
            Hash32(params.merkle_root).to_hex()
        );

        Ok(())
    }

    /// Process [`SettlementInstruction::VerifyInclusion`].
    ///
    /// Verifies that `leaf_hash` is part of the on-chain MMR root stored in the settlement PDA.
    ///
    /// # Errors
    ///
    /// Returns [`SettlementProgramError::InclusionProofFailed`] if the proof fails verification.
    pub fn process_verify_inclusion(
        accounts: &[AccountInfo],
        leaf_hash: [u8; 32],
        proof_bytes: &[u8],
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let pda_info = next_account_info(account_info_iter)?;

        let data = pda_info.try_borrow_data()?;
        let current_root = SettlementRoot::deserialize_from(&data)?;

        let proof = MmrProof::from_bytes(proof_bytes)
            .map_err(|e| SettlementProgramError::DeserializationError(e.to_string()))?;

        let expected_root = Hash32(current_root.merkle_root);
        let target_leaf = Hash32(leaf_hash);

        proof.verify(&expected_root, &target_leaf).map_err(|e| {
            SettlementProgramError::InclusionProofFailed(format!(
                "proof rejected against batch seq {}: {e}",
                current_root.batch_seq
            ))
        })?;

        msg!(
            "[VERIFIED] Leaf {} cryptographically verified in on-chain batch seq {}",
            target_leaf.to_hex(),
            current_root.batch_seq
        );

        Ok(())
    }
}
