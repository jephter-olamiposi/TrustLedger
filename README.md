# TrustLedger

A financial ledger in Rust with double entry accounting, durable storage, replicated state, and cryptographic settlement with Solana.

TrustLedger is a financial system built around a deterministic ledger core. The ledger handles accounts, balances, pending transfers, captures, voids, journal events, and accounting invariants. Around that core is the infrastructure to make the state durable, controlled under load, replicated across nodes, reconciled across different views of a payment, and verifiable after settlement.

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

Pending money is tracked separately from posted money. A pending transfer can be captured fully or partially, or voided. Once it reaches a terminal state it cannot be changed again. The ledger checks these transitions as part of the state machine rather than leaving them to the application layer.

## The ledger core

`ledger-core` owns the accounting state. Accounts have explicit types: asset, liability, equity, revenue, expense. Amounts are integer base units with an explicit decimal scale — no floating point arithmetic for money. Balances keep posted and pending amounts separate so an authorization hold does not look like settled money. Transfers have explicit states and legal transitions. The ledger keeps a journal of account and transfer events; that journal is the history from which state can be rebuilt.

The important invariant is that the state can be checked independently of the code path that produced it. The ledger verifies balance conservation, debit and credit totals, pending balances, transfer references, and other state relationships. When a mutation is applied, invariant checks can catch a problem close to the mutation instead of waiting for a later operation to expose it.

## Batch processing

Ledger operations can be prepared as a batch before they become permanent. The ledger keeps the previous values of affected state while a batch is being prepared. If an operation fails, those values are restored — a failed batch does not leave half of its changes behind, and it avoids cloning the entire ledger for rollback behaviour. Only after a batch has been successfully prepared are its journal events ready to be persisted.

## Durable storage

The `wal` crate provides the durable write ahead log used by the ingestion path. The WAL stores framed records with sequence information, payload length, and CRC32C verification.

During recovery, each frame is checked. If a crash leaves an incomplete or invalid record at the end of the file, recovery keeps the verified prefix and removes the damaged tail. The same event application path is used when rebuilding the ledger, so recovery does not require a separate interpretation of the journal.

The WAL supports both individual writes and grouped writes. For grouped writes, several prepared events can be written together and the filesystem synchronised once for the group — making the durability boundary explicit while avoiding a filesystem sync for every operation. Recovery is streamed instead of loading the complete WAL into memory.

## Ingestion

The ingestion layer is deliberately bounded. Requests enter through a Tokio channel with fixed capacity. When the queue is full, new work is rejected instead of allowing memory to grow without limit.

The ingestion engine collects requests into short batches (512 operations or 2 ms). A batch is prepared against the ledger, converted into journal events, written to the WAL, and then committed to the in memory state. The order matters: the ledger state is not treated as durable until the corresponding journal data has crossed the configured durability boundary. The ingestion layer also exposes gRPC endpoints and records rejected work, queue behaviour, and processing metrics.

## Replicated state

TrustLedger contains a three node OpenRaft implementation. The replicated path separates agreement from the accounting rules — Raft determines the committed order of changes and the state machine applies those committed entries to the ledger core. The cluster supports leader discovery, proposals, node isolation, partitions, recovery after healing, and node shutdown in the test environment.

The current Raft log store is an in memory `MemLogStore`, separate from the file backed WAL used by the ingestion engine. That distinction is intentional: the project has a durable local ingestion path and a separate replicated state path rather than pretending the two persistence mechanisms are the same thing. Snapshots contain the ledger scale and journal information needed to rebuild state.

## Settlement

The settlement layer turns a group of settled transfers into a cryptographic commitment using a Merkle Mountain Range. Each transfer becomes a leaf hash; the MMR maintains its peaks as new transfers are added and folds those peaks into a single root. The hashing scheme separates leaves, internal nodes, and peak folding with different domain prefixes, making the different hashing operations unambiguous. The result is an incremental structure that produces an inclusion proof for an individual transfer without requiring the complete batch to be shared with the verifier.

## Solana settlement

The `solana-settle` crate contains the Solana settlement program and supporting client and verification code. The program stores settlement state in a PDA containing authority, epoch, batch sequence, current root, previous root, transfer count, cumulative settled amount, chain tip, settlement timestamp, and bump. Each new settlement advances the batch sequence and carries forward the previous root so the sequence of commitments remains linked. The program also exposes inclusion verification — a proof can be checked against the root stored in the settlement account.

The project produces a portable transfer receipt containing the transfer information, leaf hash, settlement information, committed root, and inclusion proof. The standalone verifier can validate that receipt without running the ledger:

```bash
cargo run -p solana-settle --bin verifier -- \
  --receipt path/to/receipt.json
```

The important boundary: the cryptographic proof proves inclusion in the committed root. It does not independently prove the original off chain data was truthful — the commitment proves the transfer belongs to the dataset that was committed.

The demo currently exercises the Solana program locally through Solana account structures. It is not pretending to be a live Solana production deployment.

