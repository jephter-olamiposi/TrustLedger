//! Errors returned by the write-ahead log.

use thiserror::Error;

/// Errors returned by the write-ahead log and its snapshots.
#[derive(Debug, Error)]
pub enum WalError {
    /// An underlying filesystem operation failed.
    #[error("wal io failure during {op} on {path}: {source}")]
    Io {
        /// The operation that failed (e.g. "open", "write_all", "sync_data", "set_len").
        op: &'static str,
        /// Path of the file involved.
        path: String,
        /// The underlying io error.
        #[source]
        source: std::io::Error,
    },

    /// Another process holds an exclusive lock on this wal file.
    #[error("wal file {path} is locked by another process: {source}")]
    Locked {
        /// Path of the file involved.
        path: String,
        /// The lock acquisition error.
        #[source]
        source: std::io::Error,
    },

    /// A record payload exceeds the configured maximum frame length.
    #[error("payload of {actual} bytes exceeds the maximum record length of {max} bytes")]
    PayloadTooLarge {
        /// Actual payload length.
        actual: usize,
        /// Configured maximum payload length.
        max: usize,
    },

    /// A frame on disk violates the record layout at the given byte offset.
    #[error("record header at byte {offset} violates the wal layout: {kind}")]
    InvalidHeader {
        /// File offset of the offending header.
        offset: u64,
        /// Why the header is invalid.
        kind: HeaderError,
    },

    /// A record's seq is not the one the recovery scan expected, meaning the
    /// log was appended by another writer or rewritten out of order.
    #[error("record at byte {offset} has seq {actual} but {expected} was expected")]
    SeqMismatch {
        /// File offset of the offending record.
        offset: u64,
        /// Seq recorded on disk.
        actual: u64,
        /// Seq the recovery scan expected.
        expected: u64,
    },

    /// A full frame's checksum did not match, indicating torn or corrupted
    /// data at that offset.
    #[error("checksum mismatch at byte {offset}")]
    ChecksumMismatch {
        /// File offset of the offending record.
        offset: u64,
    },

    /// The log file changed while an open handle was in use, so the committed
    /// prefix can no longer be trusted.
    #[error("the wal file changed while open; re-open to recover")]
    ChangedWhileOpen,
}

/// Reason a record header was rejected during recovery.
#[derive(Debug, Error)]
pub enum HeaderError {
    /// The magic bytes are not those of a wal frame.
    #[error("frame magic does not match the wal layout")]
    BadMagic,
    /// The schema version is not supported by this build.
    #[error("unsupported on-disk schema version {0}")]
    UnsupportedVersion(u8),
    /// The declared payload length exceeds the configured maximum.
    #[error("declared payload length exceeds the configured maximum")]
    PayloadTooLarge,
    /// The frame extends past the end of the file (a torn write).
    #[error("frame extends past the end of the file")]
    TruncatedFrame,
}

impl WalError {
    /// The file path associated with the error, if any.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Io { path, .. } | Self::Locked { path, .. } => Some(path),
            _ => None,
        }
    }
}
