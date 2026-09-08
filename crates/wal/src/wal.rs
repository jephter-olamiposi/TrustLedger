//! Append-only, checksummed write-ahead log.
//!
//! One writer. Each batch is one framed record (see [`crate::record`]);
//! `append` returns only after the frame is `fsync`ed, so an acknowledged seq
//! survives a crash. Opening a [`Wal`] replays every frame from the start,
//! discards anything after the first broken frame (a crash tail), and reports
//! the recovered state so the caller can rebuild application state.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use crate::error::WalError;
use crate::record::{self, Record};

/// Outcome of opening a wal: where the caller resumes and what was discarded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// First unused batch number; the seq `append` will assign next.
    pub next_seq: u64,
    /// Bytes of unverified tail removed from the log.
    pub truncated_bytes: u64,
    /// Number of intact records replayed from the committed prefix.
    pub verified_records: u64,
}

/// Tunables for a [`Wal`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WalOptions {
    /// Max payload bytes per record; must fit the largest batch this node
    /// ever commits.
    pub max_record_payload_len: usize,
    /// `fsync` every append before returning. Setting this to `false` is only
    /// for tests and benchmarks — never for production writes.
    pub sync_per_append: bool,
}

impl Default for WalOptions {
    fn default() -> Self {
        Self {
            max_record_payload_len: record::DEFAULT_MAX_PAYLOAD_LEN,
            sync_per_append: true,
        }
    }
}

/// Write-ahead log over a single append-only file.
///
/// Not `Sync`: append and the recovery scan share the file's seek state, so a
/// [`Wal`] must be owned (or mutexed) by the committing thread.
pub struct Wal {
    path: PathBuf,
    file: File,
    next_seq: u64,
    committed_end: u64,
    options: WalOptions,
    encode_buf: Vec<u8>,
}

impl Wal {
    /// Open (creating if needed) and recover the log at `path`.
    ///
    /// Records with a valid frame and a contiguous `seq` starting at 0 are
    /// kept; anything after the first broken frame is truncated. A valid
    /// frame with the wrong `seq` is an error: another writer touched the log,
    /// so no prefix can be trusted.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Locked`] if another process holds an open lock on the log.
    /// Returns [`WalError::Io`] on filesystem failure and
    /// [`WalError::SeqMismatch`] if the on-disk `seq` does not follow the
    /// expected order. Checksum failures do not fail the open — they truncate
    /// the tail instead.
    pub fn open(path: impl AsRef<Path>, options: WalOptions) -> Result<(Self, Recovery), WalError> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(map_io("open", &path))?;

        file.try_lock_exclusive().map_err(|err| WalError::Locked {
            path: path.display().to_string(),
            source: err,
        })?;

        let mut next_seq = 0u64;
        let mut verified_records = 0u64;
        let mut verified_end = 0u64;

        {
            let mut reader = BufReader::new(&mut file);
            loop {
                let offset = verified_end;
                match record::next_record(&mut reader, options.max_record_payload_len, false)
                    .map_err(map_io("read_frame", &path))?
                {
                    record::ScanStep::Record { seq, len, .. } => {
                        if seq != next_seq {
                            return Err(WalError::SeqMismatch {
                                offset,
                                actual: seq,
                                expected: next_seq,
                            });
                        }
                        next_seq += 1;
                        verified_records += 1;
                        verified_end += record::frame_len(len) as u64;
                    }
                    record::ScanStep::End | record::ScanStep::Broken => break,
                }
            }
        }

        let file_len = file.metadata().map_err(map_io("metadata", &path))?.len();
        let truncated_bytes = file_len.saturating_sub(verified_end);
        if truncated_bytes > 0 {
            file.set_len(verified_end)
                .map_err(map_io("set_len", &path))?;
            file.sync_all().map_err(map_io("sync_all", &path))?;
        }
        file.seek(SeekFrom::Start(verified_end))
            .map_err(map_io("seek_end", &path))?;

