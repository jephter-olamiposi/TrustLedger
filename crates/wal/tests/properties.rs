//! Property tests: recovery must never fabricate, drop, or corrupt history.
//!
//! Under any truncation, random bit corruption, or garbage tail, reopening a
//! wal must yield a byte-identical prefix of the committed records — never a
//! too-long list, never a reordering, and never a corrupted frame that passes
//! its checksum.

use std::fs;

use proptest::prelude::*;
use tempfile::TempDir;

use wal::{record, Wal, WalOptions};

/// Bounded batch payloads: up to 8 batches of up to 256 bytes each.
fn payloads() -> impl Strategy<Value = Vec<Vec<u8>>> {
    proptest::collection::vec(
        proptest::collection::vec(any::<u8>(), 0..256usize),
        0..8usize,
    )
}

/// Byte positions to flip in a corruption case (deduped by modulus).
fn flip_positions() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(any::<usize>(), 0..4usize)
}

/// Garbage appended to the end of a log.
fn garbage_tail() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 1..48usize)
}

/// Commit `payloads` to a per-case wal and return the raw committed bytes.
fn build(tmp: &TempDir, payloads: &[Vec<u8>]) -> Vec<u8> {
    let path = tmp.path().join("prop.log");
    let options = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, _) = Wal::open(&path, options).expect("open wal");
    for payload in payloads {
        wal.append(payload).expect("append");
    }
    drop(wal);
    fs::read(&path).expect("read wal")
}

/// Write `bytes` as the whole wal file, reopen, and return the replayed
/// payloads plus the reported next seq.
fn recover(raw: &[u8]) -> (Vec<Vec<u8>>, u64) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("prop.log");
    fs::write(&path, raw).expect("write wal");
    let options = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&path, options).expect("recover wal");
    let records = wal.read_records().expect("replay");
    (
        records
            .iter()
            .map(|record| record.payload.clone())
            .collect(),
        recovery.next_seq,
    )
}

/// Count records of the intact `bytes` whose frame ends within the truncated
/// file of length `truncated_len`.
fn expected_prefix_len(bytes: &[u8], truncated_len: usize) -> usize {
    let mut offset = 0usize;
    let mut count = 0usize;
    while offset + record::FRAME_OVERHEAD <= bytes.len() {
        let head = &bytes[offset..offset + record::HEADER_LEN];
        let magic = u32::from_le_bytes(head[0..4].try_into().expect("magic is 4 bytes"));
        if magic != record::MAGIC {
            break;
        }
        if head[4] != record::VERSION {
            break;
        }
        let payload_len = u32::from_le_bytes(
            bytes[offset + record::HEADER_LEN - 4..offset + record::HEADER_LEN]
                .try_into()
                .expect("payload length is 4 bytes"),
        ) as usize;
        if payload_len > record::DEFAULT_MAX_PAYLOAD_LEN {
            break;
        }
        let total = record::frame_len(payload_len);
        if offset + total > truncated_len {
            break;
        }
        count += 1;
        offset += total;
    }
    count
}

/// The three invariants every mutation must satisfy.
fn assert_recovers_as_prefix(original: &[Vec<u8>], replayed: &[Vec<u8>], next_seq: u64) {
    assert!(
        replayed.len() <= original.len(),
        "recovery invented history: replayed {} records but only {} were committed",
        replayed.len(),
        original.len()
    );
    assert_eq!(
        replayed,
        &original[..replayed.len()],
        "recovered records are not a byte-identical prefix of the commit history"
    );
    assert_eq!(
        next_seq,
        replayed.len() as u64,
        "next_seq must immediately follow the replayed prefix"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn byte_corruption_recovers_only_intact_prefix(
        payloads in payloads(),
        flips in flip_positions(),
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut bytes = build(&tmp, &payloads);
        for position in flips {
            if position < bytes.len() {
                bytes[position] ^= 0x40;
            }
        }
        let (replayed, next_seq) = recover(&bytes);
        assert_recovers_as_prefix(&payloads, &replayed, next_seq);
    }

    #[test]
    fn garbage_tail_recovers_only_intact_prefix(
        payloads in payloads(),
        garbage in garbage_tail(),
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut bytes = build(&tmp, &payloads);
        bytes.extend_from_slice(&garbage);
        let (replayed, next_seq) = recover(&bytes);
        assert_recovers_as_prefix(&payloads, &replayed, next_seq);
    }
}

#[test]
fn truncation_recovers_exactly_the_committed_prefix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let payloads = [
        b"first".to_vec(),
        Vec::new(),
        vec![0u8; 257],
        b"small".to_vec(),
        vec![1u8; 64],
        b"last".to_vec(),
    ];
    let bytes = build(&tmp, &payloads);

    for truncated_len in 0..=bytes.len() {
        let expected = expected_prefix_len(&bytes, truncated_len);
        let (replayed, next_seq) = recover(&bytes[..truncated_len]);
        assert_eq!(
            replayed,
            payloads[..expected],
            "truncation to {truncated_len} bytes recovered {}/{} of {} committed records",
            replayed.len(),
            expected,
            payloads.len()
        );
        assert_eq!(next_seq, expected as u64);
    }
}
