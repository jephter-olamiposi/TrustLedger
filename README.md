# TrustLedger

A financial ledger in Rust with double entry accounting, durable storage, replicated state, and cryptographic settlement with Solana.

TrustLedger is a financial system built around a deterministic ledger core.

The ledger handles accounts, balances, pending transfers, captures, voids, journal events, and accounting invariants. Around that core is the infrastructure needed to make the state durable, controlled under load, replicated across nodes, reconciled across different views of a payment, and verifiable after settlement.

The project includes a durable write ahead log, bounded asynchronous ingestion, micro batching, OpenRaft replication, Merkle Mountain Range settlement commitments, a Solana settlement program, payment webhooks, idempotency handling, reconciliation, observability, and deterministic failure simulation.

The main idea is to keep the accounting state small and deterministic while making everything around it explicit.

## The system

A transfer can exist as a pending hold before it becomes a posted transfer.

```text
Pending
   |
   +------> Voided
   |
   v
Posted
   |
   v
Settled
   |
   v
MMR root
   |
   v
Solana commitment
```

Pending money is tracked separately from posted money.

A pending transfer can be captured fully or partially, or it can be voided. Once it reaches a terminal state it cannot be changed again.

The ledger checks these transitions as part of the state machine rather than leaving them to the application layer.

## The ledger core

`ledger-core` owns the accounting state.

Accounts have explicit types including asset, liability, equity, revenue, and expense.

Amounts are represented as integer base units with an explicit decimal scale. The ledger does not use floating point arithmetic for money.

Balances keep posted and pending amounts separate so that an authorization hold does not look like settled money.

Transfers have explicit states and legal transitions.

The ledger also keeps a journal of account and transfer events. That journal is the history from which state can be rebuilt.

The important invariant is that the state can be checked independently of the code path that produced it.

The ledger verifies balance conservation, debit and credit totals, pending balances, transfer references, and other state relationships.

When a mutation is applied, invariant checks can catch a problem close to the mutation instead of waiting until some later operation exposes it.

## Batch processing

Ledger operations can be prepared as a batch before they become permanent.

The ledger keeps the previous values of the affected state while a batch is being prepared. If an operation fails, those values are restored.

That means a failed batch does not leave half of its changes behind.

It also avoids cloning the entire ledger just to get rollback behaviour.

Only after a batch has been successfully prepared are its journal events ready to be persisted.

## Durable storage

The `wal` crate provides the durable write ahead log used by the ingestion path.

The WAL stores framed records with sequence information, payload length, and CRC32C verification.

On recovery the log is scanned and each frame is checked.

If a crash leaves an incomplete or invalid record at the end of the file, recovery keeps the verified prefix and removes the damaged tail.

The same event application path is used when rebuilding the ledger, so recovery does not require a separate interpretation of the journal.

The WAL supports both individual writes and grouped writes.

For grouped writes, several prepared events can be written together and the filesystem can be synchronised once for the group.

That makes the durability boundary explicit while avoiding a filesystem sync for every individual operation.

Recovery is streamed instead of loading the complete WAL into memory.

## Ingestion

The ingestion layer is deliberately bounded.

Requests enter through a Tokio channel with a fixed capacity.

When the queue is full, new work is rejected instead of allowing memory usage to grow without a limit.

The ingestion engine collects requests into short batches.

The current limits are 512 operations or 2 milliseconds.

A batch is prepared against the ledger, converted into journal events, written to the WAL, and then committed to the in memory state.

The order matters.

The ledger state is not treated as durable until the corresponding journal data has crossed the configured durability boundary.

The ingestion layer also exposes gRPC endpoints and records rejected work, queue behaviour, and processing metrics.

## Replicated state

TrustLedger also contains a three node OpenRaft implementation.

The replicated path separates agreement from the accounting rules.

Raft determines the committed order of changes and the state machine applies those committed entries to the ledger core.

The cluster supports leader discovery, proposals, node isolation, partitions, recovery after healing, and node shutdown in the test environment.

The current Raft log store is an in memory `MemLogStore`.

It is separate from the file backed WAL used by the ingestion engine.

