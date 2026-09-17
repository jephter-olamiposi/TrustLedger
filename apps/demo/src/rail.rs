//! Settlement rail adapters: Mock ACH fiat batching and Solana USDC on-chain settlement.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use ledger_core::transfer::Transfer;
use merkle::{hash_transfer, MerkleMountainRange};
use solana_program::account_info::AccountInfo;
use solana_program::pubkey::Pubkey;
use solana_settle::client::TransferReceipt;
use solana_settle::instruction::{commit_settlement, initialize, CommitParams};
use solana_settle::processor::Processor;
use solana_settle::state::SettlementRoot;

use crate::error::RailError;

/// Result of submitting a transfer batch to a settlement rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailSubmission {
    /// Name of the settlement rail used (`Mock-ACH` or `Solana-USDC`).
    pub rail_name: String,
    /// Batch sequence number.
    pub batch_seq: u64,
    /// Total transfers settled in this batch.
    pub transfer_count: usize,
    /// Cumulative monetary amount settled.
    pub total_amount: u128,
    /// Rail reference ID or on-chain transaction signature.
    pub reference: String,
    /// Merkle Mountain Range root (if committed on-chain).
    pub merkle_root: Option<String>,
}

/// A settlement rail capable of settling batches of transfers.
pub trait SettlementRail: Send + Sync {
    /// Return the human-readable identifier of this rail.
    fn name(&self) -> &'static str;

    /// Submit a batch of transfers to this rail for final settlement.
    ///
    /// # Errors
    ///
    /// Returns [`RailError`] if rail submission or on-chain commitment fails.
    fn submit_batch(
        &self,
        batch_seq: u64,
        transfers: &[Transfer],
    ) -> Result<RailSubmission, RailError>;
}

/// Simulated Automated Clearing House (ACH) fiat settlement rail.
#[derive(Debug, Default)]
pub struct MockAchRail;

impl MockAchRail {
    /// Create a new mock ACH rail.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SettlementRail for MockAchRail {
    fn name(&self) -> &'static str {
        "Mock-ACH"
    }

    fn submit_batch(
        &self,
        batch_seq: u64,
        transfers: &[Transfer],
    ) -> Result<RailSubmission, RailError> {
        let mut total_amount: u128 = 0;
        for t in transfers {
            total_amount = total_amount
                .checked_add(t.amount().as_u128())
                .ok_or(RailError::AmountOverflow)?;
        }

        let reference = format!("ACH-NACHA-BATCH-{batch_seq:06}");

        Ok(RailSubmission {
            rail_name: self.name().to_string(),
            batch_seq,
            transfer_count: transfers.len(),
            total_amount,
            reference,
            merkle_root: None,
        })
    }
}

/// On-chain Solana USDC settlement rail using Merkle Mountain Range commitments.
pub struct SolanaUsdcRail {
    program_id: Pubkey,
    authority: Pubkey,
    pda_data: RwLock<Vec<u8>>,
    latest_mmr: RwLock<MerkleMountainRange>,
    settled_transfers: RwLock<Vec<Transfer>>,
    batch_seq: AtomicU64,
}

impl SolanaUsdcRail {
    /// Create and initialize a new Solana settlement rail with a fresh PDA.
    ///
    /// # Errors
    ///
    /// Returns [`RailError`] if initial PDA setup fails.
    pub fn new(program_id: Pubkey, authority: Pubkey) -> Result<Self, RailError> {
        let (pda, _bump) = SettlementRoot::find_pda(&authority, &program_id);
        let mut pda_data = vec![0u8; SettlementRoot::LEN];
        let mut pda_lamports = 1_000_000u64;
        let mut auth_lamports = 10_000_000u64;
        let mut sys_lamports = 1u64;
        let mut auth_data = vec![];
        let mut sys_data = vec![];
        let default_owner = Pubkey::default();

        let init_ix = initialize(program_id, authority, Pubkey::default())?;
        let account_infos = vec![
            AccountInfo::new(
                &authority,
                true,
                false,
                &mut auth_lamports,
                &mut auth_data,
                &default_owner,
                false,
                0,
            ),
            AccountInfo::new(
                &pda,
                false,
                true,
                &mut pda_lamports,
                &mut pda_data,
                &program_id,
                false,
                0,
            ),
            AccountInfo::new(
                &default_owner,
                false,
                false,
                &mut sys_lamports,
                &mut sys_data,
                &default_owner,
                false,
                0,
            ),
        ];

        Processor::process(&program_id, &account_infos, &init_ix.data)
            .map_err(|e| RailError::SubmissionFailed(format!("PDA init failed: {e}")))?;

        Ok(Self {
            program_id,
            authority,
            pda_data: RwLock::new(pda_data),
            latest_mmr: RwLock::new(MerkleMountainRange::new()),
            settled_transfers: RwLock::new(Vec::new()),
            batch_seq: AtomicU64::new(0),
        })
    }

    /// Return the authority public key.
    #[must_use]
    pub const fn authority(&self) -> Pubkey {
        self.authority
    }

    /// Return the PDA address derived from authority and program id.
    #[must_use]
    pub fn pda_address(&self) -> Pubkey {
        let (pda, _) = SettlementRoot::find_pda(&self.authority, &self.program_id);
        pda
    }

    /// Return current settlement root state if initialized and batch_seq > 0.
    #[must_use]
    pub fn current_settlement_root(&self) -> Option<SettlementRoot> {
        let guard = self.pda_data.read().ok()?;
        let state = SettlementRoot::deserialize_from(&guard).ok()?;
        if state.batch_seq == 0 {
            None
        } else {
            Some(state)
        }
    }

