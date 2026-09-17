# TrustLedger

**A fast, crash-resilient double-entry ledger in Rust with cryptographic settlement on Solana.**

[![CI Status](https://img.shields.io/github/actions/workflow/status/jephter-olamiposi/TrustLedger/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger/actions)
[![MSRV](https://img.shields.io/badge/MSRV-1.80-blue.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger)
[![License](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger/blob/main/LICENSE)
[![Safety](https://img.shields.io/badge/Unsafe-0%25-brightgreen.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger)
[![Performance](https://img.shields.io/badge/Throughput->4.4M%20tx%2Fs-purple.svg?style=flat-square)](docs/BENCHMARKS.md)

[Quickstart](#quickstart) |
[Why TrustLedger?](#why-trustledger) |
[Architecture](#architecture) |
[Core Invariants](#core-invariants) |
[Crate Layout](#crate-layout) |
[Benchmarks](#benchmarks) |
[Chaos Testing (DST)](#chaos-testing-dst) |
[ADRs](docs/adr/)

---

## Why TrustLedger?

Most modern banking and payment apps still run on traditional relational databases like PostgreSQL or MySQL. While relational databases are great general-purpose tools, they struggle with high-volume financial ledgers:

1. **Row locks choke throughput:** When thousands of customers pay the same merchant, database row-level locks on that merchant's balance serialize every transaction into a massive bottleneck.
2. **Silent balance drift:** If a database crashes mid-commit or an external payment rail drops a webhook, balances across services can quietly drift out of sync with no automated way to detect or repair the discrepancy.
3. **Torn writes cause data corruption:** An abrupt server power cut while writing to disk can leave half-written, corrupt database pages that fail upon restart.
4. **Auditors must blindly trust the database admin:** Traditional databases offer zero external mathematical proof that historical transactions haven't been quietly edited, deleted, or backdated by a rogue employee or compromised credential.

I built **TrustLedger** to solve these problems by rethinking the ledger stack from the ground up:

* **Keep the hot path in memory:** Pure deterministic state machine in Rust processing **over 4.4 million transfers per second** with zero database lock contention.
* **Durability before acknowledgment:** Mutations are written to an append-only, CRC32C-checksummed Write-Ahead Log (WAL) with group fsync. Crash recovery is tested down to every single byte boundary of the log.
* **Balances physically cannot leak:** Double-entry accounting is enforced at the type level. Debits must equal credits on every mutation, available balance cannot drop below zero, and floating-point math is strictly forbidden.
* **Cryptographic proof on Solana:** Batches of settled transfers are hashed into an incremental Merkle Mountain Range (MMR) and anchored to a Solana Program Derived Address (PDA). Any client can independently verify their payment receipt offline without trusting the ledger operator.
* **Continuous 3-way reconciliation:** An automated background auditor continuously reconciles payment gateway intents, double-entry ledger balances, and Solana blockchain state, ensuring zero balance drift ($\text{Drift} \equiv \$0.00$).

---

## Quickstart

### 1. Launch the Single-Page Operations Console

TrustLedger includes an interactive web dashboard (built with Axum and clean light-theme Tailwind CSS) to test real payment processing, card holds, and on-chain settlement:

```bash
# Build and run the demo server on port 8080:
cargo run -p demo --bin demo
```

Open your browser to:
```text
http://127.0.0.1:8080/
```

From this console, you can:
* **Run the 1-Click Pipeline:** Watch an order authorize with a hold ($50.00), capture to the merchant, commit to the Solana PDA, and verify its Merkle inclusion proof.
* **Experience Two-Phase Card Holds:** Place funds in escrow and watch the double-entry books reserve money until captured or cancelled.
* **Simulate & Heal Balance Drift:** Click "Simulate Drift" to inject a fake $50 bank discrepancy and watch the continuous reconciliation engine detect and auto-heal it.
* **Trigger In-Modal Solana Anchoring:** Commit batches to Solana L1 directly from inside the digital receipt modal.
* **Run Hardware Chaos Tests:** Trigger simulated network partitions or torn-write power failures directly from the browser.

### 2. Verify a Cryptographic Receipt Offline

Any customer or external auditor can verify payment finality against the Solana blockchain using the standalone CLI:

```bash
# Verify a portable transfer receipt JSON:
cargo run -p solana-settle --bin verifier -- --receipt path/to/receipt.json

# Or verify raw parameters directly:
cargo run -p solana-settle --bin verifier -- \
  --root <64-char-hex-solana-merkle-root> \
  --leaf <64-char-hex-transfer-hash> \
  --proof-file path/to/proof.bin
```

When verified, the CLI exits with code `0`:
```text
[VERIFIED] Cryptographic proof matches on-chain Merkle root!
Status: Transfer is immutably included in settlement batch #42.
```

### 3. Run the Multi-Node Cluster in Docker

```bash
# Launch a 3-node Raft consensus cluster and Prometheus scraper:
docker compose -f docker/docker-compose.yml up -d

# Inspect live Prometheus exposition metrics:
curl -s http://localhost:8080/metrics | grep trustledger_
```

---

## Architecture

```text
                                  CLIENT TRAFFIC
                                        │
                         ┌──────────────┴──────────────┐
                         │   tonic/gRPC & REST Ingress  │
                         │   Bounded Queue Backpressure │
                         └──────────────┬──────────────┘
                                        │ (Micro-Batch Accumulator: 512 tx or 2ms)
                                        ▼
                         ┌─────────────────────────────┐
                         │   OpenRaft Consensus Group  │
                         │   3-Node Replicated Quorum   │
                         └──────────────┬──────────────┘
                                        │
                    ┌───────────────────┴───────────────────┐
                    ▼                                       ▼
    ┌───────────────────────────────┐       ┌───────────────────────────────┐
    │       Write-Ahead Log         │       │     In-Memory State Engine    │
    │   Append-Only + CRC32C Frame  │       │   Deterministic BTreeMap      │
    │   Single-Fsync Group Commit   │──────▶│   Two-Phase Pending Holds     │
    │   Torn-Write Crash Recovery   │       │   Zero Overdraft Conservation │
    └───────────────────────────────┘       └───────────────┬───────────────┘
                                                            │
                                                            ▼ (Batch Settled)
                                            ┌───────────────────────────────┐
                                            │  Merkle Mountain Range (MMR)  │
                                            │  RFC 6962 Domain Separation   │
                                            │  O(log N) Cryptographic Proof │
                                            └───────────────┬───────────────┘
                                                            │
                                                            ▼ (Anchored to L1)
                                            ┌───────────────────────────────┐
                                            │   Solana On-Chain Notary PDA  │
                                            │   Tamper-Evident Root Chaining│
                                            │   Offline Verifier CLI (<4k CU)│
                                            └───────────────────────────────┘
```

---

## Code Example

The core accounting engine is pure, thread-safe, and zero-allocation on validation errors. Here is how a two-phase payment (Card Hold $\to$ Capture) works in `ledger-core`:

```rust
use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Initialize ledger configured for USDC (6 decimal places)
let mut ledger = Ledger::new(Scale::usdc());
let vault = AccountId::new(1);
let alice = AccountId::new(2);

// Create bank clearing vault (Asset) and customer wallet (Liability)
ledger.create_account(vault, AccountType::Asset, AccountFlags::bank_asset(), Scale::usdc(), 0)?;
ledger.create_account(alice, AccountType::Liability, AccountFlags::customer(), Scale::usdc(), 0)?;

// Phase 1: Place an authorization hold ($1.00 USDC = 1,000,000 atomic units)
let hold = Transfer::new_pending(TransferId::new(1), vault, alice, Amount::new(1_000_000), 0)?;
ledger.create_pending(hold)?;

// Phase 2: Post the hold to capture settled funds to Alice's balance
ledger.post_pending(TransferId::new(1), TransferId::new(2), Amount::new(1_000_000), 0)?;

// Formally verify that debits equal credits across all accounts
ledger.verify_invariants()?;
# Ok(())
# }
```

> **Living Documentation:** This README is compiled directly into `ledger-core`'s crate documentation via `#![doc = include_str!("../../../README.md")]`. The example above is tested in CI with `cargo test --doc`, ensuring the documentation never goes stale.

---

## Core Invariants

TrustLedger enforces invariants at the type level, backed by property tests:

* **Debits Must Equal Credits:** Total debits equal total credits for both posted and pending balances at all times ($\sum \text{Debits} \equiv \sum \text{Credits}$). Money cannot appear from nowhere or vanish.
* **Acknowledged = Fsynced:** A mutation is never confirmed to the caller until it is durably flushed to disk via `fsync`. If the power cord is pulled mid-write, crash recovery truncates the torn tail to the last intact frame.
* **No Negative Balances:** All currency math uses `u128` integer units and fixed decimal scale. Overdrafts return strongly typed `LedgerError::InsufficientFunds`. Floating-point numbers are completely barred from the codebase.
* **Deterministic Replay:** Accounts and transfers live in `BTreeMap` structures. Replaying an identical transaction journal on any machine will always produce the exact same in-memory state, byte-for-byte.
* **Verifiable On-Chain Roots:** Every settled batch commits its Merkle root to a Solana PDA using root chaining (`hash(prior_root, new_mmr_root)`), creating a permanent public audit trail.
* **Continuous 3-Way Reconciliation:** Real-time auditing proves balance parity across payment intents, internal double-entry books, and Solana on-chain state ($\text{Drift} \equiv \$0.00$).

---

## Crate Layout

The codebase is split into modular, focused crates:

| Crate | What it does |
|---|---|
| [`crates/ledger-core`](crates/ledger-core) | Pure in-memory double-entry accounting engine, balance invariants, and deterministic journal replay. |
| [`crates/wal`](crates/wal) | Append-only write-ahead log with CRC32C framing, torn-write recovery, and snapshot checkpoints. |
| [`crates/ingest`](crates/ingest) | High-concurrency gRPC ingress (`proto/ledger.proto`), bounded queue backpressure, and micro-batching. |
| [`crates/raft`](crates/raft) | 3-node OpenRaft consensus cluster with automated failover in <0.5s and split-brain immunity. |
| [`crates/merkle`](crates/merkle) | Append-only Merkle Mountain Range (MMR) generating $O(\log N)$ cryptographic inclusion proofs. |
| [`crates/solana-settle`](crates/solana-settle) | Solana BPF program for on-chain batch notarization and standalone offline verifier CLI. |
| [`crates/observability`](crates/observability) | Lock-free atomic metrics and Prometheus text exposition (`/metrics`). |
| [`crates/simulator`](crates/simulator) | Deterministic Simulation Testing (DST) harness for seeded pseudo-random chaos injection. |
| [`apps/demo`](apps/demo) | Reference client, payment lifecycle state machine, 3-way reconciliation, and single-page web UI. |

---

## Benchmarks

Throughput and latency are measured with Criterion (`benches/throughput.rs` and `benches/persistence.rs`). CI enforces automated regression gates (`scripts/bench_gate.py`) to prevent performance slips.

*Measured on Apple Silicon, `release` profile with Link-Time Optimization (LTO):*

### In-Memory Accounting Engine (`ledger-core`)

| Operation | Throughput | Latency (Median) |
|---|---|---|
| **Direct Transfer** (`create_transfer`) | **4,366,800 tx/sec** | **229 ns / op** |
| **Two-Phase Hold $\to$ Post** (Round-trip) | **1,594,800 pairs/sec** | **627 ns / op** |
| **Batch Application** (256 operations/batch) | **3,367,000 tx/sec** | **297 ns / op** |
| **Journal Replay** (`Ledger::replay`) | **4,854,000 events/sec** | **206 ns / op** |

### Write-Ahead Log Durability (`crates/wal`)

| Mode | Bandwidth | Throughput | Batch Latency |
|---|---|---|---|
| **Immediate Fsync** (Per batch) | 3.50 MB/s | ~214 batches/sec | 4.68 ms |
| **Group Commit** (Buffered + single fsync) | **259.30 MB/s** | **15,827 batches/sec** | **63.2 µs** |
| **Crash Recovery & Replay** | **1,092.93 MB/s** | **~66,700 batches/sec** | **15.0 µs** |

---

## Chaos Testing (DST)

Unit tests only test what you anticipate. To catch subtle distributed bugs and hardware failures, TrustLedger uses **Deterministic Simulation Testing** inspired by FoundationDB and TigerBeetle (`crates/simulator`):

```text
       ┌────────────────────────────────────────────────────────┐
       │     Deterministic Simulation Testing Engine (DST)      │
       │    Virtual Discrete-Event Time  •  ChaCha8 Seeded PRNG │
       └───────────────────────────┬────────────────────────────┘
                                   │
         ┌─────────────────────────┼─────────────────────────┐
         ▼                         ▼                         ▼
┌──────────────────┐      ┌──────────────────┐      ┌──────────────────┐
│ Network Chaos    │      │ Storage Faults   │      │ Node Crashes     │
│ • Drop packets   │      │ • Torn writes    │      │ • Abrupt reboots │
│ • Partitions     │      │ • Corrupt frames │      │ • Leader kills   │
│ • Latency jitter │      │ • Truncated logs │      │ • Split-brains   │
└──────────────────┘      └──────────────────┘      └──────────────────┘
                                   │
                                   ▼
          FORMAL ASSERTION: Total Wealth Conserved Across Every Tick
```

All simulated time, network message delivery, and disk failures are driven by a seeded PRNG (`rand_chacha::ChaCha8Rng`). If a test fails after 100,000 chaotic events, re-running with the exact same seed reproduces the exact bug every single time:

```bash
# Run all pre-packaged scenarios (NetworkPartition, CrashTornWrite, ChaosSoak):
cargo run -p simulator -- --scenario all --steps 200

# Fuzz 50 randomized seeds looking for edge cases:
cargo run -p simulator -- --fuzz 50 --steps 100

# Replay an exact failure deterministically:
cargo run -p simulator -- --seed 42 --scenario soak --steps 500
```

---

## Testing Matrix

| Test Suite | File | What it proves |
|---|---|---|
| **Rejection Matrix** | `crates/ledger-core/tests/unit.rs` | Double-posts, post-after-void, duplicate IDs, same-account transfers, zero amounts, and backwards timestamps are strictly rejected. |
| **Property Invariants** | `crates/ledger-core/tests/invariants.rs` | Proptest fuzzing (200 cases per property): shadow model asserts ledger state matches independent balance calculations. |
| **Byte-Boundary Crash** | `crates/wal/tests/crash.rs` | Truncates log files at **every single byte boundary**, proving recovery lands on the last verified frame without corruption. |
| **gRPC Backpressure** | `crates/ingest/tests/grpc_e2e.rs` | Full RPC lifecycle, two-phase holds, and high-concurrency burst load shedding (`RESOURCE_EXHAUSTED`). |
| **Raft Consensus** | `crates/raft/tests/partition.rs` | Network partition survival: minority isolation, majority progression, healing, and zero split-brain. |
| **Rapid Failover** | `crates/raft/tests/leader_kill.rs` | Leader killed mid-flight; asserts cluster elects new leader and resumes in <0.5s. |
| **MMR Cryptography** | `crates/merkle/tests/properties.rs` | Inclusion proofs for arbitrary leaf counts, RFC 6962 domain separation, and tamper detection. |
| **On-Chain Settlement** | `crates/solana-settle/tests/` | PDA initialization, sequential root chaining, and <4k CU on-chain verification. |
| **Reconciliation** | `apps/demo/tests/e2e.rs` | Two-phase holds, HMAC webhook security, multi-rail settlement, and continuous 3-way reconciliation ($\text{Drift} \equiv \$0.00$). |

---

## Engineering Standards

* **Zero Panics in Production:** Strictly zero `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` on production code paths.
* **Deny Unsafe Code:** Core ledger and consensus crates enforce `#![deny(unsafe_code)]`.
* **Clean Invariant Comments:** Code comments explain *why* (invariants, recovery logic, failure models), never *what* (syntax narration).
* **MSRV & Supply-Chain Checks:** Minimum Supported Rust Version (1.80) pinned and checked in CI. `cargo-deny` validates licenses (MIT / Apache-2.0 only), security advisories, and banned crates.

---

## Architecture Decision Records (ADRs)

Every major design decision is recorded under [`docs/adr/`](docs/adr/):

| ADR | Title | Decision Summary |
|---|---|---|
| [ADR-0001](docs/adr/ADR-0001-scale-and-amount-representation.md) | Scale & Amount Representation | Fixed-point `u128` integer math with scale metadata; eliminates floating-point drift. |
| [ADR-0002](docs/adr/ADR-0002-btreemap-for-deterministic-state.md) | BTreeMap for Determinism | Replaces `HashMap` with `BTreeMap` to guarantee byte-for-byte deterministic log replay. |
| [ADR-0003](docs/adr/ADR-0003-wire-and-journal-serialization.md) | Postcard for Serialization | Compact, zero-overhead binary serialization for write-ahead log frames. |
| [ADR-0004](docs/adr/ADR-0004-available-balance-semantics.md) | Non-Negative Balance Math | `available_balance` never returns negative numbers; typed `InsufficientFunds` errors. |
| [ADR-0005](docs/adr/ADR-0005-property-testing-with-proptest.md) | Property-Based Invariant Testing | Proptest shadow model verifying debits equal credits across randomized mutations. |
| [ADR-0006](docs/adr/ADR-0006-event-sourcing-and-journal-structure.md) | Event Sourcing & In-Memory State | Event-sourced mutation log separating pure in-memory state from disk I/O. |
| [ADR-0007](docs/adr/ADR-0007-wal-crash-recovery-and-framing.md) | WAL Framing & Torn-Write Recovery | CRC32C-checksummed frames with truncation to last verified record on restart. |
| [ADR-0008](docs/adr/ADR-0008-ci-benchmarks-and-supply-chain-policy.md) | Benchmarks & Supply Chain Policy | Criterion automated regression gate in CI and `cargo-deny` dependency validation. |
| [ADR-0009](docs/adr/ADR-0009-raft-consensus-and-cluster-replication.md) | Raft Distributed Consensus | 3-node OpenRaft cluster with simulated network router and <0.5s failover SLO. |
| [ADR-0010](docs/adr/ADR-0010-solana-settlement-pda-and-merkle-proofs.md) | Solana Settlement PDA & MMR Proofs | On-chain Merkle Mountain Range root commitment to Solana PDA in <4,000 CUs. |
| [ADR-0011](docs/adr/ADR-0011-three-way-continuous-reconciliation.md) | Three-Way Continuous Reconciliation | Real-time automated audit across Ingress, Double-Entry Books, and Solana L1. |
| [ADR-0012](docs/adr/ADR-0012-deterministic-simulation-testing.md) | Deterministic Simulation Testing (DST) | Discrete-event virtual time harness with seeded PRNG chaos fault injection. |

---

## Operator Runbooks

* **[WAL Runbook](docs/WAL-RUNBOOK.md)**: Handling disk exhaustion, torn-write recovery, snapshot bit rot, and emergency log truncation.
* **[Production Runbook](docs/OPERATOR-RUNBOOK.md)**: Deployment guidelines, cluster bootstrap, Prometheus alerts, and reconciliation drift triage.
* **[Failure Modes (FMEA)](docs/failure-modes.md)**: Hardware failure analysis, network partition behavior, and mitigations.
* **[Benchmarks](docs/BENCHMARKS.md)**: Detailed Criterion latency percentiles and statistical methodology.

---

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.