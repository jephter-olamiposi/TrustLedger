# TrustLedger

A Rust financial ledger built around double-entry accounting, durable writes, replicated state, and cryptographic settlement commitments.

TrustLedger is an engineering project for exploring what happens when a financial ledger has to care about more than moving numbers between accounts.

The core keeps accounting deterministic and explicit. Money is represented with fixed-point integer amounts, transfers follow a defined lifecycle, and every mutation is checked against balance and conservation invariants.

Around that core, the project adds a durable write-ahead log, bounded ingress with micro-batching, a 3-node OpenRaft path, cryptographic batch commitments with a Merkle Mountain Range, a Solana settlement program, reconciliation, observability, and deterministic failure testing.

The goal is simple: make the important failure cases visible instead of assuming they will never happen.

---

## What TrustLedger models

A payment is not always a single transfer.

A common lifecycle is:

```text
Authorization
     │
     ▼
 Pending hold
     │
     ├──────────────► Voided
     │
     ▼
  Captured
     │
     ▼
 Settled batch
     │
     ▼
 MMR root
     │
     ▼
 Solana commitment
```

The ledger models that lifecycle directly.

A pending transfer reserves funds. It can later be posted, including a partial capture, or voided to release the reservation. Once posted or voided, the pending transfer is terminal.

---

## Why the core is designed this way

Financial state is a poor place for implicit rules.

TrustLedger makes several of those rules explicit in the data model and execution path:

* Monetary values use `Amount(u128)` with an explicit decimal `Scale`; floating-point values are not used for ledger arithmetic.
* Accounts have explicit accounting types such as asset, liability, equity, revenue, and expense.
* Posted and pending debits and credits are tracked separately.
* Transfers cannot debit and credit the same account or use a zero amount.
* Pending transfers can only move to Posted or Voided.
* The ledger checks conservation of posted and pending debits and credits and verifies that pending balances match the transfer index.
* Mutations use a compute-then-write approach. Batch preparation uses an undo log so speculative state can be rolled back without cloning the whole ledger.

The result is a small accounting core that is easy to replay and reason about.

### Code example

Here is how an authorization hold and capture run against `ledger-core`:

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

---

## Architecture

TrustLedger currently has two main execution paths around the same ledger core.

```text
                         CLIENT REQUESTS
                               │
                               ▼
                    ┌─────────────────────┐
                    │ Bounded Ingress     │
                    │ Tokio mpsc          │
                    │ Backpressure        │
                    └──────────┬──────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │ Single-Writer       │
                    │ Ingest Engine       │
                    │ Micro-batching      │
                    └──────────┬──────────┘
                               │
                     prepare → persist
                               │
                               ▼
                    ┌─────────────────────┐
                    │ Write-Ahead Log     │
                    │ CRC32C frames       │
                    │ fsync/group commit  │
                    └──────────┬──────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │ ledger-core         │
                    │ deterministic state │
                    │ machine             │
                    └──────────┬──────────┘
                               │
                               │
              ┌────────────────┴────────────────┐
              │                                 │
              ▼                                 ▼
    OpenRaft replicated path          Settlement / audit path
              │                                 │
              ▼                                 ▼
     3-node state machine             MMR over settled transfers
              │                                 │
              │                                 ▼
              │                       Solana settlement program
              │                                 │
              │                                 ▼
              │                         Transfer receipts
              │                                 │
              └────────────────┬────────────────┘
                               ▼
                         Reconciliation
```

The production-style ingress path is intentionally single-writer: a bounded Tokio queue feeds an engine that accumulates commands for up to 512 operations or 2 ms, prepares them against the ledger, persists the resulting events to the WAL with one group commit, and then commits them to memory.

The repository also contains a 3-node OpenRaft implementation whose committed entries are applied sequentially to the same ledger core. The current Raft log store is an in-memory `MemLogStore`; it is intentionally separate from the file-backed WAL used by the ingest engine.

