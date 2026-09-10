//! In-memory and WAL-backed log storage implementations for Raft consensus.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogFlushed, LogState, RaftLogReader, RaftLogStorage};
use openraft::{Entry, LogId, OptionalSend, StorageError, Vote};
use tokio::sync::RwLock;

use crate::types::TypeConfig;

#[derive(Debug, Default)]
struct MemLogStoreInner {
    vote: Option<Vote<u64>>,
    committed: Option<LogId<u64>>,
    last_purged_log_id: Option<LogId<u64>>,
    entries: BTreeMap<u64, Entry<TypeConfig>>,
}

/// In-memory implementation of [`RaftLogStorage`] for fast deterministic testing and cluster operation.
#[derive(Clone, Debug, Default)]
pub struct MemLogStore {
    inner: Arc<RwLock<MemLogStoreInner>>,
}

impl MemLogStore {
    /// Creates a new empty [`MemLogStore`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MemLogStoreInner::default())),
        }
    }
}

impl RaftLogReader<TypeConfig> for MemLogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        let mut entries = Vec::new();
        for (_, entry) in inner.entries.range(range) {
            entries.push(entry.clone());
        }
        Ok(entries)
    }
}

impl RaftLogStorage<TypeConfig> for MemLogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<u64>> {
        let inner = self.inner.read().await;
        let last_purged_log_id = inner.last_purged_log_id;
        let last_log_id = inner
            .entries
            .values()
            .last()
            .map(|e| e.log_id)
            .or(last_purged_log_id);

        Ok(LogState {
            last_purged_log_id,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.vote = Some(*vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok(inner.vote)
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<u64>>,
    ) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.committed = committed;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok(inner.committed)
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut inner = self.inner.write().await;
        for entry in entries {
            inner.entries.insert(entry.log_id.index, entry);
        }
        // Notify Raft that write was successfully flushed
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        // Drop any uncommitted entries at and after log_id.index
        inner.entries.split_off(&log_id.index);
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.last_purged_log_id = Some(log_id);
        inner.entries.retain(|&idx, _| idx > log_id.index);
        Ok(())
    }
}