That distinction is intentional. The project has a durable local ingestion path and a separate replicated state path rather than pretending that the two persistence mechanisms are the same thing.

Snapshots contain the ledger scale and journal information needed to rebuild state.

## Settlement

The settlement layer turns a group of settled transfers into a cryptographic commitment.

TrustLedger uses a Merkle Mountain Range.

Each transfer becomes a leaf hash.

The MMR maintains its peaks as new transfers are added and folds those peaks into a single root.

```text
transfer
   |
   v
leaf hash
   |
   v
MMR
   |
   v
settlement root
```

The hashing scheme separates leaves, internal nodes, and peak folding with different domain prefixes.

That makes the different hashing operations unambiguous.

The result is an incremental structure that can produce an inclusion proof for an individual transfer without requiring the complete batch to be shared with the verifier.

## Solana settlement

The `solana-settle` crate contains the Solana settlement program and the supporting client and verification code.

The program stores the settlement state in a PDA.

The state contains the authority, epoch, batch sequence, current root, previous root, transfer count, cumulative settled amount, chain tip, settlement timestamp, and bump.

Each new settlement advances the batch sequence.

After the first settlement, the previous root is carried forward so the sequence of commitments remains linked.

The program also exposes inclusion verification.

A proof can be checked against the root stored in the settlement account.

The project produces a portable transfer receipt containing the transfer information, leaf hash, settlement information, committed root, and inclusion proof.

The standalone verifier can validate that receipt without running the ledger.

```bash
cargo run -p solana-settle --bin verifier -- \
  --receipt path/to/receipt.json
```

The important boundary here is that the cryptographic proof proves inclusion in the committed root.

It does not independently prove that the original off chain data was truthful.

The commitment proves that the transfer belongs to the dataset that was committed.

The demo currently exercises the Solana program locally through Solana account structures. It is not pretending to be a live Solana production deployment.

## Payment flow

The demo application puts the ledger behind a payment flow.

Payments have their own lifecycle and keep references to the ledger transfers associated with them.

The application supports authorization, capture, void, and refund behaviour.

A payment can therefore be tracked independently from the underlying accounting events while still maintaining the relationship between the two.

The project also includes a mock ACH rail.

The mock rail models traditional banking settlement behaviour such as batch netting, routing information, and settlement delays.

The Solana rail and the mock ACH rail both operate against the same settlement abstraction.

This keeps the ledger independent from a particular settlement network.

## Webhooks

The payment API includes webhook verification and deduplication.

Incoming payment events are verified against their raw payload using HMAC SHA 256.

The signature comparison uses constant time verification.

Webhook timestamps are checked against a freshness window to reduce replay risk.

Processed event IDs are tracked so that duplicate webhook deliveries do not apply the same event twice.

This keeps external payment delivery separate from the accounting mutation itself.

## Idempotency

The payment API also keeps track of idempotency keys.

A repeated request with the same key does not create another financial mutation.

If processing fails, the key can be released so the operation can be retried rather than leaving a failed request permanently occupied.

This matters because payment systems routinely receive retries from clients and external systems.

The ledger should see one intended mutation rather than one mutation for every network attempt.

## Reconciliation

The application keeps separate views of payment state, ledger state, and settlement state.

Reconciliation compares those views.

For example, captured payment totals can be compared against the corresponding ledger liability and the amount included in settlement.

When the values disagree, the system creates a reconciliation incident rather than silently accepting the difference.

The reconciliation layer therefore treats disagreement as state that needs to be investigated.

It does not pretend that every discrepancy can automatically be repaired.

## Deterministic simulation

TrustLedger includes a deterministic simulation environment for distributed failure testing.

The simulator controls virtual time and seeded randomness.

It can inject packet delay, packet loss, duplicate packets, network partitions, node crashes, simulated storage failures, recovery, and catch up.

The cluster model exercises proposals, acknowledgements, commits, leader changes, and recovery.

The oracle checks financial conservation and replicated history while those failures are happening.

The useful property is reproducibility.

A failure is associated with a seed, so the same seed can be used to run the same sequence again.

```bash
cargo run -p simulator -- \
  --seed 42 \
  --scenario all \
  --steps 200
```

