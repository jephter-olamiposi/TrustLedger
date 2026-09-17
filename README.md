# TrustLedger

TrustLedger is a Rust financial ledger built around double-entry accounting, durable writes, replicated state, and cryptographic settlement commitments.

## The problem

Moving money is not only an accounting problem. A ledger must also answer difficult questions when something goes wrong:

- What survives if the process crashes during a write?
- Can the same journal be replayed into the same balances after restart?
- What happens when a node is partitioned from the cluster?
- How do we detect drift between payment state, ledger state, and settlement state?
- How can another system verify that a transfer was included in a settled batch?

A happy-path transfer function does not answer those questions. Financial state needs explicit invariants, durable recovery, controlled admission under load, and an audit trail that can be verified independently.

## Why I built it

I built TrustLedger to study those boundaries in one working system rather than as isolated examples. The project uses a small deterministic ledger core as the source of truth, then surrounds it with the storage, consensus, settlement, reconciliation, and simulation layers needed to exercise realistic failure cases.

The goal is not to claim that a portfolio project is a finished payment processor. The goal is to make the important properties visible in code: money conservation, legal transfer transitions, crash recovery, quorum-based replication, settlement proofs, and reproducible failure tests.

## What it contains

The core keeps accounting deterministic and explicit. Money is represented with fixed-point integer amounts, transfers follow a defined lifecycle, and every mutation is checked against balance and conservation invariants.

Around that core, the project adds a durable write-ahead log, bounded ingress with micro-batching, a 3-node OpenRaft path, cryptographic batch commitments with a Merkle Mountain Range, a Solana settlement program, reconciliation, observability, and deterministic failure testing.

The rest of this README explains how those pieces fit together and what each one proves.

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

A pending transfer is a temporary hold on funds. It can later be posted, including as a partial capture, or voided to release the hold. After it is posted or voided, it cannot change again.

---

## Why the core is designed this way

Financial state is a poor place for hidden rules. The ledger should make it clear what happened, what is allowed next, and whether money was conserved.

TrustLedger makes those rules explicit in the data model and execution path:

* Money uses integer base units and an explicit decimal scale instead of floating-point numbers.
* Accounts have clear accounting types such as asset, liability, equity, revenue, and expense.
* Posted money and temporarily held money are tracked separately.
* A transfer cannot move money from an account back to itself or use a zero amount.
* A pending transfer can only become `Posted` or `Voided`.
* The ledger checks that every debit and credit is balanced and that held balances match the transfers that created them.
* A batch is calculated before it is permanently written. If the calculation fails, an undo log restores the earlier state without copying the entire ledger.

The result is a compact accounting core that can be replayed and checked after a restart.

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

The ingress path is intentionally single-writer: one engine applies ledger changes in a known order. A bounded Tokio queue limits how much work can wait in memory, and the engine groups up to 512 operations or 2 ms of work before writing them together.

The repository also contains a 3-node OpenRaft implementation. Once a change is agreed by the cluster, each node applies it to the same ledger core in the same order. The current Raft log store is an in-memory `MemLogStore`; it is separate from the file-backed WAL used by the ingest engine.

---

## 1. Double-entry ledger

The `ledger-core` crate is the centre of the project. It owns the accounting rules and is the place where balances are changed.

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

The ledger also keeps an append-only journal of account and transfer events. The journal is a history of what happened, so the system can rebuild the same balances by replaying it in order.

### Atomic batch preparation

Batches are first tested speculatively, meaning the ledger tries the whole batch before accepting it permanently.

The ledger records the previous value of each affected account or transfer, applies the operations, and restores those previous values if one operation fails. Only a successful batch produces journal events that are written and committed.

This provides rollback without copying the entire ledger for every batch.

---

## 2. Durable write-ahead log

The `wal` crate is the ledger's durable journal. It is an append-only file designed around a simple guarantee:

> An acknowledged record must survive a crash when synchronous persistence is enabled.

Each record is framed as:

```text
magic | version | flags | sequence | payload length | payload | CRC32C
```

Each record has a header, the saved data, and a CRC32C checksum. On restart, the scanner checks every record. If a crash left a partial record at the end, recovery keeps the verified history and removes only the damaged tail.

The WAL also supports batched appends. Several prepared payloads can be written together and committed with one `sync_data()` call, reducing the cost of syncing the filesystem for every individual transfer.

Recovery reads the file a piece at a time instead of loading the entire log into memory.

---

## 3. Backpressure and micro-batching

A financial system should not respond to heavy traffic by accepting unlimited work until the process runs out of memory.

Ingress uses a bounded Tokio `mpsc` queue. When the queue is full, a new mutation fails immediately with `QueueSaturated` instead of waiting forever or growing memory without a limit.

The engine then collects incoming mutations into short micro-batches, which means several small requests are processed together:

* **Max batch size:** 512
* **Max batch delay:** 2 ms

Each batch is prepared against the ledger, encoded as journal events, persisted through the WAL, and then committed.

This separates three decisions:

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

The `raft` crate provides a 3-node replicated state-machine path. In plain terms, the nodes agree on the order of changes before applying them.

A cluster starts with three OpenRaft nodes and supports leader discovery, proposals, node isolation, network partitions, recovery after healing, and node shutdown.

The state machine applies approved Raft entries one at a time to `ledger-core`. Snapshots contain the ledger scale and journal, allowing a node to rebuild its state without processing every network message again.

