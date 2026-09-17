//! Append-only write-ahead log with crash-safe recovery.
//!
//! The WAL persists framed records, truncates torn tails on reopen, and exposes
//! a single-writer contract for durability and replay. With synchronous append
//! enabled, an acknowledged record has crossed the filesystem sync boundary.
//! Higher layers serialize their own payloads and rebuild state by replaying the
//! verified prefix.

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
