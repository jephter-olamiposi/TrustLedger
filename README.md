# TrustLedger

**A distributed double-entry settlement engine in Rust with on-chain (Solana) finality, crash-resilient write-ahead logging, and deterministic chaos simulation.**

[![CI Status](https://img.shields.io/github/actions/workflow/status/jephter-olamiposi/TrustLedger/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger/actions)
[![MSRV](https://img.shields.io/badge/MSRV-1.80-blue.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger)
[![License](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger/blob/main/LICENSE)
[![Safety](https://img.shields.io/badge/Unsafe-0%25-brightgreen.svg?style=flat-square)](https://github.com/jephter-olamiposi/TrustLedger)
[![Performance](https://img.shields.io/badge/Throughput->4.4M%20tx%2Fs-purple.svg?style=flat-square)](docs/BENCHMARKS.md)

[Architecture](#architecture) |
[Quickstart](#quickstart--interactive-demo) |
[System Guarantees](#system-guarantees--invariants) |
[Crate Catalog](#repository-architecture--crate-catalog) |
[Performance](#performance--benchmarks) |
[Deterministic Simulation](#deterministic-simulation-testing-dst) |
[Production Discipline](#production-engineering-standards) |
[ADRs](docs/adr/)

---

## Executive Summary

Financial ledgers are traditionally deployed on general-purpose relational SQL databases. Under high concurrency, this design exposes critical vulnerabilities: lock contention on hot merchant balances, vulnerability to silent balance drift during uncoordinated distributed transactions, data loss from torn writes during abrupt power cuts, and zero external cryptographic verifiability for auditors.

**TrustLedger** is an institutional-grade, event-sourced distributed settlement engine built from scratch in Rust. It combines:

1. **In-Memory Accounting Kernel (`ledger-core`)**: Pure deterministic state machine executing **>4.4 million transfers/sec** with mathematical balance conservation ($\sum \text{Debits} \equiv \sum \text{Credits}$).
2. **TigerBeetle-Style Durability (`wal`)**: Append-only, CRC32C-checksummed Write-Ahead Log with group fsync and byte-boundary torn-write crash recovery.
3. **Consensus & Fault Tolerance (`raft`)**: 3-node OpenRaft consensus cluster with automated failover in <0.5s and verified split-brain immunity under network partitions.
4. **Micro-Batched Ingress (`ingest`)**: Tonic/gRPC streaming ingress with bounded non-blocking queues and instant backpressure load shedding (`RESOURCE_EXHAUSTED`).
5. **Cryptographic Solana L1 Finality (`merkle` + `solana-settle`)**: Incremental Merkle Mountain Range (MMR) accumulator committing batch roots to a Solana Program Derived Address (PDA), producing $O(\log N)$ portable cryptographic inclusion receipts verifiable in <4,000 Compute Units.
6. **Continuous 3-Way Reconciliation (`apps/demo`)**: Real-time automated auditor ensuring absolute parity ($\text{Drift} \equiv \$0.00$) across payment ingress, double-entry books, and on-chain state.
7. **Deterministic Simulation Testing (`simulator`)**: FoundationDB/TigerBeetle-style discrete-event chaos harness driving network partitions, torn writes, and crashes via seeded PRNG with 100% replayable determinism.

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
                                            │   Offline Verifier CLI (<4k CU│
                                            └───────────────────────────────┘
```

---

## Quickstart & Interactive Demo

### 1. Launch the Operations Console

TrustLedger includes an interactive, single-page enterprise operations console (built with Axum, Tailwind CSS, and Server-Sent state updates) modeling real-world payment flows:

```bash
# Build and start the demo server on port 8080:
cargo run -p demo --bin demo
```

Open your browser to:
```text
http://127.0.0.1:8080/
```

Inside the console, you can interactively:
- **Run the 1-Click Pipeline**: Authorize a $50.00 card hold, capture settled funds to the merchant, commit to the Solana PDA, and inspect the mathematical Merkle inclusion proof.
- **Experience Two-Phase Card Holds**: Execute instant payments or place funds in escrow, with real-time balance validation.
- **Test Continuous 3-Way Reconciliation**: Inject a simulated $50.00 bank discrepancy and trigger automated self-healing.
- **Trigger In-Modal Solana L1 Anchor**: Commit queued transfers directly from within the digital payment receipt modal.
- **Execute Fault Injection**: Run TigerBeetle DST chaos tests (Network Partitions, Torn Writes, Chaos Soak) directly from the browser.

### 2. Standalone Offline Receipt Verifier CLI

Any customer, partner bank, or external auditor can cryptographically verify payment finality against the Solana blockchain without trusting the ledger operator:

```bash
# Verify a portable transfer receipt JSON:
cargo run -p solana-settle --bin verifier -- --receipt path/to/receipt.json

# Or verify raw cryptographic parameters directly:
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

Deploy a 3-node Raft consensus cluster with Prometheus metrics collection:

```bash
# Launch 3-node cluster and Prometheus telemetry scraper:
docker compose -f docker/docker-compose.yml up -d

# Inspect live Prometheus exposition metrics:
curl -s http://localhost:8080/metrics | grep trustledger_
```

---

## Core Code Example

The core ledger engine is pure, thread-safe, and zero-allocation on validation errors. Here is a two-phase transfer (Authorization Hold $\to$ Capture) written against `ledger-core`:

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

// Phase 1: Place authorization hold ($1.00 USDC = 1,000,000 atomic units)
let hold = Transfer::new_pending(TransferId::new(1), vault, alice, Amount::new(1_000_000), 0)?;
ledger.create_pending(hold)?;

// Phase 2: Post the hold to capture settled funds to Alice's balance
ledger.post_pending(TransferId::new(1), TransferId::new(2), Amount::new(1_000_000), 0)?;

// Formally verify that debits equal credits across all accounts
ledger.verify_invariants()?;
# Ok(())
# }
```

> **Single Source of Truth**: This README is compiled directly as the crate-level documentation for `ledger-core` (`#![doc = include_str!("../../../README.md")]`). The code example above is verified as a compiled doctest (`cargo test --doc`).

---

## System Guarantees & Invariants

TrustLedger enforces invariants at the type level, verified by formal property suites:

### 1. Mathematical Balance Conservation
Across every mutation, total debits equal total credits for both posted and pending balances:
$$\sum \text{Debits}_{\text{posted}} \equiv \sum \text{Credits}_{\text{posted}} \quad \text{and} \quad \sum \text{Debits}_{\text{pending}} \equiv \sum \text{Credits}_{\text{pending}}$$
Money can neither be created nor destroyed. Invariant verification runs in $O(A)$ time and is asserted after every batch execution.

### 2. Acknowledged Means Fsynced (Zero Data Loss)
Every mutation is written to the Write-Ahead Log before returning success to the client. Each frame contains a 4-byte magic identifier (`LETW`), version, monotonically incrementing 64-bit sequence number, 32-bit CRC32C payload checksum, and length-prefixed payload. Crash recovery sweeps the file, detects torn writes from abrupt power cuts, truncates uncommitted tails back to the last verified record boundary, and hard-fails on sequence gaps. Recovery is tested across **every single byte boundary** of the log file.

### 3. Zero Overdraft & Scaled Decimal Safety
Balances use non-negative `u128` integers scaled by fixed precision (`Scale`, e.g., 6 decimals for USDC). All math uses `checked_add` and `checked_sub`. Accounts can never drop below zero; attempts to overdraw return strongly typed `LedgerError::InsufficientFunds`. Floating-point arithmetic is strictly forbidden throughout the entire codebase.

### 4. Deterministic Replay
All accounts and transfers are indexed in `BTreeMap` structures (ADR-0002). Replaying an identical sequence of journal events on any machine produces byte-for-byte identical state. Monotonic timestamp verification prevents out-of-order execution and time-travel anomalies.

### 5. Cryptographic Non-Repudiation (Solana L1)
Settled transfers are organized into an incremental Merkle Mountain Range (MMR). Batch roots are submitted via CPI to a Solana Program Derived Address (PDA). The program chains roots (`hash(prior_root, new_mmr_root)`), creating an immutable, non-repudiable audit trail anchored in Solana blockchain consensus.

### 6. Continuous 3-Way Reconciliation
TrustLedger continuously audits parity across three independent ledgers:
$$\text{Payment Gateway Intent} \equiv \text{Double-Entry Books} \equiv \text{Solana On-Chain State}$$
Any divergence immediately triggers automated incident logging, Prometheus alerting, and safe self-healing.

---

## Repository Architecture & Crate Catalog

The repository is organized into a modular workspace:

| Crate / Path | Responsibility | Key Technologies & Patterns |
|---|---|---|
| [`crates/ledger-core`](crates/ledger-core) | Core double-entry accounting kernel | Pure state machine, `u128` scaled math, `BTreeMap` determinism, proptest |
| [`crates/wal`](crates/wal) | Append-only write-ahead log & snapshotting | CRC32C checksums, torn-write truncation, $O(1)$ streaming reader, group fsync |
| [`crates/ingest`](crates/ingest) | High-throughput network ingress & backpressure | Tonic gRPC, Protobuf (`proto/ledger.proto`), bounded MPSC, single-writer batching |
| [`crates/raft`](crates/raft) | Distributed 3-node consensus cluster | OpenRaft (`storage-v2`), simulated network router, leader failover <0.5s |
| [`crates/merkle`](crates/merkle) | Merkle Mountain Range (MMR) proof engine | Append-only MMR, RFC 6962 domain separation, $O(\log N)$ branch proofs |
| [`crates/solana-settle`](crates/solana-settle) | Solana settlement program & verifier CLI | BPF on-chain program, PDA notary, root chaining, <4k CU verification |
| [`crates/observability`](crates/observability) | Lock-free metrics & Prometheus telemetry | Atomic counters/gauges, fixed-bucket histograms, Prometheus 2.0 text format |
| [`crates/simulator`](crates/simulator) | Deterministic Simulation Testing (DST) | FoundationDB/TigerBeetle DST harness, virtual time ticks, ChaCha8 PRNG chaos |
| [`apps/demo`](apps/demo) | Reference client, payment engine & UI | Axum HTTP server, Stripe-style dashboard, HMAC webhooks, 3-way reconciliation |

---

## Performance & Benchmarks

Hot-path throughput is measured using Criterion (`benches/throughput.rs` and `benches/persistence.rs`). Automated regression gates enforce performance budgets in CI (`scripts/bench_gate.py` fails the build if any median regresses beyond baseline bounds, ADR-0008).

### In-Memory Accounting Engine (`ledger-core`)
*Measured on Apple Silicon, `release` profile with Link-Time Optimization (LTO):*

| Operation | Throughput | Latency (Median) |
|---|---|---|
| **`create_transfer`** (Direct transfer) | **4,366,800 tx/sec** | **229 ns / op** |
| **`two-phase round-trip`** (Hold $\to$ Post) | **1,594,800 pairs/sec** | **627 ns / op** |
| **`apply_batch`** (256 operations / batch) | **3,367,000 tx/sec** | **297 ns / op** |
| **`Ledger::replay`** (Journal reconstruction) | **4,854,000 events/sec** | **206 ns / op** |

### Durability & Persistence Layer (`crates/wal`)
*Measured over 10,000 batches $\times$ 16 KiB payloads with direct I/O:*

| Mode | Bandwidth | Throughput | Batch Latency |
|---|---|---|---|
| **Fsync per batch** (Immediate disk sync) | 3.50 MB/s | ~214 batches/sec | 4.68 ms |
| **Group Commit** (Buffered + single sync) | **259.30 MB/s** | **15,827 batches/sec** | **63.2 µs** |
| **Recovery & Log Replay** | **1,092.93 MB/s** | **~66,700 batches/sec** | **15.0 µs** |

---

## Deterministic Simulation Testing (DST)

Traditional integration testing fails to uncover subtle distributed race conditions, split-brain scenarios, or disk corruption edge cases. TrustLedger adopts FoundationDB and TigerBeetle-style **Deterministic Simulation Testing** (`crates/simulator`):

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

By virtualizing time into discrete ticks and driving all scheduling decisions through a seeded PRNG (`rand_chacha::ChaCha8Rng`), any failure encountered across millions of state transitions can be replayed on an engineer's laptop with 100% determinism:

```bash
# Run all pre-packaged scenarios (NetworkPartition, CrashTornWrite, ChaosSoak):
cargo run -p simulator -- --scenario all --steps 200

# Fuzz 50 randomized simulation seeds exploring edge-case partitions:
cargo run -p simulator -- --fuzz 50 --steps 100

# Replay an exact failure scenario deterministically:
cargo run -p simulator -- --seed 42 --scenario soak --steps 500
```

---

## Testing & Verification Matrix

TrustLedger maintains an exhaustive testing matrix across all layers:

| Test Suite | File / Crate | Scope & Guarantees Proven |
|---|---|---|
| **Unit Matrix** | `crates/ledger-core/tests/unit.rs` | Exhaustive rejection matrix: double-post, post-after-void, duplicate IDs, same-account transfers, zero amounts, closed accounts, backwards timestamps. |
| **Property Suite** | `crates/ledger-core/tests/invariants.rs` | Proptest fuzzing (200 cases per property): shadow model asserts ledger equals independent computation after every state transition. |
| **Byte-Boundary Crash** | `crates/wal/tests/crash.rs` | Sweeps log files truncated at **every single byte boundary**, proving recovery lands exactly on the last intact frame. |
| **gRPC & Backpressure** | `crates/ingest/tests/grpc_e2e.rs` | End-to-end gRPC RPC suite, two-phase holds, and high-concurrency burst load-shedding (`RESOURCE_EXHAUSTED`). |
| **Jepsen Consensus** | `crates/raft/tests/partition.rs` | 3-node partition simulation: minority isolation, majority progression, partition healing, log truncation, zero split-brain. |
| **Rapid Failover** | `crates/raft/tests/leader_kill.rs` | Unannounced leader kill; asserts cluster elects new leader and resumes replication in <0.5s. |
| **MMR Cryptography** | `crates/merkle/tests/properties.rs` | Arbitrary leaf count inclusion proofs, RFC 6962 domain separation, tamper detection. |
| **On-Chain Settlement** | `crates/solana-settle/tests/` | PDA initialization, sequential batch commitments with root chaining, <4k CU on-chain verification. |
| **Reconciliation & Rails**| `apps/demo/tests/e2e.rs` | Payment lifecycle, HMAC card webhooks with anti-replay, multi-rail settlement, continuous 3-way reconciliation ($\text{Drift} \equiv \$0.00$). |

---

## Production Engineering Standards

TrustLedger is written to senior production engineering standards:

* **Zero Panics in Production**: Strictly **zero** `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` on production code paths. All errors are mapped into strongly typed domain error enums.
* **Forbid Unsafe Code**: The entire core ledger and consensus engine enforce `#![deny(unsafe_code)]`.
* **Clean Code & Invariant Hygiene**: Every public item and enum variant is documented. Comments explain *why* (invariants, recovery rationale, failure modes), never *what* (syntax narration).
* **MSRV & Supply-Chain Security**: Pinned Minimum Supported Rust Version (MSRV 1.80) enforced in CI. `cargo-deny` audits licenses (MIT / Apache-2.0 only), security advisories, and banned crates on every pull request.
* **Pre-Push Quality Gate**: Run `./scripts/check` to execute `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the full test suite, benchmarks, and regression gates before merging.

---

## Architecture Decision Records (ADRs)

Every major architectural choice is documented as an Architecture Decision Record under [`docs/adr/`](docs/adr/):

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

## Operator Runbooks & Documentation

* **[WAL Operations Runbook](docs/WAL-RUNBOOK.md)**: Handling disk saturation, torn-write recovery, snapshot corruption, and emergency log truncation.
* **[Production Operator Runbook](docs/OPERATOR-RUNBOOK.md)**: Deployment guidelines, cluster bootstrap, Prometheus alert rules, and reconciliation drift triage.
* **[Failure Modes & Effects Analysis (FMEA)](docs/failure-modes.md)**: Comprehensive architectural risk matrix detailing hardware failures, partitions, and mitigations.
* **[Benchmark Methodology & Results](docs/BENCHMARKS.md)**: Detailed Criterion statistical analysis, latency percentiles, and hardware specifications.

---

## License

This project is dual-licensed under either:

* **[MIT License](LICENSE-MIT)**
* **[Apache License, Version 2.0](LICENSE-APACHE)**

at your option.