        let recovery = Recovery {
            next_seq,
            truncated_bytes,
            verified_records,
        };
        let initial_buf_capacity = record::frame_len(options.max_record_payload_len.min(64 * 1024));
        Ok((
            Self {
                path,
                file,
                next_seq,
                committed_end: verified_end,
                options,
                encode_buf: Vec::with_capacity(initial_buf_capacity),
            },
            recovery,
        ))
    }

    /// Append one framed batch and return its seq.
    ///
    /// The frame is `sync_data`-fsynced before this returns (per
    /// [`WalOptions::sync_per_append`]), so an acknowledged seq survives a
    /// crash. `payload` must fit within
    /// [`WalOptions::max_record_payload_len`].
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] on write or fsync failure and
    /// [`WalError::PayloadTooLarge`] if the payload exceeds the configured cap.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, WalError> {
        let seq = self.next_seq;
        record::encode_into(
            seq,
            payload,
            self.options.max_record_payload_len,
            &mut self.encode_buf,
        )?;
        self.file
            .write_all(&self.encode_buf)
            .map_err(map_io("write_all", &self.path))?;
        if self.options.sync_per_append {
            self.file
                .sync_data()
                .map_err(map_io("sync_data", &self.path))?;
        }
        self.committed_end += self.encode_buf.len() as u64;
        self.next_seq = seq + 1;
        Ok(seq)
    }

    /// Replay the committed prefix: every record with seq `0..next_seq`.
    ///
    /// Reads through this handle, so no path is re-opened and the same
    /// streaming scanner as recovery is used. The file position is left at the
    /// end of the log, so the next [`Self::append`] still lands at the end.
    /// If the file changed underneath this handle, this returns
    /// [`WalError::ChangedWhileOpen`] instead of a silently partial or
    /// duplicated history.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::ChangedWhileOpen`] if the file no longer describes
    /// a contiguous commit history and [`WalError::Io`] on read failure.
    pub fn read_records(&mut self) -> Result<Vec<Record>, WalError> {
        let res = (|| {
            self.file
                .seek(SeekFrom::Start(0))
                .map_err(map_io("seek_start", &self.path))?;

            let mut records = Vec::with_capacity(self.next_seq as usize);
            let mut reader = BufReader::new(&mut self.file);
            while records.len() as u64 != self.next_seq {
                match record::next_record(&mut reader, self.options.max_record_payload_len, true)
                    .map_err(map_io("read_record", &self.path))?
                {
                    record::ScanStep::Record { seq, payload, .. } => {
                        let Some(payload) = payload else {
                            return Err(WalError::ChangedWhileOpen);
                        };
                        if seq != records.len() as u64 {
                            return Err(WalError::ChangedWhileOpen);
                        }
                        records.push(Record { seq, payload });
                    }
                    record::ScanStep::End | record::ScanStep::Broken => {
                        return Err(WalError::ChangedWhileOpen);
                    }
                }
            }
            match record::next_record(&mut reader, self.options.max_record_payload_len, false)
                .map_err(map_io("read_record", &self.path))?
            {
                record::ScanStep::End => {}
                record::ScanStep::Record { .. } | record::ScanStep::Broken => {
                    return Err(WalError::ChangedWhileOpen);
                }
            }
            Ok(records)
        })();

        let seek_res = self
            .file
            .seek(SeekFrom::Start(self.committed_end))
            .map_err(map_io("seek_end", &self.path));
        let records = res?;
        seek_res?;
        Ok(records)
    }

    /// Next seq this log will assign.
    #[inline]
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Length of the committed file on disk in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] on filesystem failure.
    pub fn committed_len(&self) -> Result<u64, WalError> {
        self.file
            .metadata()
            .map(|meta| meta.len())
            .map_err(map_io("metadata", &self.path))
    }

    /// The log file path.
    #[inline]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Lift an io error into [`WalError::Io`] with the operation and path attached.
