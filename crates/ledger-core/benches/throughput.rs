//! Zero-dependency hot-path benchmark for the ledger's core write operations.
//!
//! Measures representative workloads against a pre-seeded ledger:
//! - immediate `create_transfer`
//! - two-phase `create_pending` + `post_pending` round-trip
//! - sliced `apply_batch` application
//! - full `replay` of a journal
//!
//! Run with `cargo bench --package ledger-core`. Numbers are printed as
//! transfers/second to back the throughput claims in the ADR.

use std::hint::black_box;
use std::time::Instant;

use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::ledger::Ledger;
use ledger_core::transfer::Transfer;

const ITERATIONS: u64 = 1_000_000;
const USDC_SCALE: Scale = Scale::usdc();

/// Seeds a ledger with two funded accounts (a vault and a customer) plus one
/// pool account so transfers can circulate without exhausting balance.
fn seeded_ledger() -> Ledger {
    let mut ledger = Ledger::new(USDC_SCALE);
    let vault = AccountId::new(1);
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);

    for (id, account_type, flags) in [
        (vault, AccountType::Asset, AccountFlags::bank_asset()),
        (pool, AccountType::Asset, AccountFlags::unrestricted()),
        (customer, AccountType::Liability, AccountFlags::customer()),
    ] {
        ledger
            .create_account(id, account_type, flags, USDC_SCALE, 0)
            .expect("seed account");
    }

    let funding = Transfer::new_immediate(
        TransferId::new(1),
        vault,
        customer,
        Amount::new(u128::MAX / 2),
        0,
    )
    .expect("seed funding");
    ledger.create_transfer(funding).expect("apply funding");

    ledger
}

fn bench_immediate_transfer() {
    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);

    let start = Instant::now();
    for (next_id, i) in (100u128..).zip(0..ITERATIONS) {
        let transfer =
            Transfer::new_immediate(TransferId::new(next_id), pool, customer, Amount::new(1), i)
                .expect("construct transfer");
        ledger
            .create_transfer(transfer)
            .expect("apply immediate transfer");
    }
    let elapsed = start.elapsed();
    let per_second = ITERATIONS as f64 / elapsed.as_secs_f64();
    println!(
        "create_transfer: {:>10.0} transfers/sec ({} ns/op)",
        per_second,
        elapsed.as_nanos() / ITERATIONS as u128
    );
    black_box(ledger);
}

fn bench_two_phase_round_trip() {
    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);
    let mut next_id = 1000u128;

    let start = Instant::now();
    for _ in 0..ITERATIONS / 2 {
        let pending =
            Transfer::new_pending(TransferId::new(next_id), customer, pool, Amount::new(1), 0)
                .expect("construct pending");
        ledger.create_pending(pending).expect("apply pending hold");
        ledger
            .post_pending(
                TransferId::new(next_id),
                TransferId::new(next_id + 1),
                Amount::new(1),
                0,
            )
            .expect("capture pending hold");
        next_id += 2;
    }
    let elapsed = start.elapsed();
    let ops = ITERATIONS as f64;
    let per_second = ops / elapsed.as_secs_f64();
    println!(
        "two-phase (pending+post): {:>10.0} round-trips/sec ({} ns/op)",
        per_second,
        elapsed.as_nanos() / ITERATIONS as u128
    );
    black_box(ledger);
}

fn bench_apply_batch() {
    const BATCH_SIZE: usize = 256;
    const BATCH_ITERATIONS: u64 = 64;

    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);
    let mut next_id = 100_000u128;
    let mut clock = 1_000_000u64;

    let start = Instant::now();
    let total = BATCH_SIZE as u128 * BATCH_ITERATIONS as u128;
    for _ in 0..BATCH_ITERATIONS {
        let batch: Vec<Transfer> = (0..BATCH_SIZE)
            .map(|_| {
                let transfer = Transfer::new_immediate(
                    TransferId::new(next_id),
                    pool,
                    customer,
                    Amount::new(1),
                    clock,
                )
                .expect("construct batch transfer");
                next_id += 1;
                clock += 1;
                transfer
            })
            .collect();
        ledger.apply_batch(&batch).expect("apply batch");
    }
    let elapsed = start.elapsed();
    println!(
        "apply_batch ({BATCH_SIZE}/batch): {:>10.0} transfers/sec ({} ns/op)",
        total as f64 / elapsed.as_secs_f64(),
        elapsed.as_nanos() / total
    );
    black_box(ledger);
}

fn bench_replay() {
    const REPLAY_EVENTS: u64 = 200_000;

    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);

    for (transfer_id, i) in (200_000u128..).zip(0..REPLAY_EVENTS) {
        let transfer = Transfer::new_immediate(
            TransferId::new(transfer_id),
            pool,
            customer,
            Amount::new(1),
            1_000_000 + i,
        )
        .expect("construct replayed transfer");
        ledger
            .create_transfer(transfer)
            .expect("apply replayed transfer");
    }

    let journal = ledger.journal().to_vec();
    let start = Instant::now();
    let replayed = Ledger::replay(USDC_SCALE, &journal).expect("replay");
    let elapsed = start.elapsed();
    replayed.verify_invariants().expect("replayed invariants");
    println!(
        "replay: {:>10.0} events/sec ({} ns/op)",
        journal.len() as f64 / elapsed.as_secs_f64(),
        elapsed.as_nanos() / journal.len() as u128
    );
    black_box(replayed);
}

fn main() {
    println!(
        "== ledger-core hot-path throughput [{} iterations each] ==",
        ITERATIONS
    );
    bench_immediate_transfer();
    bench_two_phase_round_trip();
    bench_apply_batch();
    bench_replay();
}
