//! Measures the wal's persistence cost: append throughput with `fsync` per
//! record (the durable-commit rate) and the recovery scan rate (bytes verified
//! per second on open). Run with `cargo bench --package wal`.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use wal::Wal;
use wal::WalOptions;

const BATCH_PAYLOAD_LEN: usize = 16 * 1024;
const BATCHES: u64 = 10_000;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wal-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create bench dir");
    dir
}

/// A payload approximating a real ledger batch (postcard-encoded events).
fn batch_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(BATCH_PAYLOAD_LEN);
    for i in 0..BATCH_PAYLOAD_LEN / 8 {
        payload.extend_from_slice(&(i as u64).to_le_bytes());
    }
    payload
}

fn bench_append(sync_per_append: bool) {
    let dir = temp_dir();
    let path = dir.join(if sync_per_append {
        "sync.log"
    } else {
        "nosync.log"
    });
    let _ = std::fs::remove_file(&path);

    let options = WalOptions {
        sync_per_append,
        ..WalOptions::default()
    };
    let (mut wal, recovery) = Wal::open(&path, options).expect("open wal");
    assert_eq!(recovery.next_seq, 0);
    let payload = batch_payload();

    let start = Instant::now();
    for _ in 0..BATCHES {
        wal.append(&payload).expect("append batch");
    }
    let elapsed = start.elapsed();
    let total_bytes = BATCHES as u128 * BATCH_PAYLOAD_LEN as u128;
    let label = if sync_per_append {
        "append (fsync per batch)"
    } else {
        "append (buffered, no fsync)"
    };
    println!(
        "{label}: {:>9.2} MB/s ({:>8.0} batches/s, {:.1} us/batch)",
        total_bytes as f64 / elapsed.as_secs_f64() / 1e6,
        BATCHES as f64 / elapsed.as_secs_f64(),
        elapsed.as_nanos() as f64 / BATCHES as f64 / 1e3,
    );
    drop(wal);
    black_box(());
}

fn bench_recovery_scan() {
    let dir = temp_dir();
    let path = dir.join("scan.log");
    let _ = std::fs::remove_file(&path);
    let options = WalOptions::default();
    let (mut wal, _) = Wal::open(&path, options).expect("open wal");
    let payload = batch_payload();
    for _ in 0..BATCHES {
        wal.append(&payload).expect("append batch");
    }
    drop(wal);
    let total_bytes = BATCHES as u128 * BATCH_PAYLOAD_LEN as u128;

    let start = Instant::now();
    let (mut wal, recovery) = Wal::open(&path, options).expect("recover wal");
    let _ = wal.read_records().expect("replay records");
    let elapsed = start.elapsed();
    println!(
        "recover + replay {} batches: {:>9.2} MB/s ({:.1} us/batch)",
        recovery.verified_records,
        total_bytes as f64 / elapsed.as_secs_f64() / 1e6,
        elapsed.as_nanos() as f64 / BATCHES as f64 / 1e3,
    );
    drop(wal);
    black_box(());
}

fn main() {
    println!(
        "== wal persistence [{} x {} KiB batches] ==",
        BATCHES,
        BATCH_PAYLOAD_LEN / 1024
    );
    bench_append(true);
    bench_append(false);
    bench_recovery_scan();
}
