# TrustLedger

A distributed double-entry settlement ledger in Rust with on-chain (Solana)
finality. It is:

* **Correct**: balances can't leak. Conservation, transfer structure, and
  account lifecycle are enforced at the mutation boundary and proven by a
  property suite, not inferred from tests that happen to pass.

* **Durable**: acknowledged means fsynced. Every mutation is journaled to a
  checksummed write-ahead log, and crash recovery is tested at every byte
  boundary of the file.

* **Fast**: measured, not estimated. Criterion medians on the hot path exceed
  1M transfers/sec (ADR-0001's promise), and the full throughput table is
  committed to `docs/BENCHMARKS.md`.

[![CI status][ci-badge]][ci-url]
[![license MIT or Apache-2.0][license-badge]][license]

[ci-badge]: https://img.shields.io/github/actions/workflow/status/jephter-olamiposi/TrustLedger/ci.yml?branch=main&label=CI
[ci-url]: https://github.com/jephter-olamiposi/TrustLedger/actions
[license-badge]: https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg
[license]: https://github.com/jephter-olamiposi/TrustLedger/blob/main/LICENSE

[GitHub](https://github.com/jephter-olamiposi/TrustLedger) |
[ADRs](docs/adr/) |
[WAL runbook](docs/WAL-RUNBOOK.md) |
[Benchmarks](docs/BENCHMARKS.md)

## Overview

TrustLedger is an event-sourced, double-entry ledger written as a Rust
workspace. The ledger engine is pure and deterministic — the durability layer
owns the disk. At a high level it provides:

* **`ledger-core`** — the accounting engine: accounts, two-phase (pending →
  posted/voided) transfers, `u128` scaled-decimal math, and
  balances-can't-leak invariants. Replay is fail-loud and byte-for-byte
  deterministic. *(v0.1 shipped)*
* **`crates/wal`** — an append-only stream of length-prefixed, CRC32C-checksummed
  frames with torn-write recovery to the last verified record, streaming constant-memory
  readers, multi-record batch appends, and snapshot checkpoints to bound restart replay. *(v0.1 shipped)*
* **`crates/ingest`** — high-performance tonic/gRPC network ingress with protobuf schemas,
  bounded non-blocking queues, backpressure load-shedding (`RESOURCE_EXHAUSTED` under overload),
  a single-writer micro-batch accumulator, and atomic single-fsync group commits. *(v0.1 shipped)*
* **`crates/raft`** — three-node consensus cluster powered by OpenRaft; decoupled storage (`storage-v2`), simulated in-memory network router for deterministic chaos testing, automated failover in < 0.5s, and split-brain prevention under network partitions (ADR-0009). *(v0.1 shipped)*
* **`crates/merkle`** — an append-only Merkle Mountain Range (MMR) cryptographic proof engine
  supporting $O(\log N)$ inclusion proofs (~640 bytes for $10^6$ transfers), RFC 6962 domain separation,
  and peak bagging. *(v0.1 shipped)*
* **`crates/solana-settle`** — Solana on-chain settlement program and independent verifier CLI.
  Commits batch roots to a Program Derived Address (PDA) with tamper-evident root chaining,
  on-chain inclusion verification in < 4k CUs, and standalone offline receipt verification (ADR-0010). *(v0.1 shipped)*
* **`apps/demo`** — reference client, e2e suite, and the "Verify transfer"
  surface. *(planned)*


## Example

Add the crate, then run a two-phase transfer from vault to customer:

```toml
[dependencies]
ledger_core = { path = "crates/ledger-core" }
```

```rust
use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;
# fn main() -> Result<(), Box<dyn std::error::Error>> {

let mut ledger = Ledger::new(Scale::usdc());
let vault = AccountId::new(1);
let alice = AccountId::new(2);

ledger.create_account(vault, AccountType::Asset, AccountFlags::bank_asset(), Scale::usdc(), 0)?;
ledger.create_account(alice, AccountType::Liability, AccountFlags::customer(), Scale::usdc(), 0)?;

let hold = Transfer::new_pending(TransferId::new(1), vault, alice, Amount::new(1_000_000), 0)?;
ledger.create_pending(hold)?;
ledger.post_pending(TransferId::new(1), TransferId::new(2), Amount::new(1_000_000), 0)?;
ledger.verify_invariants()?;
# Ok(())
# }
```

This README is also the `ledger-core` crate documentation
(`#![doc = include_str!("../../README.md")]` in `src/lib.rs`), so the example
above is a compiled doctest: `cargo test --doc` runs it and documented API
cannot drift from the code.

## Safety model

The ledger enforces, and a property suite proves:

* **Conservation** — total debits equal total credits for both posted and
  pending balances.
* **Structure** — every pending transfer is exactly backed by its debit and
  credit accounts' pending reservations, and a posted transfer may only link a
  hold that exists and is itself posted (checked eagerly at the mutation
  boundary, not only at `verify_invariants`).
* **Encapsulation** — `Scale`, `Amount`, `AccountId`, `TransferId`, and every
  `Transfer` field are private; transfers are only created through validated
  constructors, and the Pending→Posted/Voided lifecycle is a single state
  machine that rejects illegal moves. A deserialized transfer is re-validated
  (fresh ID, distinct accounts, non-zero amount, valid hold link) before it can
  be applied.
* **Determinism** — accounts and transfers live in `BTreeMap` (ADR-0002), so
  identical journal output always reproduces identical state;
  `Ledger::replay` equals the original ledger by value, byte-for-byte.
* **Fail-loud replay** — a journal whose voided amount contradicts the
  reservation it encodes, or whose timestamps run backwards, is rejected with a
  typed `LedgerError` instead of replaying to a wrong-but-plausible state.
* **Monotonic timestamps** — every journal event must carry a non-decreasing
  `timestamp`; older events are rejected (`TimestampBehindPrior`).
* **Zero overdraft and zero leakage** — `available_liability`/`available_asset`
  never report negative money (they read zero when empty, ADR-0004), and
  breaches surface as typed `InsufficientFunds` errors.
* **Account lifecycle** — `close_account` freezes an account (both legs)
  through the same journaled, replayable path as every other mutation.

### Durability

The ledger stays pure; `crates/wal` owns the disk. A mutation is committed in
three steps (ADR-0007):

1. **compute** — `ledger.prepare_batch(&[Transfer])` validates the batch and
   returns the event list without persisting any change;
2. **durable** — encode with `ledger_core::codec` (postcard, ADR-0003) and
   `wal.append(&bytes)`, which fsyncs the frame before returning;
3. **commit** — `ledger.commit_events(&events)` re-validates and applies the
   same list to memory (ADR-0006's ordering).

```text
use ledger_core::codec::encode_events;
use wal::{Wal, WalOptions};

let (mut wal, recovery) = Wal::open("ledger.log", WalOptions::default())?;
let events = ledger.prepare_batch(&[transfer])?;
wal.append(&encode_events(&events)?)?;
ledger.commit_events(&events)?;
```

Each record is a length-prefixed, CRC32C-checksummed frame (`magic "LETW"`,
version, monotonically incrementing `seq`). Crash recovery scans the prefix,
truncates a torn tail to the last verified record, and hard-fails on a
well-formed-but-divergent `seq` (`WalError::SeqMismatch`) rather than guessing at
frame boundaries. Snapshot checkpoints (`SnapshotFile`, atomic rename + parent
fsync) bound restart replay; a corrupt or stale snapshot falls back to full wal
replay. Failure modes and operator response live in
[`docs/WAL-RUNBOOK.md`][runbook].

The guarantee — **acknowledged = durable, crash rolls back only to a verified
record boundary** — is proven to the byte: `crates/wal/tests/crash.rs` and
`crates/ledger-core/tests/wal_e2e.rs` truncate the wal at every byte offset of a
mixed journal and assert the rebuilt ledger equals exactly the acked prefix,
byte-for-byte, plus `verify_invariants`.

### Network Ingress & Micro-Batching (`crates/ingest`)

High-throughput financial ledgers cannot afford a disk fsync per network request. `crates/ingest`
provides a high-performance tonic/gRPC ingress layer that decouples concurrent ingress connections
from disk I/O:

1. **Protobuf Contract** (`proto/ledger.proto`) — strongly typed domain RPCs (`CreateAccount`,
   `CreateTransfer`, `CreatePending`, `PostPending`, `VoidPending`, `ApplyBatch`, `GetAccount`,
   `GetTransfer`). Currency amounts use integer atomic units plus fixed scale (`u8`),
   eliminating floating-point precision bugs.
2. **Bounded Queue & Load Shedding** — ingress workers dispatch requests into a bounded
   non-blocking channel (`IngestQueue`). When the queue reaches capacity under traffic spikes, requests are
   immediately shed with gRPC `RESOURCE_EXHAUSTED` status, shielding the state engine from
   unbounded memory growth and cascading latency.
3. **Single-Writer Micro-Batch Accumulator** — a dedicated worker task accumulates incoming requests
   up to a configurable batch size (e.g. 512 operations) or maximum delay window (e.g. 2ms).
4. **Group Commit** — the accumulated batch is validated, serialized via `codec`, and committed to the
   write-ahead log with a single `append_batch` call. A single `fsync` commits hundreds of
   transactions simultaneously, amortizing disk latency down to microseconds per transfer.
5. **Causal FIFO Ordering** — requests are executed strictly in queue arrival order, guaranteeing
   immediate intra-batch and cross-operation visibility (e.g. creating an account and funding it
   in subsequent requests succeeds deterministically without race conditions).

## Testing

* **Unit** (`tests/unit.rs`, 21 tests) — exact-error matrix for every rejection
  path: double-post, post-after-void, void-after-post, duplicate IDs,
  same-account, zero-amount, closed-account, partial-exceeds-pending,
  timestamp-in-past, corrupted journal.
* **Property** (`tests/invariants.rs`, 3 proptests, 200 cases) — a shadow model
  asserts the ledger equals an independent balance computation after *every*
  action; a corrupted-journal variant proves replay fails loudly.
* **Rollback matrix** (`src/ledger.rs`) — every `LedgerEvent` variant is applied
  as part of a batch whose later event fails; the batch must roll back to the
  exact pre-batch ledger, so a forgotten undo pre-image for any event kind is
  caught by a test, not by an incident.
* **Doctest** — the README example above is the crate-root documentation, so
  `cargo test --doc` compiles and runs it when it runs the suite.
* **wal** (`crates/wal`: 34 tests) — every-byte-boundary crash sweeps over
  byte-corrupted and partially-written logs, proving recovery lands exactly on
  the last verified frame ([runbook][runbook]).
* **Network Ingress & Backpressure** (`crates/ingest`: 8 tests) — end-to-end gRPC integration
  suite verifying the full RPC lifecycle, two-phase holds, scale rejections, atomic batch rollbacks,
  and high-concurrency burst load-shedding (`RESOURCE_EXHAUSTED`).
* **Distributed Consensus & Chaos** (`crates/raft`: 3 tests) — 3-node cluster bootstrap and
  quorum replication, Jepsen-style network partition simulations (minority isolation, majority progression,
  partition healing, log truncation, zero split-brain), and rapid leader failover (< 0.5s failover SLO).
* **Merkle Mountain Range (MMR)** (`crates/merkle`: 9 tests) — proptests across arbitrary leaf counts,
  domain-separated SHA-256 leaf/node/peak hashing (RFC 6962), logarithmic inclusion proofs, and tamper detection.
* **On-Chain Settlement & Verifier** (`crates/solana-settle`: 4 tests) — PDA initialization, sequential batch
  commitments with tamper-evident root chaining, on-chain proof verification in BPF runtime (< 4k CUs), and
  standalone verifier CLI execution.

## Cryptographic Verifiability & On-Chain Finality

TrustLedger commits each settled batch root to a Solana Program Derived Address (PDA). Any client or
auditor can independently verify the inclusion of a transfer in $O(\log N)$ steps without trusting
the operator:

```bash
# Verify a portable transfer receipt using the standalone verifier CLI
cargo run -p solana-settle --bin verifier -- --receipt path/to/receipt.json

# Or verify direct cryptographic parameters:
cargo run -p solana-settle --bin verifier -- \
  --root <64-char-hex-merkle-root> \
  --leaf <64-char-hex-transfer-hash> \
  --proof-file path/to/proof.bin
```

When verified, the CLI exits with code `0` and outputs:
```text
[VERIFIED] Cryptographic proof matches on-chain Merkle root!
Status: Transfer is immutably included in settlement batch 42.
```

## Benchmarks

### Hot-Path State Machine Throughput

Hot-path throughput is measured by the criterion harness at
`benches/throughput.rs` (statistical medians, [docs/BENCHMARKS.md][benchmarks]),
and `scripts/bench_gate.py` fails CI when a median exceeds its committed bound
in `docs/benchmarks/baseline.json` (ADR-0008). Medians on Apple silicon (`profile.release`
with LTO + overflow checks):

* `create_transfer`: ~4.4M transfers/sec (~229 ns/op)
* two-phase round-trip: ~1.6M pairs/sec (~627 ns/op)
* `apply_batch` (256/batch): ~3.4M transfers/sec (~297 ns/op)
* `Ledger::replay`: ~4.9M events/sec (~206 ns/op)

Each exceeds the >1M transfers/sec claim in ADR-0001, confirming the
`BTreeMap` determinism cost is not observable on the hot path.

### Durability & Persistence (`crates/wal`)

Durability and recovery throughput measured via `benches/persistence.rs` (10,000 batches x 16 KiB):

* `append (fsync per batch)`: 3.50 MB/s (~214 batches/sec, 4.68 ms/batch)
* `append (buffered, single sync)`: 259.30 MB/s (~15,827 batches/sec, 63.2 µs/batch)
* `recover + replay 10,000 batches`: 1092.93 MB/s (~15.0 µs/batch)


## Supported Rust versions

The workspace pins MSRV 1.80 and a rolling-patch toolchain
(`rust-toolchain.toml`). CI runs an MSRV job (`cargo check --workspace
--all-targets --locked` on 1.80.0) plus a `cargo-deny` supply-chain job on
every push (ADR-0008). The committed lockfile is the source of truth for the
1.80-compatible pins: criterion =0.7.0, proptest 1.8.0, and a couple of
transitive holds — new majors of criterion (0.8) and proptest (1.9+) and
edition-2024 transitives (e.g. getrandom 0.4, clap_lex 1.x) each raise the
floor, so an MSRV bump is a reviewed decision, not a side-effect of
`cargo update`.

## Contributing

Run the full gate before pushing — `./scripts/check` runs fmt, clippy
`-D warnings`, the test suite, both benchmark harnesses, and the regression
gate. CI runs the same gate plus MSRV and supply-chain jobs. Every architecture
decision is recorded as an ADR under [`docs/adr`][adrs].

## License

This project is licensed under either [MIT][mit] or [Apache-2.0][apache], at
your option.

[mit]: https://github.com/jephter-olamiposi/TrustLedger/blob/main/LICENSE-MIT
[apache]: https://github.com/jephter-olamiposi/TrustLedger/blob/main/LICENSE-APACHE
[runbook]: docs/WAL-RUNBOOK.md
[benchmarks]: docs/BENCHMARKS.md
[adrs]: docs/adr/