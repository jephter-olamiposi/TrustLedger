//! Bounded ingress queue and internal command routing for ledger execution.

use ledger_core::account::{Account, AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::BatchOp;
use tokio::sync::oneshot;

use crate::error::IngestError;

/// Target entity referenced by a mutation operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetEntity {
    Account(AccountId),
    Transfer(TransferId),
}

/// A single ledger mutation operation.
#[derive(Clone, Debug)]
pub(crate) enum IngestOp {
    CreateAccount {
        id: AccountId,
        account_type: AccountType,
        flags: AccountFlags,
        scale: Scale,
        timestamp: u64,
    },
    CreateTransfer {
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        timestamp: u64,
    },
    CreatePending {
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        timestamp: u64,
    },
    PostPending {
        pending_id: TransferId,
        post_transfer_id: TransferId,
        amount: Amount,
        timestamp: u64,
    },
    VoidPending {
        pending_id: TransferId,
        timestamp: u64,
    },
}

impl IngestOp {
    #[must_use]
    pub(crate) fn target(&self) -> TargetEntity {
        match self {
            Self::CreateAccount { id, .. } => TargetEntity::Account(*id),
            Self::CreateTransfer { id, .. } | Self::CreatePending { id, .. } => {
                TargetEntity::Transfer(*id)
            }
            Self::PostPending {
                post_transfer_id, ..
            } => TargetEntity::Transfer(*post_transfer_id),
            Self::VoidPending { pending_id, .. } => TargetEntity::Transfer(*pending_id),
        }
    }

    pub(crate) fn into_batch_op(self, watermark: &mut u64) -> Result<BatchOp, IngestError> {
        match self {
            Self::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            } => {
                let ts = allocate_timestamp(timestamp, watermark);
                Ok(BatchOp::CreateAccount {
                    id,
                    account_type,
                    flags,
                    scale,
                    timestamp: ts,
                })
            }
            Self::CreateTransfer {
                id,
                debit_account_id,
                credit_account_id,
                amount,
                timestamp,
            } => {
                let ts = allocate_timestamp(timestamp, watermark);
                let transfer =
                    Transfer::new_immediate(id, debit_account_id, credit_account_id, amount, ts)
                        .map_err(IngestError::Ledger)?;
                Ok(BatchOp::Transfer(transfer))
            }
            Self::CreatePending {
                id,
                debit_account_id,
                credit_account_id,
                amount,
                timestamp,
            } => {
                let ts = allocate_timestamp(timestamp, watermark);
                let transfer =
                    Transfer::new_pending(id, debit_account_id, credit_account_id, amount, ts)
                        .map_err(IngestError::Ledger)?;
                Ok(BatchOp::Pending(transfer))
            }
            Self::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            } => {
                let ts = allocate_timestamp(timestamp, watermark);
                Ok(BatchOp::PostPending {
                    pending_id,
                    post_transfer_id,
                    amount,
                    timestamp: ts,
                })
            }
            Self::VoidPending {
                pending_id,
                timestamp,
            } => {
                let ts = allocate_timestamp(timestamp, watermark);
                Ok(BatchOp::VoidPending {
                    pending_id,
                    timestamp: ts,
                })
            }
        }
    }
}

/// Result returned to the caller upon successful execution of a single mutation.
#[derive(Clone, Debug)]
pub(crate) enum MutationResult {
    Account(Account),
    Transfer(Transfer),
}

/// Internal command dispatched from the ingress server to the single-writer engine.
#[derive(Debug)]
pub(crate) enum IngestCommand {
    Mutate {
        op: IngestOp,
        respond_to: oneshot::Sender<Result<MutationResult, IngestError>>,
    },
    ApplyBatch {
        ops: Vec<IngestOp>,
        respond_to: oneshot::Sender<Result<u32, IngestError>>,
    },
    GetAccount {
        id: AccountId,
        respond_to: oneshot::Sender<Result<Account, IngestError>>,
    },
    GetTransfer {
        id: TransferId,
        respond_to: oneshot::Sender<Result<Transfer, IngestError>>,
    },
}

// Invariant: monotonic timestamps guarantee event causality even under backward wall-clock adjustments.
fn allocate_timestamp(requested: u64, watermark: &mut u64) -> u64 {
    if requested == 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let ts = now.max(watermark.saturating_add(1));
        *watermark = ts;
        ts
    } else {
        *watermark = (*watermark).max(requested);
        requested
    }
}

/// Bounded queue for dispatching gRPC commands to the single-writer ledger engine.
#[derive(Clone, Debug)]
pub struct IngestQueue {
    sender: tokio::sync::mpsc::Sender<IngestCommand>,
}

impl IngestQueue {
    #[must_use]
    pub(crate) fn new(sender: tokio::sync::mpsc::Sender<IngestCommand>) -> Self {
        Self { sender }
    }

    pub(crate) fn submit(&self, command: IngestCommand) -> Result<(), IngestError> {
        self.sender.try_send(command).map_err(|err| match err {
            tokio::sync::mpsc::error::TrySendError::Full(_) => IngestError::QueueSaturated,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => IngestError::ChannelClosed,
        })
    }
}