---

## 1. Double-entry ledger

The `ledger-core` crate is the centre of the project.

At its lowest level:

* Asset
* Liability
* Equity
* Revenue
* Expense

are explicit account types, while balances keep posted and pending amounts separate.

A transfer is represented as:

```text
Pending  ─────► Posted
    │
    └──────────► Voided
```

Only the legal transitions are accepted by the ledger.

The ledger also keeps an append-only journal of account and transfer events. That journal is the input used to rebuild state through deterministic replay.

### Atomic batch preparation

Batches are first applied speculatively.

The ledger records the pre-image of each affected account or transfer, applies operations against live state, and rolls back the speculative changes if an operation fails. The resulting journal events can then be persisted and committed.

This avoids taking a full copy of the ledger just to get rollback semantics.

---

## 2. Durable write-ahead log

The `wal` crate is an append-only file format designed around a simple guarantee:

> An acknowledged record must survive a crash when synchronous persistence is enabled.

Each record is framed as:

```text
magic | version | flags | sequence | payload length | payload | CRC32C
```

The scanner verifies the header, payload length, and checksum for every frame. If the file ends with a torn or corrupt frame, recovery keeps the verified prefix and truncates the damaged tail.

The WAL also supports batched appends. Multiple prepared payloads are encoded into one buffer and committed with a single `sync_data()` call, which makes the durability boundary explicit while avoiding one filesystem sync per transfer.

Recovery is streamed rather than loading the entire log into memory.

---

## 3. Backpressure and micro-batching

A financial system should not respond to overload by allocating memory until the process falls over.

Ingress uses a bounded Tokio `mpsc` queue. When the queue is full, a mutation is rejected immediately with `QueueSaturated` rather than waiting indefinitely or growing the queue.

The engine then collects incoming mutations into short micro-batches:

* **Max batch size:** 512
* **Max batch delay:** 2 ms

Each batch is prepared against the ledger, encoded as journal events, persisted through the WAL, and then committed.

This separates three concerns that are easy to mix together:

```text
admission control
      │
      ▼
batch formation
      │
      ▼
durable commit
```

---

## 4. Replicated state with OpenRaft

The `raft` crate provides a 3-node replicated state-machine path.

A cluster is bootstrapped with three OpenRaft nodes and supports leader discovery, proposals, node isolation, partitions, healing, and node shutdown.

The state machine applies committed Raft entries sequentially to `ledger-core`. Snapshots contain the ledger scale and journal and can rebuild the ledger by replaying those events.

The test transport is deliberately in-memory. That makes it possible to inject partitions and node isolation without depending on an external network.

---

## 5. Cryptographic settlement commitments

The settlement layer uses an incremental Merkle Mountain Range.

Transfers are hashed into leaves, internal nodes are domain-separated, and the current peaks are folded into a single root:

```text
transfer
   │
   ▼
SHA-256 leaf
   │
   ▼
MMR
   │
   ▼
Merkle root
```

The hash scheme uses separate prefixes for leaves, internal nodes, and peak bagging:

```text
leaf  = SHA256(0x00 || data)
node  = SHA256(0x01 || left || right)
peak  = SHA256(0x02 || left_peak || right_peak)
```

This gives incremental append behaviour and logarithmic inclusion proofs.

### Solana settlement program

`crates/solana-settle` contains the Solana program that stores the settlement commitment in a PDA.

The PDA state contains:

* authority
* epoch
* batch sequence
* current MMR root
* previous root
* transfer count
* cumulative settled amount
* chain tip
* settlement timestamp
* bump

Each committed batch must advance the sequence number and, after the first batch, reference the previous root.

The program also exposes an inclusion-verification instruction that checks an MMR proof against the root stored in the PDA.

### What the commitment proves

A valid receipt proves that a transfer leaf is included in the committed MMR root.

