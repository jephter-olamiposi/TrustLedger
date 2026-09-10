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
        let range = self.append_batch(&[payload])?;
        Ok(range.start)
    }

    /// Append a batch of record payloads sequentially, committing them with a single `sync_data` call.
    ///
    /// The entire batch is validated and encoded into an in-memory buffer before any bytes are
    /// written to disk. If any individual payload exceeds the configured maximum size, the function
    /// returns an error immediately without modifying the write buffer or disk state.
    ///
    /// If `WalOptions::sync_per_append` is enabled, all records in the batch are committed with
    /// a single fsync, eliminating the physical per-transfer fsync bottleneck.
    ///
    /// Returns the contiguous sequence range `start_seq..end_seq` assigned to the batch.
    /// If `payloads` is empty, returns `self.next_seq..self.next_seq` without performing disk I/O.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::PayloadTooLarge`] if any payload exceeds [`WalOptions::max_record_payload_len`].
    /// Returns [`WalError::Io`] on write or fsync failure.
    pub fn append_batch<T: AsRef<[u8]>>(
        &mut self,
        payloads: &[T],
    ) -> Result<std::ops::Range<u64>, WalError> {
        if payloads.is_empty() {
            return Ok(self.next_seq..self.next_seq);
        }

        // Validate all payload lengths before touching the buffer or disk to ensure failure leaves
        // the log completely unmodified.
        let mut total_len = 0usize;
        for payload in payloads {
            let len = payload.as_ref().len();
            if len > self.options.max_record_payload_len {
                return Err(WalError::PayloadTooLarge {
                    actual: len,
                    max: self.options.max_record_payload_len,
                });
            }
            total_len =
                total_len
                    .checked_add(record::frame_len(len))
                    .ok_or(WalError::PayloadTooLarge {
                        actual: usize::MAX,
                        max: self.options.max_record_payload_len,
                    })?;
        }

        self.encode_buf.clear();
        if self.encode_buf.capacity() < total_len {
            self.encode_buf
                .reserve(total_len - self.encode_buf.capacity());
        }

        let start_seq = self.next_seq;
        let mut cur_seq = start_seq;

        for payload in payloads {
            record::encode_append(
                cur_seq,
                payload.as_ref(),
                self.options.max_record_payload_len,
                &mut self.encode_buf,
            )?;
            cur_seq += 1;
        }

        if let Err(err) = self
            .file
            .write_all(&self.encode_buf)
            .map_err(map_io("write_all", &self.path))
        {
            let _ = self.file.set_len(self.committed_end);
            let _ = self.file.seek(SeekFrom::Start(self.committed_end));
            return Err(err);
        }

        if self.options.sync_per_append {
            if let Err(err) = self
                .file
                .sync_data()
                .map_err(map_io("sync_data", &self.path))
            {
                let _ = self.file.set_len(self.committed_end);
                let _ = self.file.seek(SeekFrom::Start(self.committed_end));
                return Err(err);
            }
        }

        self.committed_end += self.encode_buf.len() as u64;
        self.next_seq = cur_seq;
        Ok(start_seq..cur_seq)
    }

    /// Create a streaming iterator over the committed records in the log.
    ///
    /// Reads records sequentially from the start of the log in constant memory ($O(1)$).
    /// Holds an exclusive mutable borrow of `self` for the duration of streaming, ensuring
    /// no concurrent writes can invalidate the read position. When iteration completes or
    /// the stream is dropped, the file cursor is restored to the end of the committed log.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] if seeking to the beginning of the log fails.
    pub fn stream_records(&mut self) -> Result<RecordStream<'_>, WalError> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(map_io("seek_start", &self.path))?;

        Ok(RecordStream {
            reader: BufReader::new(&mut self.file),
            path: &self.path,
            max_payload_len: self.options.max_record_payload_len,
            expected_next_seq: self.next_seq,
            committed_end: self.committed_end,
            current_seq: 0,
            seek_restored: false,
            finished: false,
        })
    }

    /// Replay the committed prefix: every record with seq `0..next_seq`.
    ///
    /// Reads through a streaming reader, bounding initial memory allocation to avoid OOM
    /// hazards on large or corrupted logs. The file position is left at the end of the log,
    /// so subsequent appends land safely at the end.
    ///
    /// If the file changed underneath this handle, this returns
    /// [`WalError::ChangedWhileOpen`] instead of a silently partial or
    /// duplicated history.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::ChangedWhileOpen`] if the file no longer describes
    /// a contiguous commit history and [`WalError::Io`] on read or seek failure.
    pub fn read_records(&mut self) -> Result<Vec<Record>, WalError> {
        // Safely bound the initial capacity allocation to prevent OOM on massive logs.
        let initial_cap = (self.next_seq as usize).min(1024);
        let stream = self.stream_records()?;
        let mut records = Vec::with_capacity(initial_cap);
        for item in stream {
            records.push(item?);
        }
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

/// A streaming record iterator over the write-ahead log.
///
/// Yields records sequentially from the start of the log in constant memory ($O(1)$).
/// Exclusively borrows the [`Wal`] for the lifetime `'a` of the stream.
/// When iteration finishes or the stream is dropped, the file cursor is restored
/// to the end of the committed log.
#[derive(Debug)]
pub struct RecordStream<'a> {
    reader: BufReader<&'a mut File>,
    path: &'a Path,
    max_payload_len: usize,
    expected_next_seq: u64,
    committed_end: u64,
    current_seq: u64,
    seek_restored: bool,
    finished: bool,
}

