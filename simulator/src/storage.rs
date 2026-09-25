//! Simulated fault-injected durable storage backing the state machine and WAL.
//!
//! Models OS page caching, explicit fsync boundaries, un-synced data loss on crash,
//! and torn-write byte truncation to rigorously exercise crash recovery.

use std::fmt;

use crate::rng::SimRng;

/// In-memory fault-injected storage simulating physical disk blocks and torn writes.
#[derive(Clone, Default)]
pub struct SimDisk {
    /// Bytes durably persisted to simulated non-volatile storage via `sync()`.
    flushed: Vec<u8>,
    /// Bytes written to volatile OS page cache buffers pending fsync.
    buffered: Vec<u8>,
    /// Counter tracking write operations performed.
    writes_count: u64,
    /// Counter tracking fsync operations performed.
    syncs_count: u64,
    /// Counter tracking crash events survived.
    crashes_count: u64,
}

impl fmt::Debug for SimDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimDisk")
            .field("flushed_bytes", &self.flushed.len())
            .field("buffered_bytes", &self.buffered.len())
            .field("writes_count", &self.writes_count)
            .field("syncs_count", &self.syncs_count)
            .field("crashes_count", &self.crashes_count)
            .finish()
    }
}

impl SimDisk {
    /// Creates a new empty simulated disk.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flushed: Vec::new(),
            buffered: Vec::new(),
            writes_count: 0,
            syncs_count: 0,
            crashes_count: 0,
        }
    }

    /// Appends data to volatile write buffers (simulating standard OS write/page cache).
    pub fn write(&mut self, data: &[u8]) {
        self.buffered.extend_from_slice(data);
        self.writes_count = self.writes_count.saturating_add(1);
    }

    /// Flushes all volatile buffered data into durable non-volatile storage (simulating fsync).
    pub fn sync(&mut self) {
        self.flushed.append(&mut self.buffered);
        self.syncs_count = self.syncs_count.saturating_add(1);
    }

    /// Simulates sudden power loss or kernel crash.
    ///
    /// If `torn_write` is true and buffered data exists, a partial prefix of the buffered bytes
    /// is written to disk before power cuts, simulating a torn frame that violates CRC32C checks.
    /// Otherwise, all un-synced buffered data is completely discarded.
    pub fn crash(&mut self, rng: &mut SimRng, torn_write: bool) {
        self.crashes_count = self.crashes_count.saturating_add(1);

        if torn_write {
            if !self.buffered.is_empty() {
                // Persist a random non-empty prefix of the uncommitted buffer, truncating the frame.
                let max_range = (self.buffered.len() as u64).max(2);
                let torn_len = (rng.gen_range(1..max_range) as usize).min(self.buffered.len());
                if let Some(prefix) = self.buffered.get(..torn_len) {
                    self.flushed.extend_from_slice(prefix);
                }
            } else if !self.flushed.is_empty() {
                // Simulate an in-flight sector torn write appending invalid trailing bytes
                let torn_len = rng.gen_range(1..=15) as usize;
                let fragment: Vec<u8> = (0..torn_len)
                    .map(|i| (i as u8).wrapping_add(0xEE))
                    .collect();
                self.flushed.extend_from_slice(&fragment);
            }
        }

        // Invariant: all volatile memory in OS cache is vaporized on sudden power cut.
        self.buffered.clear();
    }

    /// Truncates durably persisted storage to `len` bytes, discarding any torn write tail.
    pub fn truncate_durable(&mut self, len: usize) {
        self.flushed.truncate(len);
    }

    /// Reads all durably flushed bytes from the simulated disk.
    #[must_use]
    pub fn read_durable(&self) -> &[u8] {
        &self.flushed
    }

    /// Returns the number of durably persisted bytes on disk.
    #[must_use]
    pub fn durable_len(&self) -> usize {
        self.flushed.len()
    }

    /// Returns the total write calls made.
    #[must_use]
    pub const fn writes_count(&self) -> u64 {
        self.writes_count
    }

    /// Returns the total sync calls made.
    #[must_use]
    pub const fn syncs_count(&self) -> u64 {
        self.syncs_count
    }

    /// Returns the total crashes survived.
    #[must_use]
    pub const fn crashes_count(&self) -> u64 {
        self.crashes_count
    }
}