The simulator also supports running multiple seeds as a fuzzing campaign.

This gives the distributed system a way to exercise combinations of failures that are difficult to reproduce with normal integration tests.

## Testing

Testing is spread across the individual crates and the complete application.

The ledger tests accounting rules, state transitions, rollback behaviour, and invariants.

The ingestion tests cover backpressure and gRPC behaviour.

The WAL tests cover persistence and recovery.

The Raft tests exercise cluster behaviour and failure conditions.

The settlement tests cover commitments, proofs, Solana program instructions, and receipt verification.

The demo contains end to end tests covering the payment flow.

The simulator provides another layer where the system is exercised under controlled failures.

The result is not one large test suite around one application.

The different layers can be tested independently and then exercised together.

## Observability

The project exposes metrics for the important runtime paths.

Transfer processing and rejection are tracked.

Queue behaviour is visible.

Settlement and reconciliation activity can be observed.

The observability crate exposes Prometheus compatible metrics without making the accounting core depend on the monitoring implementation.

This keeps operational concerns outside the ledger itself.

## Benchmarks

The repository contains benchmark suites for the accounting core, WAL, ingestion, MMR operations, proof generation, and proof verification.

The current recorded results include roughly 4.4 million immediate transfers per second in the ledger benchmark and roughly 4.9 million journal events per second during replay.

The benchmark suite also records WAL throughput, recovery throughput, MMR append cost, root generation, proof generation, and proof verification.

These are measurements from the repository benchmark environment.

They are not production capacity claims.

The full methodology and recorded results are in `docs/BENCHMARKS.md`.

## Failure results

The deterministic simulation suite currently covers network partitions, leader crashes, torn write recovery, packet loss, duplicate packets, and longer chaos runs.

The recorded campaign includes a 100 seed fuzz run with no recorded invariant violations.

The important part is not the number itself.

The important part is that the failure schedule is reproducible and the financial invariants are checked while the failures are being injected.

## Project structure

```text
TrustLedger
|
+-- crates
|   |
|   +-- ledger-core
|   |   Double entry accounting and state machine
|   |
|   +-- wal
|   |   Durable write ahead log and recovery
|   |
|   +-- ingest
|   |   Bounded ingestion and micro batching
|   |
|   +-- raft
|   |   OpenRaft replicated state machine
|   |
|   +-- merkle
|   |   Merkle Mountain Range and proofs
|   |
|   +-- solana-settle
|   |   Solana settlement program and verifier
|   |
|   +-- observability
|       Runtime metrics
|
+-- apps
|   |
|   +-- demo
|       Payment flow, API, dashboard, settlement,
|       reconciliation, webhooks, and end to end tests
|
+-- simulator
|   Deterministic distributed failure simulation
|
+-- docs
    Architecture decisions, benchmarks,
    failure analysis, and operator documentation
```

## Running it

Run the demo with

```bash
cargo run -p demo --bin demo
```

Then open

```text
http://127.0.0.1:8080/
```

Run the complete test suite with

```bash
cargo test --workspace
```

Run formatting checks with

```bash
cargo fmt --check
```

Run Clippy with

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Run the simulator with

```bash
cargo run -p simulator -- \
  --seed 42 \
  --scenario all \
  --steps 200
```

The repository also contains a Docker environment for running the local multi node setup.

## Design notes

The deeper design decisions live in the repository documentation.

The core ledger decision explains the accounting model and deterministic state.

The WAL decision explains the durability boundary and recovery model.

The distributed consensus decision explains the OpenRaft integration.

The settlement decision explains the MMR design and Solana commitment model.

The reconciliation decision explains how the payment, ledger, and settlement views are kept separate.

The deterministic simulation decision explains the failure model and reproducibility approach.

These documents are part of the project because the code is easier to understand when the reasons behind the boundaries are visible.

## What this project is

TrustLedger is a working engineering project built around financial state.

It brings accounting, persistence, asynchronous ingestion, replication, settlement, reconciliation, security, observability, and failure testing into one system.

The individual pieces are useful on their own.

The interesting part is how they behave together when the system is under load, when a request is retried, when a process crashes, when a node disappears, or when two views of the same money disagree.