It does not by itself prove that the off-chain source data was honest before the root was created. The root is a cryptographic commitment to the dataset supplied by the settlement process.

That distinction is important.

### Transfer receipts

The project can produce a portable JSON receipt containing the transfer identifiers, leaf hash, settlement batch, committed root, and MMR proof.

The standalone verifier can check that proof without running the ledger:

```bash
cargo run -p solana-settle --bin verifier -- \
  --receipt path/to/receipt.json
```

A successful verification exits with code 0.

*Note on runtime context:* The demo settlement rail currently exercises the Solana program locally through Solana `AccountInfo` structures rather than submitting a real network transaction to a Solana RPC endpoint.

---

## 6. Reconciliation

The demo application keeps a separate payment model from the ledger.

A payment can move through:

```text
Authorized
    │
    ├──► Voided
    │
    ▼
Captured
    │
    ▼
Refunded
```

The payment record keeps references to the corresponding ledger transfers and the settlement batch.

The reconciliation engine compares:

```text
application captured total
            │
            ├── ledger merchant liability total
            │
            └── on-chain settlement total
```

A mismatch becomes a structured reconciliation incident rather than being silently ignored.

---

## 7. Deterministic failure testing

Traditional unit tests are useful, but they usually explore the failures the author remembered to write down.

TrustLedger also contains a deterministic simulation harness.

The simulator controls:

* virtual time
* seeded randomness
* packet delay
* packet loss
* duplicate packets
* network partitions
* node crashes
* simulated storage failures
* recovery and catch-up

The simulated cluster models proposals, acknowledgements, commits, leader changes, and catch-up while the oracle checks financial conservation and replicated-log agreement.

The important property is reproducibility.

A simulation run is identified by a seed:

```bash
cargo run -p simulator -- \
  --seed 42 \
  --scenario all \
  --steps 200
```

A failing run can be repeated with the same seed and scenario. The CLI also supports fuzzing across consecutive seeds.

---

## Performance

