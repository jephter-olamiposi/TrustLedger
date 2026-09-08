//! Deterministic crash simulation for the write-ahead log.
//!
//! Models a crash as: the disk retained exactly the first `offset` bytes of
//! the committed file, and nothing after. For every byte boundary of a
//! committed log the recovery must reproduce exactly the acknowledged prefix —
//! every record whose frame ends at or before `offset`, and no more.

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use wal::{record, Record, Recovery, Wal, WalOptions};

/// Commit `batches` deterministic records and return them along with the
/// cumulative byte length after each append (the durable prefix boundary).
fn build_wal(wal_path: &Path, batches: usize) -> (TempDir, Vec<Vec<u8>>, Vec<usize>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let wal_path = tmp.path().join(wal_path);

    let opts = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&wal_path, opts).expect("open wal");
    assert_eq!(
        recovery,
        Recovery {
            next_seq: 0,
            truncated_bytes: 0,
            verified_records: 0,
        }
    );

    let mut payloads = Vec::new();
    let mut ends = Vec::new();
    let mut seen = 0usize;
    for i in 0..batches {
        let payload: Vec<u8> = (0..((i % 7) + 1) * 16 + i % 5)
            .map(|k| ((i * 31 + k) % 251) as u8)
            .collect();
        wal.append(&payload).expect("append");
        seen += record::frame_len(payload.len());
        ends.push(seen);
        payloads.push(payload);
    }
    drop(wal);
    (tmp, payloads, ends)
}

/// Rewrite `wal_path` to hold exactly `bytes[..offset]`, reopen, and return
/// the recovery report plus the replayed prefix.
fn recover_at(wal_path: &Path, bytes: &[u8], offset: usize) -> (Recovery, Vec<Record>) {
    fs::write(wal_path, &bytes[..offset]).expect("write truncated wal");
    let opts = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(wal_path, opts).expect("recover wal");
    let records = wal.read_records().expect("replay records");
    (recovery, records)
}

#[test]
fn every_byte_boundary_recovers_exactly_the_acknowledged_prefix() {
    let (tmp, payloads, ends) = build_wal(Path::new("crash.log"), 12);
    let wal_path = tmp.path().join("crash.log");
    let bytes = fs::read(&wal_path).expect("read wal");

    for offset in 0..=bytes.len() {
        let expected_count = ends.iter().filter(|&&end| end <= offset).count();
        let (recovery, records) = recover_at(&wal_path, &bytes, offset);

        assert_eq!(
            recovery.next_seq, expected_count as u64,
            "next_seq at byte offset {offset}"
        );
        assert_eq!(
            recovery.verified_records, expected_count as u64,
            "verified_records at byte offset {offset}"
        );

        let replayed: Vec<Vec<u8>> = records
            .iter()
            .map(|record| record.payload.clone())
            .collect();
        assert_eq!(
            replayed,
            payloads[..expected_count],
            "replayed prefix at byte offset {offset}"
        );
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record.seq, index as u64, "seq at byte offset {offset}");
        }

        let verified_end = ends
            .get(expected_count.wrapping_sub(1))
            .copied()
            .unwrap_or(0);
        assert_eq!(
            recovery.truncated_bytes,
            (offset - verified_end) as u64,
            "truncated_bytes at byte offset {offset}"
        );
    }
}

#[test]
fn garbage_tail_is_discarded_back_to_last_verified_record() {
    let (tmp, payloads, _) = build_wal(Path::new("garbage.log"), 5);
    let wal_path = tmp.path().join("garbage.log");

    let mut bytes = fs::read(&wal_path).expect("read wal");
    bytes.extend_from_slice(b"\x00\x11\x22\x33\x44\x55\x66");
    fs::write(&wal_path, &bytes).expect("append garbage");

    let opts = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&wal_path, opts).expect("recover");
    assert_eq!(recovery.next_seq, 5);
    assert_eq!(recovery.verified_records, 5);
    assert!(recovery.truncated_bytes > 0);
    let replayed: Vec<Vec<u8>> = wal
        .read_records()
        .expect("replay")
        .iter()
        .map(|record| record.payload.clone())
        .collect();
    assert_eq!(replayed, payloads);
}

#[test]
fn near_miss_magic_is_still_garbage() {
    let (tmp, _, _) = build_wal(Path::new("near.log"), 3);
    let wal_path = tmp.path().join("near.log");

    let mut bytes = fs::read(&wal_path).expect("read wal");
    bytes.extend_from_slice(&[b'L', 0x00, 0x00, 0x01, 0x02, 0x03, 0x04]);
    fs::write(&wal_path, &bytes).expect("append near-miss garbage");

    let opts = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (wal, recovery) = Wal::open(&wal_path, opts).expect("recover");
    assert_eq!(recovery.next_seq, 3);
    assert_eq!(wal.next_seq(), 3);
    let _ = recovery;
}

#[test]
fn append_resumes_after_a_crash_boundary() {
    let (tmp, payloads, ends) = build_wal(Path::new("resume.log"), 6);
    let wal_path = tmp.path().join("resume.log");
    let bytes = fs::read(&wal_path).expect("read wal");

    let boundary = ends[2] + 5;
    fs::write(&wal_path, &bytes[..boundary]).expect("truncate at crash");

    let opts = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&wal_path, opts).expect("recover");
    assert_eq!(recovery.next_seq, 3);

    let next = b"after-crash".to_vec();
    assert_eq!(wal.append(&next).expect("append after crash"), 3);
    let replayed: Vec<Vec<u8>> = wal
        .read_records()
        .expect("replay")
        .iter()
        .map(|record| record.payload.clone())
        .collect();
    assert_eq!(replayed, {
        let mut expected = payloads[..3].to_vec();
        expected.push(next);
        expected
    });
}