    /// Return the latest committed Merkle root hex string, if any.
    #[must_use]
    pub fn latest_root_hex(&self) -> Option<String> {
        self.current_settlement_root().map(|state| {
            state
                .merkle_root
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        })
    }

    /// Return transfer receipt and cryptographic proof for a transfer ID if settled in the latest batch.
    ///
    /// # Errors
    ///
    /// Returns [`RailError`] if no batch has been settled or transfer is not found.
    pub fn receipt_for_transfer(&self, transfer_id: u128) -> Result<TransferReceipt, RailError> {
        let transfers = self
            .settled_transfers
            .read()
            .map_err(|_| RailError::SubmissionFailed("lock poisoned".to_string()))?;

        let batch_seq = self.batch_seq.load(Ordering::SeqCst);
        if transfers.is_empty() || batch_seq == 0 {
            return Err(RailError::SubmissionFailed(
                "No settlement batch has been committed to Solana yet".to_string(),
            ));
        }

        let index = transfers
            .iter()
            .position(|t| t.id().as_u128() == transfer_id)
            .ok_or_else(|| {
                RailError::SubmissionFailed(format!(
                    "Transfer {transfer_id} not found in latest Solana settlement batch"
                ))
            })?;

        self.generate_receipt(batch_seq, index, &transfers[index])
    }

    /// Generate an inclusion proof and receipt for a specific transfer in the latest settled batch.
    ///
    /// # Errors
    ///
    /// Returns [`RailError`] if proof generation fails.
    pub fn generate_receipt(
        &self,
        batch_seq: u64,
        transfer_index: usize,
        transfer: &Transfer,
    ) -> Result<TransferReceipt, RailError> {
        let mmr = self
            .latest_mmr
            .read()
            .map_err(|_| RailError::SubmissionFailed("lock poisoned".to_string()))?;

        let leaf_hash = hash_transfer(transfer)?;
        let proof = mmr.generate_proof(transfer_index)?;

        Ok(TransferReceipt {
            transfer_id: transfer.id().as_u128(),
            amount: transfer.amount().as_u128(),
            debit_account: transfer.debit_account_id().as_u128(),
            credit_account: transfer.credit_account_id().as_u128(),
            leaf_hash: leaf_hash.to_hex(),
            batch_seq,
            merkle_root: mmr.root().to_hex(),
            proof,
        })
    }
}

impl SettlementRail for SolanaUsdcRail {
    fn name(&self) -> &'static str {
        "Solana-USDC"
    }

    fn submit_batch(
        &self,
        batch_seq: u64,
        transfers: &[Transfer],
    ) -> Result<RailSubmission, RailError> {
        if transfers.is_empty() {
            return Err(RailError::SubmissionFailed(
                "cannot settle empty batch".to_string(),
            ));
        }

        let mut mmr = MerkleMountainRange::new();
        let mut total_amount: u128 = 0;

        for t in transfers {
            mmr.append_transfer(t)?;
            total_amount = total_amount
                .checked_add(t.amount().as_u128())
                .ok_or(RailError::AmountOverflow)?;
        }

        let root = mmr.root();

        let (pda, _) = SettlementRoot::find_pda(&self.authority, &self.program_id);
        let mut pda_data_guard = self
            .pda_data
            .write()
            .map_err(|_| RailError::SubmissionFailed("lock poisoned".to_string()))?;

        let current_state = SettlementRoot::deserialize_from(&pda_data_guard)
            .map_err(|e| RailError::SubmissionFailed(e.to_string()))?;

        let params = CommitParams {
            epoch: 1,
            batch_seq,
            merkle_root: *root.as_bytes(),
            previous_root: current_state.merkle_root,
            transfer_count: transfers.len() as u64,
            total_settled_amount: total_amount,
            chain_tip: [0xaa; 32],
        };

        let commit_ix =
            commit_settlement(self.program_id, self.authority, params, Pubkey::default())?;

        let mut auth_lamports = 10_000_000u64;
        let mut pda_lamports = 1_000_000u64;
        let mut auth_data = vec![];
        let default_owner = Pubkey::default();

        let account_infos = vec![
            AccountInfo::new(
                &self.authority,
                true,
                false,
                &mut auth_lamports,
                &mut auth_data,
                &default_owner,
                false,
                0,
            ),
            AccountInfo::new(
                &pda,
                false,
                true,
                &mut pda_lamports,
                &mut pda_data_guard,
                &self.program_id,
                false,
                0,
            ),
        ];

        Processor::process(&self.program_id, &account_infos, &commit_ix.data)
            .map_err(|e| RailError::SubmissionFailed(format!("on-chain commit error: {e}")))?;

        if let Ok(mut mmr_guard) = self.latest_mmr.write() {
            *mmr_guard = mmr;
        }

        if let Ok(mut transfers_guard) = self.settled_transfers.write() {
            *transfers_guard = transfers.to_vec();
        }
        self.batch_seq.store(batch_seq, Ordering::SeqCst);

        let reference = format!("solana:tx:batch-{batch_seq}:root:{}", root.to_hex());

        Ok(RailSubmission {
            rail_name: self.name().to_string(),
            batch_seq,
            transfer_count: transfers.len(),
            total_amount,
            reference,
            merkle_root: Some(root.to_hex()),
        })
    }
}
