//! Criterion-driven hot-path benchmarks for the ledger's core write operations.
//!
//! Statistical medians replace the one-shot harness (see
//! `docs/BENCHMARKS.md`). `scripts/bench_gate.py` runs these, records them
//! against `docs/benchmarks/baseline.json`, and fails the gate if a median
//! exceeds the committed bound (ADR-0008).

#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::ledger::Ledger;
use ledger_core::transfer::Transfer;

const USDC_SCALE: Scale = Scale::usdc();
const BATCH_SIZE: usize = 256;
const REPLAY_EVENTS: u64 = 200_000;

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

fn bench_create_transfer(c: &mut Criterion) {
    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);
    let mut next_id = 100u128;

    c.bench_function("create_transfer", |b| {
        b.iter(|| {
            let transfer = Transfer::new_immediate(
                TransferId::new(black_box(next_id)),
                pool,
                customer,
                Amount::new(1),
                0,
            )
            .expect("construct transfer");
            next_id += 1;
            ledger.create_transfer(transfer).expect("apply transfer");
        })
    });
}

fn bench_two_phase_round_trip(c: &mut Criterion) {
    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);
    let mut next_id = 1000u128;

    c.bench_function("two_phase_round_trip", |b| {
        b.iter(|| {
            let pending = Transfer::new_pending(
                TransferId::new(black_box(next_id)),
                customer,
                pool,
                Amount::new(1),
                0,
            )
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
        })
    });
}

fn bench_apply_batch(c: &mut Criterion) {
    let mut ledger = seeded_ledger();
    let pool = AccountId::new(2);
    let customer = AccountId::new(3);
    let mut next_id = 100_000u128;
    let mut clock = 1_000_000u64;

    c.bench_function("apply_batch", |b| {
        b.iter(|| {
            let batch: Vec<Transfer> = (0..BATCH_SIZE)
                .map(|_| {
                    let transfer = Transfer::new_immediate(
                        TransferId::new(black_box(next_id)),
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
            ledger.apply_batch(&batch).expect("apply batch")
        })
    });
}

fn bench_replay(c: &mut Criterion) {
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
    c.bench_function("replay", |b| {
        b.iter(|| {
            let replayed = Ledger::replay(USDC_SCALE, &journal).expect("replay");
            replayed.verify_invariants().expect("replayed invariants");
            black_box(replayed)
        })
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default();
    targets = bench_create_transfer,
        bench_two_phase_round_trip,
        bench_apply_batch,
        bench_replay,
);
criterion_main!(benches);