The test network runs in memory. This makes it possible to simulate partitions and isolated nodes without needing three real servers.

---

## 5. Cryptographic settlement commitments

The settlement layer uses an incremental Merkle Mountain Range, or MMR. An MMR groups transfer hashes into a compact tree that can be extended as new transfers are settled.

Each transfer becomes a SHA-256 leaf. Different prefixes are used for leaves, internal nodes, and the final peak combination so the same bytes cannot be confused as different kinds of tree data. The current peaks are folded into one root:

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

This allows new transfers to be added efficiently and produces small inclusion proofs. An inclusion proof lets someone check that a particular transfer belongs to a committed batch without downloading the entire batch.

### Solana settlement program

`crates/solana-settle` contains the Solana program that stores the settlement commitment in a PDA, a program-owned account on Solana.

The account stores:

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

Each committed batch must advance the sequence number and, after the first batch, refer to the previous root. This links batches into a verifiable chain.

The program also exposes an instruction that checks an MMR proof against the root stored in the account.

### What the commitment proves

A valid receipt proves that a transfer's hash is included in the committed MMR root.

It does not prove that the off-chain data was honest before the root was created. It proves that the transfer is included in the exact dataset that the settlement process committed.

That distinction is important.

### Transfer receipts

The project can produce a portable JSON receipt containing the transfer IDs, transfer hash, settlement batch, committed root, and inclusion proof.

The standalone verifier can check that proof without running the ledger:

```bash
cargo run -p solana-settle --bin verifier -- \
  --receipt path/to/receipt.json
```

A successful verification exits with code 0.

*Note on runtime context:* The demo settlement rail currently exercises the Solana program locally through Solana `AccountInfo` structures rather than submitting a real network transaction to a Solana RPC endpoint.

---

## 6. Reconciliation

The demo application keeps a payment record alongside the ledger. The two models are linked by transfer IDs and settlement batch IDs.

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

The payment record keeps references to the corresponding ledger transfers and settlement batch.

The reconciliation engine compares three views of the same money:

```text
application captured total
            │
            ├── ledger merchant liability total
            │
            └── on-chain settlement total
```

If the totals do not match, the application creates a reconciliation incident instead of silently accepting the difference.

---

## 7. Deterministic failure testing

Traditional unit tests are useful, but they usually cover only the failure cases someone explicitly wrote as tests.

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

The simulated cluster models proposals, acknowledgements, commits, leader changes, and catch-up. At each step, checks confirm that money is conserved and that the active nodes agree on their committed history.

The important property is reproducibility: the same seed produces the same failure schedule.

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

The recorded benchmark results are documented in [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). They are reference measurements, not promises about production capacity.

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
│   ├── decisions/         # Design decisions
│   ├── BENCHMARKS.md      # Benchmark methodology and results
│   ├── failure-modes.md   # Failure-mode analysis
│   └── OPERATOR-RUNBOOK.md
│
└── docker/                # Local multi-node/demo environment
```

The workspace is split so the accounting, storage, networking, consensus, settlement, demo, and simulation code can be tested separately.

---

## Quick start

### Run the demo
```bash
cargo run -p demo --bin demo
```

Then open: `http://127.0.0.1:8080/`

The demo shows the payment lifecycle, settlement flow, reconciliation, proof verification, and simulation controls.

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

These are the main engineering rules behind the project:

* **Exact money:** Amounts use integer base units and an explicit scale. Arithmetic checks for overflow and invalid operations.
* **Repeatable execution:** Ordered data structures and one writer make it possible to replay the same history into the same state.
* **Durable acknowledgement:** In synchronous mode, the WAL does not acknowledge a write until it reaches the filesystem sync boundary.
* **Bounded work:** The ingress queue rejects new work when full instead of buffering without a limit.
* **No unsafe code:** The workspace rejects unsafe Rust code.
* **Automated checks:** CI runs formatting, clippy, tests, benchmarks, documentation generation, the Rust 1.80 compatibility check, and dependency license/source checks.

---

## Trade-offs

Every design choice has a cost. The main trade-offs are:

1. **One writer:** Makes ordering easier to reason about, but all writes pass through that writer.
2. **File-backed WAL:** Gives clear recovery behavior, but synchronous filesystem commits take real time.
3. **OpenRaft replication:** Helps the system survive node failures, but nodes must communicate and reach agreement.
4. **MMR state:** Makes appends and proofs efficient, but storing the full tree uses more memory as the number of transfers grows.
5. **Off-chain accounting with on-chain commitments:** Keeps accounting practical while making selected settlement data auditable, but the blockchain commitment does not independently recalculate every debit and credit.

---

## Further reading

The repository keeps the deeper design reasoning separate from the README:

* [Core ledger decision](docs/decisions/core-ledger.md)
* [WAL persistence decision](docs/decisions/wal-persistence.md)
* [Distributed consensus decision](docs/decisions/distributed-consensus-raft.md)
* [MMR and Solana settlement decision](docs/decisions/merkle-mountain-range-solana-settlement.md)
* [Benchmarks](docs/BENCHMARKS.md)
* [Failure modes (FMEA)](docs/failure-modes.md)
* [Operator runbook](docs/OPERATOR-RUNBOOK.md)

---

## Status

TrustLedger is an engineering project focused on financial correctness, durability, distributed-state behaviour, cryptographic commitments, and failure testing.

It is intentionally designed so that the important properties can be inspected in code, tested, benchmarked, and reproduced.