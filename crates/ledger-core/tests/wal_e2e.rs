//! End-to-end write-ahead test: crash at every byte of a committed log and
//! check that recovery rebuilds exactly the acknowledged journal.
//!
//! The write-ahead flow is exercised for every journal event kind:
//!
//! ```text
//! events = ledger.prepare_batch(&batch)
//! seq    = wal.append(codec::encode_events(...))
//! ledger.commit_events(&events)
//! ```

use std::fs;
use std::path::Path;

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::codec::{decode_events, encode_events};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::journal::LedgerEvent;
use ledger_core::transfer::Transfer;
use ledger_core::{BatchOp, Ledger};
use wal::{Record, SnapshotFile, Wal, WalOptions};

const SCALE: Scale = Scale::usdc();

/// Build a ledger whose journal is written through the wal, returning the
/// per-batch event lists and the byte length of the file after each batch.
fn build_path_and_history(dir: &Path) -> (Ledger, Vec<Vec<LedgerEvent>>, Vec<u64>, Vec<u8>) {
    let wal_path = dir.join("history.log");
    let options = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, _) = Wal::open(&wal_path, options).expect("open wal");
    let mut ledger = Ledger::new(SCALE);

    let op_batch = |i: usize| -> Vec<BatchOp> {
        match i {
            0 => vec![
                BatchOp::CreateAccount {
                    id: AccountId::new(1),
                    account_type: AccountType::Asset,
                    flags: AccountFlags::bank_asset(),
                    scale: SCALE,
                    timestamp: 1,
                },
                BatchOp::CreateAccount {
                    id: AccountId::new(2),
                    account_type: AccountType::Liability,
                    flags: AccountFlags::customer(),
                    scale: SCALE,
                    timestamp: 1,
                },
            ],
            1 | 5 | 9 | 13 | 15 => vec![BatchOp::Transfer(
                Transfer::new_immediate(
                    TransferId::new(30_000 + i as u128),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(1_000_000),
                    (i as u64) + 1,
                )
                .expect("immediate transfer"),
            )],
            2 | 4 | 6 | 8 | 10 | 12 => vec![BatchOp::Pending(
                Transfer::new_pending(
                    TransferId::new(10_000 + i as u128),
                    AccountId::new(1),
                    AccountId::new(2),
                    Amount::new(5_000_000),
                    (i as u64) + 1,
                )
                .expect("pending transfer"),
            )],
            3 | 7 | 11 => vec![BatchOp::PostPending {
                pending_id: TransferId::new(10_000 + (i - 1) as u128),
                post_transfer_id: TransferId::new(20_000 + i as u128),
                amount: Amount::new(2_000_000),
                timestamp: (i as u64) + 1,
            }],
            14 => vec![BatchOp::VoidPending {
                pending_id: TransferId::new(10_012),
                timestamp: 15,
            }],
            _ => unreachable!("schedule covers i in 0..16"),
        }
    };

    let mut batch_events: Vec<Vec<LedgerEvent>> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();

    for i in 0..16 {
        let ops = op_batch(i);
        let prepared = ledger.prepare_batch(&ops).expect("prepare batch");
        let payload = encode_events(&prepared).expect("encode batch");
        wal.append(&payload).expect("append batch");
        ledger.commit_events(&prepared).expect("commit batch");

        let disk = wal.committed_len().expect("committed length");
        ends.push(disk);
        batch_events.push(prepared);
    }

    drop(wal);
    let bytes = fs::read(&wal_path).expect("read wal");
    (ledger, batch_events, ends, bytes)
}

/// Rebuild a ledger from `records` by decoding every payload and replaying.
fn ledger_from_records(records: &[Record], from: usize) -> Ledger {
    let mut events = Vec::new();
    for record in &records[from..] {
        events.extend(decode_events(&record.payload).expect("decode record"));
    }
    Ledger::replay(SCALE, &events).expect("replay events")
}

fn expected_prefix(original: &[Vec<LedgerEvent>], count: usize) -> Vec<LedgerEvent> {
    original[..count].iter().flatten().cloned().collect()
}

#[test]
fn every_byte_boundary_recovers_exactly_the_acked_journal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (original, batch_events, ends, bytes) = build_path_and_history(dir.path());
    let original_journal = original.journal().to_vec();
    let total = bytes.len();

    for boundary in &ends {
        assert!(
            *boundary as usize <= total,
            "frame end {boundary} must sit inside a {total}-byte log"
        );
    }

    for offset in 0..=total {
        let acked = ends.iter().filter(|&&end| end <= offset as u64).count();
        let wal_path = dir.path().join("recovered.log");
        fs::write(&wal_path, &bytes[..offset]).expect("truncate at crash point");

        let options = WalOptions {
            sync_per_append: false,
            ..WalOptions::default()
        };
        let (mut wal, recovery) = Wal::open(&wal_path, options).expect("recover wal");
        assert_eq!(recovery.next_seq, acked as u64, "at offset {offset}");
        assert_eq!(
            recovery.verified_records, acked as u64,
            "at offset {offset}"
        );

        let records = wal.read_records().expect("replay wal");
        let recovered = ledger_from_records(&records, 0);
        let expected = expected_prefix(&batch_events, acked);

        assert_eq!(
            recovered.journal(),
            expected,
            "recovered journal at byte offset {offset}"
        );
        assert_eq!(
            recovered.journal(),
            &original_journal[..expected.len()],
            "recovered journal diverges from the original prefix at byte offset {offset}"
        );
        recovered.verify_invariants().expect("recovered invariants");
    }
}