The latest recorded benchmark run is documented in [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

### Ledger core

| Workload | Throughput | Median Latency |
|---|---|---|
| Immediate transfer | ~4.4M transfers/sec | ~229 ns/op |
| Pending + post | ~1.6M pairs/sec | ~627 ns/op |
| apply_batch (256 transfers) | ~3.4M transfers/sec | ~297 ns/op |
| Journal replay | ~4.9M events/sec | ~206 ns/event |

### Write-Ahead Log (WAL)

| Workload | Throughput |
|---|---|
| fsync per batch | ~4.5 MB/s |
| buffered writes | ~660–780 MB/s |
| recovery + replay | ~0.5–1.3 GB/s |

### Cryptographic path

| Operation | Recorded result |
|---|---|
| MMR append | ~820 ns/leaf |
| MMR root, 1,024 leaves | ~0.84 ms |
| Inclusion proof generation | ~4.2 µs |
| Proof verification | ~3.6 µs |

*These numbers are measurements from the repository's benchmark harnesses, not capacity claims for a production deployment.*

---

## Failure testing results

The deterministic simulation benchmark currently records:

| Scenario | Result |
|---|---|
| Network partition | Passed |
| Leader crash + torn-write recovery | Passed |
| Chaos soak with packet loss and duplicates | Passed |
| 100-seed fuzz campaign | 0 recorded invariant violations |

The simulation harness checks wealth conservation and replicated-log agreement while injecting faults into the model.

---

## Workspace

```text
TrustLedger/
├── crates/
│   ├── ledger-core/       # Double-entry accounting state machine
│   ├── wal/               # Append-only durable log
│   ├── ingest/            # Bounded ingress + micro-batching
│   ├── raft/              # OpenRaft replicated state machine
│   ├── merkle/            # Merkle Mountain Range + proofs
│   ├── solana-settle/     # Solana settlement program + verifier
│   └── observability/     # Metrics + Prometheus exposition
│
├── apps/
│   └── demo/              # Payment flow, settlement rail, reconciliation, UI
│
├── simulator/             # Deterministic distributed-systems simulator
│
├── docs/
│   ├── adr/               # Architecture decisions
│   ├── BENCHMARKS.md      # Benchmark methodology and results
│   ├── failure-modes.md   # Failure-mode analysis
│   └── OPERATOR-RUNBOOK.md
│
└── docker/                # Local multi-node/demo environment
```

The workspace is split so that the accounting core, persistence, networking, consensus, cryptographic settlement, application demo, and simulation can be tested independently.

---

## Quick start

### Run the demo
```bash
cargo run -p demo --bin demo
```

Then open: `http://127.0.0.1:8080/`

The demo exposes the payment lifecycle, settlement flow, reconciliation, cryptographic verification, and simulation controls.

### Run the test suite
```bash
cargo test --workspace
```

### Run formatting and lint checks
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

### Run the deterministic simulator
```bash
cargo run -p simulator -- \
  --scenario all \
  --seed 42 \
  --steps 200
```

### Run the verifier
```bash
cargo run -p solana-settle --bin verifier -- \
  --receipt path/to/receipt.json
```

### Run the local cluster environment
```bash
docker compose -f docker/docker-compose.yml up -d
```

---

## Engineering constraints

TrustLedger deliberately keeps several constraints visible in the repository:

* **Deterministic money:** Ledger amounts use integer base units with an explicit scale. Arithmetic is checked and returns typed errors on overflow or invalid operations.
* **Deterministic execution:** The ledger uses ordered maps and a single-writer execution model so that the same event history can be replayed into the same state.
* **Durable acknowledgement:** The WAL's synchronous mode does not acknowledge the append until the batch has reached the filesystem sync boundary.
* **Backpressure:** The ingress queue is bounded and rejects work when saturated instead of allowing unbounded buffering.
* **Unsafe code:** The workspace denies unsafe code, and the observability crate explicitly forbids it.
* **CI quality gates:** CI checks formatting, clippy with warnings denied, the full workspace test suite, benchmarks, benchmark regression, documentation generation, the Rust 1.80 MSRV, and dependency licensing/source constraints.

---

## Trade-offs

TrustLedger is intentionally opinionated. The main trade-offs are:

1. **Single-writer execution:** Simplifies ordering and removes SQL row-lock contention from the accounting hot path, but writes must pass through that execution model.
2. **File-backed WAL:** Gives clear durability semantics, but synchronous filesystem commits have real physical latency.
3. **OpenRaft replication:** Adds failure tolerance and ordering at the cost of network coordination.
4. **MMR state:** Gives cheap incremental appends and small inclusion proofs, but retaining the full node structure uses memory as the number of leaves grows.
5. **Off-chain accounting with on-chain commitments:** Keeps the ledger fast and practical while making selected settlement state externally auditable, but the chain commitment is still a commitment to off-chain data rather than an independent re-execution of every debit and credit.

---

## Further reading

The repository keeps the deeper reasoning separate from the README:

* [Core ledger ADR](docs/adr/ADR-0001-scale-and-amount-representation.md)
* [WAL persistence ADR](docs/adr/ADR-0007-wal-crash-recovery-and-framing.md)
* [Distributed consensus ADR](docs/adr/ADR-0009-raft-consensus-and-cluster-replication.md)
* [MMR and Solana settlement ADR](docs/adr/ADR-0010-solana-settlement-pda-and-merkle-proofs.md)
* [Benchmarks](docs/BENCHMARKS.md)
* [Failure modes (FMEA)](docs/failure-modes.md)
* [Operator runbook](docs/OPERATOR-RUNBOOK.md)

---

## Status

TrustLedger is an engineering project focused on financial correctness, durability, distributed-state behaviour, cryptographic commitments, and failure testing.

It is intentionally designed so that the important properties can be inspected in code, tested, benchmarked, and reproduced.