fn map_io(op: &'static str, path: impl AsRef<Path>) -> impl FnOnce(std::io::Error) -> WalError {
    let path = path.as_ref().to_string_lossy().into_owned();
    move |source| WalError::Io { op, path, source }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join(name)
    }

    fn open_empty(path: &Path) -> Wal {
        let (wal, recovery) = Wal::open(path, WalOptions::default()).expect("open empty wal");
        assert_eq!(
            recovery,
            Recovery {
                next_seq: 0,
                truncated_bytes: 0,
                verified_records: 0,
            }
        );
        wal
    }

    #[test]
    fn empty_and_missing_logs_recover_to_zero() {
        let path = temp_path("empty.log");
        let _ = std::fs::remove_file(&path);
        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("open");
        assert_eq!(recovery.next_seq, 0);
        assert_eq!(wal.next_seq(), 0);
        assert_eq!(wal.read_records().unwrap(), vec![]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_assigns_contiguous_seqs() {
        let path = temp_path("seq.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);
        assert_eq!(wal.append(b"first").unwrap(), 0);
        assert_eq!(wal.append(b"second").unwrap(), 1);
        assert_eq!(wal.append(b"third").unwrap(), 2);
        assert_eq!(wal.next_seq(), 3);
        drop(wal);

        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("reopen");
        assert_eq!(
            recovery,
            Recovery {
                next_seq: 3,
                truncated_bytes: 0,
                verified_records: 3,
            }
        );
        assert_eq!(
            wal.read_records().unwrap(),
            vec![
                Record {
                    seq: 0,
                    payload: b"first".to_vec()
                },
                Record {
                    seq: 1,
                    payload: b"second".to_vec()
                },
                Record {
                    seq: 2,
                    payload: b"third".to_vec()
                },
            ]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn append_continues_after_recovery() {
        let path = temp_path("continue.log");
        let _ = std::fs::remove_file(&path);
        {
            let mut wal = open_empty(&path);
            wal.append(b"one").unwrap();
            wal.append(b"two").unwrap();
        }
        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("reopen");
        assert_eq!(recovery.next_seq, 2);
        assert_eq!(wal.append(b"three").unwrap(), 2);
        assert_eq!(wal.next_seq(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_records_leaves_the_handle_at_the_end_for_appends() {
        let path = temp_path("readappend.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);
        wal.append(b"first").unwrap();
        wal.append(b"second").unwrap();

        assert_eq!(wal.read_records().unwrap().len(), 2);

        wal.append(b"third").unwrap();
        assert_eq!(wal.next_seq(), 3);
        let replayed = wal.read_records().unwrap();
        assert_eq!(
            replayed,
            vec![
                Record {
                    seq: 0,
                    payload: b"first".to_vec()
                },
                Record {
                    seq: 1,
                    payload: b"second".to_vec()
                },
                Record {
                    seq: 2,
                    payload: b"third".to_vec()
                },
            ]
        );
        drop(wal);

        let (mut reopened, recovery) = Wal::open(&path, WalOptions::default()).expect("reopen");
        assert_eq!(recovery.next_seq, 3);
        assert_eq!(reopened.read_records().unwrap(), replayed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reopen_drops_garbage_tail() {
        let path = temp_path("garbage.log");
        let _ = std::fs::remove_file(&path);
        {
            let mut wal = open_empty(&path);
            wal.append(b"good-0").unwrap();
            wal.append(b"good-1").unwrap();
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append garbage");
        file.write_all(b"this is not a frame")
            .expect("write garbage");
        file.sync_all().expect("sync garbage");
        drop(file);

        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("recover");
        assert_eq!(recovery.next_seq, 2);
        assert!(recovery.truncated_bytes > 0);
        assert_eq!(
            wal.read_records().unwrap(),
            vec![
                Record {
                    seq: 0,
                    payload: b"good-0".to_vec()
                },
                Record {
                    seq: 1,
                    payload: b"good-1".to_vec()
                },
            ]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reopen_drops_half_written_final_record() {
        let path = temp_path("torn.log");
        let _ = std::fs::remove_file(&path);
        {
            let mut wal = open_empty(&path);
            wal.append(b"durable").unwrap();
        }
        let frame = record::encode(1, b"never-fully-written", usize::MAX).unwrap();
        let torn = &frame[..record::HEADER_LEN + 3];
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open for torn write");
        file.write_all(torn).expect("write head");
        file.sync_all().expect("sync torn");
        drop(file);

        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("recover");
        assert_eq!(recovery.next_seq, 1);
        assert!(recovery.truncated_bytes >= 3);
        assert_eq!(
            wal.read_records().unwrap(),
            vec![Record {
                seq: 0,
                payload: b"durable".to_vec()
            }]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reopen_truncates_on_corrupted_payload() {
        let path = temp_path("corrupt.log");
        let _ = std::fs::remove_file(&path);
        {
            let mut wal = open_empty(&path);
            wal.append(b"intact").unwrap();
        }
        let mut bytes = std::fs::read(&path).expect("read wal");
        let last = bytes.len() - record::TRAILER_LEN;
        bytes[last.checked_sub(1).expect("payload byte")] ^= 0x40;
        std::fs::write(&path, &bytes).expect("rewrite corrupted");

        let (mut wal, recovery) = Wal::open(&path, WalOptions::default()).expect("recover");
        assert_eq!(recovery.next_seq, 0);
        assert_eq!(wal.read_records().unwrap(), vec![]);
        assert!(wal.committed_len().unwrap() < bytes.len() as u64);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn seq_gap_is_a_hard_error_not_a_truncation() {
        let path = temp_path("gap.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);
        wal.append(b"seq-0").unwrap();
        drop(wal);

        let frame = record::encode(5, b"skipped", usize::MAX).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open to inject gap");
        file.write_all(&frame).expect("write gap record");
        file.sync_all().expect("sync gap");
        drop(file);

        assert!(matches!(
            Wal::open(&path, WalOptions::default()),
            Err(WalError::SeqMismatch {
                actual: 5,
                expected: 1,
                ..
            })
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oversized_payload_is_rejected_on_append() {
        let path = temp_path("oversize.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);
        let payload = vec![0u8; record::DEFAULT_MAX_PAYLOAD_LEN + 1];
        assert!(matches!(
            wal.append(&payload),
            Err(WalError::PayloadTooLarge { .. })
        ));
        assert_eq!(wal.next_seq(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn concurrent_open_fails_with_locked_error() {
        let path = temp_path("locked.log");
        let _ = std::fs::remove_file(&path);
        let _wal = open_empty(&path);
        assert!(matches!(
            Wal::open(&path, WalOptions::default()),
            Err(WalError::Locked { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }
}
