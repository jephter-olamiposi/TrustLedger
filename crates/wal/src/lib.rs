//! Append-only, checksummed write-ahead log.
//!
//! # How it works
//!
//! A [`Wal`] is one append-only file with one writer. Each append is a single
//! framed record: a fixed 18-byte header (magic, schema version, flags, batch
//! `seq`, payload length), the payload, and a CRC32C checksum over the header
//! and payload (see [`crate::record`]).
//!
//! - `append` only returns after the frame is `fsync`ed (unless
//!   [`WalOptions::sync_per_append`] is off for tests), so an acknowledged seq
//!   is already on disk.
//! - `open` scans the file from the start. Everything after the first broken
//!   frame (bad checksum, bad framing, or a partial write) is treated as a
//!   torn crash tail and removed. [`Recovery`] describes what was kept.
//! - A valid frame with the wrong `seq` is a hard error
//!   ([`WalError::SeqMismatch`]): only another writer could cause that, and we
//!   do not guess at the log's contents.
//! - [`SnapshotFile`] saves application state up to some `seq` using
//!   `tmp + fsync + rename`, so recovery can skip old records. It is only an
//!   optimization: a missing or corrupt snapshot means a full replay, never
//!   wrong state.
//!
//! The wal stores opaque bytes and knows nothing about ledger events.
//! TrustLedger encodes `Vec<ledger_core::LedgerEvent>` batches with `postcard`
//! on the ledger side and rebuilds state with `Ledger::replay`.
//!
//! # Single-writer contract
//!
//! One process owns a wal at a time. Appends take `&mut self`, so concurrent
//! writers in one process are a borrow error; sharing a file across processes
//! is the consensus layer's job.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod error;
pub mod record;
pub mod snapshot;
pub mod wal;

pub use error::{HeaderError, WalError};
pub use record::{Record, DEFAULT_MAX_PAYLOAD_LEN};
pub use snapshot::{Snapshot, SnapshotFile};
pub use wal::{RecordStream, Recovery, Wal, WalOptions};