impl<'a> RecordStream<'a> {
    fn restore_cursor(&mut self) -> Result<(), WalError> {
        if !self.seek_restored {
            self.reader
                .seek(SeekFrom::Start(self.committed_end))
                .map_err(map_io("seek_end", self.path))?;
            self.seek_restored = true;
        }
        Ok(())
    }

    /// Explicitly close the stream, restoring the file cursor to the end of the committed log.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] if seeking back to the end of the log fails.
    pub fn close(mut self) -> Result<(), WalError> {
        self.restore_cursor()
    }
}

impl<'a> Drop for RecordStream<'a> {
    fn drop(&mut self) {
        let _ = self.restore_cursor();
    }
}

impl<'a> Iterator for RecordStream<'a> {
    type Item = Result<Record, WalError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        if self.current_seq < self.expected_next_seq {
            match record::next_record(&mut self.reader, self.max_payload_len, true)
                .map_err(map_io("read_record", self.path))
            {
                Ok(record::ScanStep::Record { seq, payload, .. }) => {
                    let Some(payload) = payload else {
                        self.finished = true;
                        return Some(Err(WalError::ChangedWhileOpen));
                    };
                    if seq != self.current_seq {
                        self.finished = true;
                        return Some(Err(WalError::ChangedWhileOpen));
                    }
                    self.current_seq += 1;
                    Some(Ok(Record { seq, payload }))
                }
                Ok(record::ScanStep::End | record::ScanStep::Broken) => {
                    self.finished = true;
                    Some(Err(WalError::ChangedWhileOpen))
                }
                Err(err) => {
                    self.finished = true;
                    Some(Err(err))
                }
            }
        } else {
            self.finished = true;
            match record::next_record(&mut self.reader, self.max_payload_len, false)
                .map_err(map_io("read_record", self.path))
            {
                Ok(record::ScanStep::End) => {
                    if let Err(err) = self.restore_cursor() {
                        return Some(Err(err));
                    }
                    None
                }
                Ok(record::ScanStep::Record { .. } | record::ScanStep::Broken) => {
                    Some(Err(WalError::ChangedWhileOpen))
                }
                Err(err) => Some(Err(err)),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            let remaining = self.expected_next_seq.saturating_sub(self.current_seq);
            let remaining_usize = usize::try_from(remaining).unwrap_or(usize::MAX);
            (remaining_usize, Some(remaining_usize))
        }
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

    #[test]
    fn append_batch_assigns_contiguous_seqs_and_single_sync() {
        let path = temp_path("batch.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);

        let payloads = vec![
            b"batch-0".to_vec(),
            b"batch-1".to_vec(),
            b"batch-2".to_vec(),
        ];
        let range = wal.append_batch(&payloads).expect("append batch");
        assert_eq!(range, 0..3);
        assert_eq!(wal.next_seq(), 3);

        let records = wal.read_records().expect("read records");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].payload, b"batch-0");
        assert_eq!(records[1].payload, b"batch-1");
        assert_eq!(records[2].payload, b"batch-2");

        let empty_range = wal.append_batch::<&[u8]>(&[]).expect("empty batch");
        assert_eq!(empty_range, 3..3);
        assert_eq!(wal.next_seq(), 3);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stream_records_early_drop_restores_cursor_for_subsequent_appends() {
        let path = temp_path("stream_drop.log");
        let _ = std::fs::remove_file(&path);
        let mut wal = open_empty(&path);

        for i in 0..5 {
            wal.append(format!("rec-{i}").as_bytes()).unwrap();
        }

        {
            let mut stream = wal.stream_records().unwrap();
            let first = stream.next().unwrap().unwrap();
            assert_eq!(first.seq, 0);
            assert_eq!(first.payload, b"rec-0");
            // Drop stream early halfway through iteration
        }

        wal.append(b"rec-5").expect("append after dropped stream");
        assert_eq!(wal.next_seq(), 6);

        let all = wal.read_records().unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(all[5].seq, 5);
        assert_eq!(all[5].payload, b"rec-5");

        let _ = std::fs::remove_file(&path);
    }
}
