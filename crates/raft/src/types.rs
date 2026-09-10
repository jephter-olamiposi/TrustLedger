//! Core type definitions, client requests, and responses for the Raft consensus layer.

use ledger_core::{
    AccountFlags, AccountId, AccountType, Amount, BatchOp, LedgerError, LedgerEvent, Scale,
    Transfer, TransferId,
};
use openraft::declare_raft_types;
use serde::{Deserialize, Serialize};

/// Client mutation request proposed through the Raft consensus log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaftRequest {
    /// Create a new double-entry ledger account.
    CreateAccount {
        /// Unique account ID.
        id: AccountId,
        /// Account classification (asset, liability, equity, revenue, expense).
        account_type: AccountType,
        /// Invariant constraints and overdraft flags.
        flags: AccountFlags,
        /// Currency decimal scale.
        scale: Scale,
        /// Wall-clock or monotonic timestamp.
        timestamp: u64,
    },
    /// Create and settle an immediate two-legged transfer.
    CreateTransfer(Transfer),
    /// Reserve funds in a two-phase pending transfer hold.
    CreatePending(Transfer),
    /// Post a pending hold, settling held funds.
    PostPending {
        /// ID of the pending transfer being settled.
        pending_id: TransferId,
        /// New transfer ID for the posted settlement record.
        post_transfer_id: TransferId,
        /// Settlement amount.
        amount: Amount,
        /// Wall-clock or sequential timestamp.
        timestamp: u64,
    },
    /// Void a pending hold, releasing reserved funds.
    VoidPending {
        /// ID of the pending transfer being voided.
        pending_id: TransferId,
        /// Cancellation timestamp.
        timestamp: u64,
    },
    /// Close an account, preventing further transactions.
    CloseAccount {
        /// ID of the account to close.
        id: AccountId,
        /// Closure timestamp.
        timestamp: u64,
    },
    /// Atomically execute a batch of operations.
    ApplyBatch(Vec<BatchOp>),
}

impl From<RaftRequest> for Vec<BatchOp> {
    fn from(req: RaftRequest) -> Self {
        match req {
            RaftRequest::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            } => vec![BatchOp::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            }],
            RaftRequest::CreateTransfer(t) => vec![BatchOp::Transfer(t)],
            RaftRequest::CreatePending(t) => vec![BatchOp::Pending(t)],
            RaftRequest::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            } => vec![BatchOp::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            }],
            RaftRequest::VoidPending {
                pending_id,
                timestamp,
            } => vec![BatchOp::VoidPending {
                pending_id,
                timestamp,
            }],
            RaftRequest::CloseAccount { id, timestamp } => {
                vec![BatchOp::CloseAccount { id, timestamp }]
            }
            RaftRequest::ApplyBatch(ops) => ops,
        }
    }
}

/// Successful response from applying a committed [`RaftRequest`] to the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaftResponse {
    /// Blank or no-op entry (e.g. leader election assertion or cluster membership entry).
    Blank,
    /// Account created successfully.
    AccountCreated(AccountId),
    /// Transfer created and settled successfully.
    TransferCreated(TransferId),
    /// Pending transfer hold reserved successfully.
    PendingCreated(TransferId),
    /// Pending hold posted successfully.
    PendingPosted(TransferId),
    /// Pending hold voided successfully.
    PendingVoided(TransferId),
    /// Account closed successfully.
    AccountClosed(AccountId),
    /// Batch applied successfully, returning the journal events produced.
    BatchApplied(Vec<LedgerEvent>),
}

/// Error returned when proposing or applying a client request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum RaftClientError {
    /// Domain ledger validation or balance invariant failure.
    #[error("ledger execution error: {0}")]
    Ledger(String),
    /// Proposal rejected because the node is not the cluster leader.
    #[error("not leader, current leader is {leader_id:?}")]
    NotLeader {
        /// Current leader node ID if known.
        leader_id: Option<u64>,
    },
    /// Consensus cluster was shut down or encountered a fatal error.
    #[error("consensus fatal error: {0}")]
    Fatal(String),
}

impl From<LedgerError> for RaftClientError {
    fn from(err: LedgerError) -> Self {
        Self::Ledger(err.to_string())
    }
}

declare_raft_types!(
    /// Type configuration mapping TrustLedger domain types to OpenRaft traits.
    pub TypeConfig:
        D = RaftRequest,
        R = Result<RaftResponse, RaftClientError>,
        NodeId = u64,
        Node = openraft::BasicNode,
        Entry = openraft::Entry<TypeConfig>,
        SnapshotData = std::io::Cursor<Vec<u8>>,
        AsyncRuntime = openraft::TokioRuntime,
);