## Payment and reconciliation

The demo application puts the ledger behind a payment flow. Payments have their own lifecycle and keep references to the ledger transfers associated with them, supporting authorization, capture, void, and refund behaviour. A payment can be tracked independently from the underlying accounting events while maintaining the relationship between the two. The project includes a mock ACH rail modelling traditional banking settlement behaviour (batch netting, routing information, settlement delays) alongside the Solana rail — both operate against the same settlement abstraction, keeping the ledger independent from a particular settlement network.

Payment events are verified using HMAC SHA 256 with constant time comparison, checked for freshness, and deduplicated by event ID. Payment mutations use idempotency keys so retries do not create duplicate financial state — failed processing releases the key so the operation can be retried.

Reconciliation compares three independent views: application captured amounts, ledger liability accounts, and on-chain settled amounts from the MMR root. When values disagree, the system creates a reconciliation incident with full context rather than silently accepting the difference. Disagreement is treated as state that needs investigation, not something that can always be automatically repaired.

## Deterministic simulation

TrustLedger includes a deterministic simulation environment for distributed failure testing. The simulator controls virtual time and seeded randomness, injecting packet delay, loss, duplication, network partitions, node crashes, simulated storage failures, recovery, and catch up. The cluster model exercises proposals, acknowledgements, commits, leader changes, and recovery. The oracle checks financial conservation and replicated history while failures are happening.

Reproducibility is the useful property: a failure is associated with a seed, so the same seed can run the same sequence again.

```bash
cargo run -p simulator -- \
  --seed 42 \
  --scenario all \
  --steps 200
```

The simulator also supports running multiple seeds as a fuzzing campaign, exercising combinations of failures difficult to reproduce with normal integration tests.

## Testing

Testing is spread across individual crates and the complete application. Ledger tests cover accounting rules, state transitions, rollback behaviour, and invariants. Ingestion tests cover backpressure and gRPC behaviour. WAL tests cover persistence and recovery. Raft tests exercise cluster behaviour and failure conditions. Settlement tests cover commitments, proofs, Solana program instructions, and receipt verification. The demo contains end to end tests covering the payment flow. The simulator provides another layer where the system is exercised under controlled failures. Different layers can be tested independently and then exercised together.

## Observability

The project exposes Prometheus compatible metrics for transfer processing and rejection, queue behaviour, settlement and reconciliation activity — without making the accounting core depend on the monitoring implementation.

## Benchmarks

The repository contains benchmark suites for the accounting core, WAL, ingestion, MMR operations, proof generation, and proof verification. Current recorded results include roughly 4.4 million immediate transfers per second in the ledger benchmark and roughly 4.9 million journal events per second during replay. The suite also records WAL throughput, recovery throughput, MMR append cost, root generation, proof generation, and proof verification. These are measurements from the repository benchmark environment — not production capacity claims. Full methodology and recorded results are in `docs/BENCHMARKS.md`.

## Failure results

The deterministic simulation suite covers network partitions, leader crashes, torn write recovery, packet loss, duplicate packets, and longer chaos runs. A 100 seed fuzz run recorded no invariant violations. The important part is not the number itself — it is that the failure schedule is reproducible and the financial invariants are checked while failures are being injected.

## Project structure

```text
TrustLedger
|
+-- crates
|   +-- ledger-core    Double entry accounting and state machine
|   +-- wal            Durable write ahead log and recovery
|   +-- ingest         Bounded ingestion and micro batching
|   +-- raft           OpenRaft replicated state machine
|   +-- merkle         Merkle Mountain Range and proofs
|   +-- solana-settle  Solana settlement program and verifier
|   +-- observability  Runtime metrics
|
+-- apps
|   +-- demo           Payment flow, API, dashboard, settlement,
|                      reconciliation, webhooks, end to end tests
|
+-- simulator          Deterministic distributed failure simulation
|
+-- docs               Architecture decisions, benchmarks,
                       failure analysis, operator documentation
```

## Running it

```bash
# Demo (payment flow, settlement, reconciliation, dashboard)
cargo run -p demo --bin demo
# → http://127.0.0.1:8080/

# Full test suite
cargo test --workspace

# Lint/format
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings

# Deterministic simulation
cargo run -p simulator -- --scenario all --seed 42 --steps 200

# Verify settlement receipt
cargo run -p solana-settle --bin verifier -- --receipt receipt.json
```

## Design notes

The deeper design decisions live in the repository documentation:

- Core ledger: accounting model, invariants, batch semantics
- WAL persistence: frame format, recovery, group commit
- Consensus: Raft config, snapshots, membership changes
- Settlement: MMR construction, PDA schema, verification
- Simulation: virtual time, fault model, oracle design
- Benchmarks: methodology, hardware, full results
- Failure modes: FMEA, detection, mitigation, runbook refs
- Operator runbook: deploy, monitor, rotate keys, recover

These documents are part of the project because the code is easier to understand when the reasons behind the boundaries are visible. 