#[test]
fn snapshot_skips_covered_records_and_corruption_falls_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wal_path = dir.path().join("snapped.log");
    let snapshot_path = dir.path().join("snapshot.dat");
    let options = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };

    let mut ledger = Ledger::new(SCALE);
    let (mut wal, _) = Wal::open(&wal_path, options).expect("open wal");
    let mut batch_events: Vec<Vec<LedgerEvent>> = Vec::new();
    let accounts = vec![
        LedgerEvent::AccountCreated {
            id: AccountId::new(1),
            account_type: AccountType::Asset,
            flags: AccountFlags::bank_asset(),
            scale: SCALE,
            timestamp: 0,
        },
        LedgerEvent::AccountCreated {
            id: AccountId::new(2),
            account_type: AccountType::Liability,
            flags: AccountFlags::customer(),
            scale: SCALE,
            timestamp: 0,
        },
    ];
    wal.append(&encode_events(&accounts).expect("encode accounts"))
        .expect("append accounts");
    ledger.commit_events(&accounts).expect("commit accounts");
    batch_events.push(accounts);

    for i in 1..13 {
        let transfer = Transfer::new_immediate(
            TransferId::new(100 + i as u128),
            AccountId::new(1),
            AccountId::new(2),
            Amount::new(1_000),
            i as u64,
        )
        .unwrap();
        let events = ledger
            .prepare_batch(&[BatchOp::Transfer(transfer)])
            .expect("prepare");
        wal.append(&encode_events(&events).expect("encode"))
            .expect("append");
        ledger.commit_events(&events).expect("commit");
        batch_events.push(events);

        if i == 5 {
            let snapshot = SnapshotFile::new(&snapshot_path, 256 * 1024);
            snapshot
                .save(
                    5,
                    &encode_events(ledger.journal()).expect("encode snapshot"),
                )
                .expect("save snapshot");
        }
    }
    let bytes = fs::read(&wal_path).expect("read wal");
    drop(wal);

    let snapshot = SnapshotFile::new(&snapshot_path, 256 * 1024);

    for offset in 0..=bytes.len() {
        let crash = dir.path().join("crash.log");
        fs::write(&crash, &bytes[..offset]).expect("truncate");

        let (mut wal, recovery) = Wal::open(&crash, options).expect("recover wal");
        let records = wal.read_records().expect("replay wal");
        let durable = recovery.next_seq as usize;

        let recovered = match snapshot
            .load()
            .expect("load snapshot")
            .filter(|snapshot| snapshot.last_seq < durable as u64)
        {
            Some(snapshot) => {
                let mut events = decode_events(&snapshot.payload).expect("decode snapshot");
                for record in &records[snapshot.last_seq as usize + 1..] {
                    events.extend(decode_events(&record.payload).expect("decode record"));
                }
                Ledger::replay(SCALE, &events).expect("replay from snapshot and wal")
            }
            None => ledger_from_records(&records, 0),
        };

        let expected = expected_prefix(&batch_events, durable);
        assert_eq!(recovered.journal(), expected, "at byte offset {offset}");
        recovered.verify_invariants().expect("recovered invariants");
    }
    snapshot
        .save(5, &encode_events(ledger.journal()).expect("encode"))
        .expect("save");
    let mut corrupt = fs::read(&snapshot_path).expect("read snapshot");
    let middle = corrupt.len() / 2;
    corrupt[middle] ^= 0x01;
    fs::write(&snapshot_path, &corrupt).expect("write corrupt snapshot");
    assert!(snapshot.load().expect("load").is_none());
}

#[test]
fn cold_start_matches_live_ledger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (original, _, _, bytes) = build_path_and_history(dir.path());

    let wal_path = dir.path().join("history.log");
    let options = WalOptions {
        sync_per_append: false,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&wal_path, options).expect("reopen wal");
    assert_eq!(recovery.next_seq, 16);
    let records = wal.read_records().expect("replay wal");
    let recovered = ledger_from_records(&records, 0);
    assert_eq!(recovered.journal(), original.journal());
    recovered.verify_invariants().expect("invariants");
    assert_eq!(
        fs::read(&wal_path).expect("read wal"),
        bytes,
        "recovery with a clean tail must not rewrite the file"
    );
}
