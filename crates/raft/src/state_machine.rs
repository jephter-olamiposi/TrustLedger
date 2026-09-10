//! Replicated state machine wrapping TrustLedger's double-entry core.
//!
//! Applies committed Raft log entries sequentially to maintain strict
//! single-writer consistency and balance conservation invariants.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ledger_core::{BatchOp, Ledger, LedgerEvent, Scale};
use openraft::storage::{RaftSnapshotBuilder, RaftStateMachine, Snapshot, SnapshotMeta};
use openraft::{
    AnyError, EntryPayload, LogId, OptionalSend, StorageError, StorageIOError, StoredMembership,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::types::{RaftClientError, RaftRequest, RaftResponse, TypeConfig};

/// Internal state machine state protected by an asynchronous `RwLock`.
#[derive(Debug)]
struct StateMachineInner {
    /// In-memory ledger holding accounts, transfers, and balance invariants.
    ledger: Ledger,
    /// Last committed log ID successfully applied to the state machine.
    last_applied_log_id: Option<LogId<u64>>,
    /// Last membership configuration applied.
    last_membership: StoredMembership<u64, openraft::BasicNode>,
    /// Latest snapshot retained by the state machine.
    current_snapshot: Option<Snapshot<TypeConfig>>,
}

/// Serialized payload format for state machine snapshots.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotPayload {
    scale: Scale,
    journal: Vec<LedgerEvent>,
}

/// Snapshot builder producing point-in-time state machine checkpoints.
#[derive(Debug)]
pub struct LedgerSnapshotBuilder {
    meta: SnapshotMeta<u64, openraft::BasicNode>,
    payload: Vec<u8>,
}

impl RaftSnapshotBuilder<TypeConfig> for LedgerSnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<u64>> {
        Ok(Snapshot {
            meta: self.meta.clone(),
            snapshot: Box::new(Cursor::new(self.payload.clone())),
        })
    }
}

/// Replicated state machine executing committed operations against [`Ledger`].
#[derive(Clone, Debug)]
pub struct LedgerStateMachine {
    inner: Arc<RwLock<StateMachineInner>>,
}

impl LedgerStateMachine {
    /// Creates a new replicated state machine initialized with the given currency [`Scale`].
    #[must_use]
    pub fn new(scale: Scale) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StateMachineInner {
                ledger: Ledger::new(scale),
                last_applied_log_id: None,
                last_membership: StoredMembership::default(),
                current_snapshot: None,
            })),
        }
    }

    /// Read-only inspection of the underlying [`Ledger`].
    pub async fn read_ledger<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Ledger) -> R,
    {
        let inner = self.inner.read().await;
        f(&inner.ledger)
    }

    /// Returns the last applied log ID.
    pub async fn last_applied_log_id(&self) -> Option<LogId<u64>> {
        self.inner.read().await.last_applied_log_id
    }
}

impl RaftStateMachine<TypeConfig> for LedgerStateMachine {
    type SnapshotBuilder = LedgerSnapshotBuilder;

    async fn applied_state(
        &mut self,
    ) -> Result<
        (
            Option<LogId<u64>>,
            StoredMembership<u64, openraft::BasicNode>,
        ),
        StorageError<u64>,
    > {
        let inner = self.inner.read().await;
        Ok((inner.last_applied_log_id, inner.last_membership.clone()))
    }

    async fn apply<I>(
        &mut self,
        entries: I,
    ) -> Result<Vec<Result<RaftResponse, RaftClientError>>, StorageError<u64>>
    where
        I: IntoIterator<Item = openraft::Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut inner = self.inner.write().await;
        let mut responses = Vec::new();

        for entry in entries {
            inner.last_applied_log_id = Some(entry.log_id);

            match entry.payload {
                EntryPayload::Blank => {
                    responses.push(Ok(RaftResponse::Blank));
                }
                EntryPayload::Membership(membership) => {
                    inner.last_membership = StoredMembership::new(Some(entry.log_id), membership);
                    responses.push(Ok(RaftResponse::Blank));
                }
                EntryPayload::Normal(request) => {
                    let result = apply_single_request(&mut inner.ledger, request);
                    responses.push(result);
                }
            }
        }

        Ok(responses)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        let inner = self.inner.read().await;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        let snapshot_id = format!(
            "{}-{}",
            inner.last_applied_log_id.map(|l| l.index).unwrap_or(0),
            ts
        );

        let meta = SnapshotMeta {
            last_log_id: inner.last_applied_log_id,
            last_membership: inner.last_membership.clone(),
            snapshot_id,
        };

        let snapshot_payload = SnapshotPayload {
            scale: inner.ledger.scale(),
            journal: inner.ledger.journal().to_vec(),
        };

        let payload = postcard::to_allocvec(&snapshot_payload).unwrap_or_default();

        LedgerSnapshotBuilder { meta, payload }
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let data = snapshot.get_ref();
        let payload: SnapshotPayload = postcard::from_bytes(data).map_err(|e| {
            StorageIOError::read_snapshot(
                meta.signature().into(),
                AnyError::error(format!("failed to deserialize snapshot payload: {e}")),
            )
        })?;

        let ledger = Ledger::replay(payload.scale, &payload.journal).map_err(|e| {
            StorageIOError::read_snapshot(
                meta.signature().into(),
                AnyError::error(format!("failed to replay snapshot journal: {e}")),
            )
        })?;

        let mut inner = self.inner.write().await;
        inner.ledger = ledger;
        inner.last_applied_log_id = meta.last_log_id;
        inner.last_membership = meta.last_membership.clone();
        inner.current_snapshot = Some(Snapshot {
            meta: meta.clone(),
            snapshot,
        });

        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok(inner.current_snapshot.clone())
    }
}

/// Applies a single [`RaftRequest`] to the underlying [`Ledger`].
fn apply_single_request(
    ledger: &mut Ledger,
    request: RaftRequest,
) -> Result<RaftResponse, RaftClientError> {
    match request {
        RaftRequest::CreateAccount {
            id,
            account_type,
            flags,
            scale,
            timestamp,
        } => {
            let op = BatchOp::CreateAccount {
                id,
                account_type,
                flags,
                scale,
                timestamp,
            };
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::AccountCreated(id))
        }
        RaftRequest::CreateTransfer(transfer) => {
            let id = transfer.id();
            let op = BatchOp::Transfer(transfer);
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::TransferCreated(id))
        }
        RaftRequest::CreatePending(transfer) => {
            let id = transfer.id();
            let op = BatchOp::Pending(transfer);
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::PendingCreated(id))
        }
        RaftRequest::PostPending {
            pending_id,
            post_transfer_id,
            amount,
            timestamp,
        } => {
            let op = BatchOp::PostPending {
                pending_id,
                post_transfer_id,
                amount,
                timestamp,
            };
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::PendingPosted(post_transfer_id))
        }
        RaftRequest::VoidPending {
            pending_id,
            timestamp,
        } => {
            let op = BatchOp::VoidPending {
                pending_id,
                timestamp,
            };
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::PendingVoided(pending_id))
        }
        RaftRequest::CloseAccount { id, timestamp } => {
            let op = BatchOp::CloseAccount { id, timestamp };
            let events = ledger.prepare_batch(&[op])?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::AccountClosed(id))
        }
        RaftRequest::ApplyBatch(ops) => {
            let events = ledger.prepare_batch(&ops)?;
            ledger.commit_events(&events)?;
            Ok(RaftResponse::BatchApplied(events))
        }
    }
}
