//! Checkpoint snapshots that let recovery skip most of the wal.
//!
//! A snapshot stores application state (in TrustLedger, the encoded journal
//! prefix) plus the highest batch `seq` it covers. It is written with
//! `tmp → write → fsync → rename`, so a crash before the rename leaves the
//! old snapshot (or none) intact and the wal stays the source of truth. A
//! missing, stale, or corrupt snapshot is not an error: recovery falls back
//! to replaying the wal from seq 0.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::WalError;
use crate::record::{self, HEADER_LEN, VERSION};

/// Magic identifying a checkpoint snapshot frame.
const SNAP_MAGIC: u32 = u32::from_le_bytes(*b"LTSN");

/// A validated snapshot read back from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Wal seq up to which this snapshot has state. Records with `seq >
    /// last_seq` must still be replayed.
    pub last_seq: u64,
    /// Application payload (the encoded journal prefix).
    pub payload: Vec<u8>,
}

/// A checkpoint snapshot stored at a fixed path.
pub struct SnapshotFile {
    path: PathBuf,
    max_payload_len: usize,
}

impl SnapshotFile {
    /// Point a snapshot store at `path`.
    #[must_use]
    pub fn new(path: impl AsRef<Path>, max_payload_len: usize) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            max_payload_len,
        }
    }

    /// Write `payload` as the snapshot for all records up to and including
    /// `last_seq`.
    ///
    /// Uses the same checksummed framing as the wal (its own magic), so a torn
    /// write reads as an invalid snapshot rather than a plausible one.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] on filesystem failure and
    /// [`WalError::PayloadTooLarge`] if the payload exceeds the configured
    /// cap.
    pub fn save(&self, last_seq: u64, payload: &[u8]) -> Result<(), WalError> {
        if payload.len() > self.max_payload_len {
            return Err(WalError::PayloadTooLarge {
                actual: payload.len(),
                max: self.max_payload_len,
            });
        }

        let parent = match self.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };

        // Create a unique temporary file in the same directory to guarantee atomic rename
        // on the same filesystem. Tempfile's drop handler automatically cleans up partial
        // leftovers if writing, syncing, or persisting fails.
        let mut tmp = tempfile::Builder::new()
            .prefix(".snap-")
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(map_io("open_tmp", &self.path))?;

        tmp.write_all(&record::encode_frame(
            SNAP_MAGIC,
            VERSION,
            last_seq,
            payload,
            self.max_payload_len,
        )?)
        .map_err(map_io("write_tmp", &self.path))?;

        tmp.as_file()
            .sync_data()
            .map_err(map_io("sync_tmp", &self.path))?;

        tmp.persist(&self.path)
            .map_err(|err| map_io("persist_snapshot", &self.path)(err.error))?;

        // Sync parent directory to persist the directory entry rename.
        // On Unix systems where directory fsync is supported (Linux), this flushes the rename.
        // On platforms/filesystems where directory fsync is unsupported (macOS/Darwin where
        // fsync on directory descriptors returns EINVAL/ENOTSUP, or Windows), we safely ignore
        // errors to avoid failing an already durable snapshot write.
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }

        Ok(())
    }

    /// Load and validate the snapshot.
    ///
    /// Returns `Ok(None)` if the snapshot is missing or fails validation; the
    /// caller then replays the wal from the start. Corrupt snapshots are not
    /// errors because the wal, not the snapshot, is the source of truth.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] on filesystem failure.
    pub fn load(&self) -> Result<Option<Snapshot>, WalError> {
        let mut bytes = Vec::new();
        match OpenOptions::new().read(true).open(&self.path) {
            Ok(mut file) => {
                file.read_to_end(&mut bytes)
                    .map_err(map_io("read_snapshot", &self.path))?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(map_io("open_snapshot", &self.path)(err)),
        }

        let (last_seq, payload_len) =
            match record::decode_frame_with(SNAP_MAGIC, VERSION, &bytes, self.max_payload_len) {
                Ok(parsed) => parsed,
                Err(_) => return Ok(None),
            };

        Ok(Some(Snapshot {
            last_seq,
            payload: bytes[HEADER_LEN..HEADER_LEN + payload_len].to_vec(),
        }))
    }

    /// The snapshot file path.
    #[inline]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn map_io(op: &'static str, path: impl AsRef<Path>) -> impl FnOnce(std::io::Error) -> WalError {
    let path = path.as_ref().to_string_lossy().into_owned();
    move |source| WalError::Io { op, path, source }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wal-snap-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn roundtrip_snapshot() {
        let dir = tmp_dir("roundtrip");
        let snap = SnapshotFile::new(dir.join("snap.dat"), record::DEFAULT_MAX_PAYLOAD_LEN);
        assert_eq!(snap.load().unwrap(), None);

        snap.save(41, b"encoded-journal-prefix").unwrap();
        assert_eq!(
            snap.load().unwrap(),
            Some(Snapshot {
                last_seq: 41,
                payload: b"encoded-journal-prefix".to_vec(),
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_tmp_never_shadows_a_valid_snapshot() {
        let dir = tmp_dir("tmp-cleanup");
        let snap = SnapshotFile::new(dir.join("snap.dat"), record::DEFAULT_MAX_PAYLOAD_LEN);
        snap.save(3, b"state").unwrap();

        let tmp = dir.join("snap.tmp");
        std::fs::write(&tmp, b"half-written-garbage").unwrap();

        assert_eq!(
            snap.load().unwrap(),
            Some(Snapshot {
                last_seq: 3,
                payload: b"state".to_vec(),
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_snapshot_falls_back_to_none() {
        let dir = tmp_dir("corrupt");
        let snap = SnapshotFile::new(dir.join("snap.dat"), record::DEFAULT_MAX_PAYLOAD_LEN);
        let path = dir.join("snap.dat");
        std::fs::write(&path, b"definitely not a snapshot frame").unwrap();

        assert_eq!(snap.load().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bit_rot_in_snapshot_falls_back_to_none() {
        let dir = tmp_dir("bitrot");
        let snap = SnapshotFile::new(dir.join("snap.dat"), record::DEFAULT_MAX_PAYLOAD_LEN);
        snap.save(9, b"trusted").unwrap();

        let mut bytes = std::fs::read(snap.path()).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x01;
        std::fs::write(snap.path(), &bytes).unwrap();

        assert_eq!(snap.load().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_snapshot_is_rejected() {
        let dir = tmp_dir("oversize");
        let snap = SnapshotFile::new(dir.join("snap.dat"), 16);
        assert!(snap.save(0, &[0u8; 17]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
