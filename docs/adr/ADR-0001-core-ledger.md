# ADR-0001: Core Ledger Architecture and Financial Invariants

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

Financial accounting systems require absolute mathematical correctness, determinism, and zero data loss. Traditional relational database approaches (e.g., executing `UPDATE accounts SET balance = balance + x WHERE id = ?`) suffer from:
1. **The Hot Account Problem:** Heavy lock contention on platform fee or merchant settlement accounts serializes writes and causes connection pool exhaustion.
2. **Floating-Point Non-Determinism:** Floating-point numbers (`f32`/`f64`) introduce rounding ambiguity and divergence during state replay across different architectures.
3. **Implicit Invariants:** Failure to enforce double-entry rules ($\sum \text{Debits} == \sum \text{Credits}$) at the type and domain layer allows value creation or leakage bugs.

## Decision

We establish `crates/ledger-core` as an in-memory, deterministic, double-entry financial core with the following architectural rules:

1. **Two Entities Only (Double-Entry Schema):** The core only models `Account` and `Transfer`. Balances are materialized views over an append-only sequence of immutable transfers.
2. **Strict Fixed-Point Integer Arithmetic:** Floats are prohibited. All monetary quantities use `Amount(u128)` paired with an explicit `Scale(u8)` (e.g., scale 2 for USD, scale 6 for USDC). Arithmetic operations are checked and return typed domain errors (`LedgerError::ArithmeticOverflow`) on overflow.
3. **Two-Phase Transfer Lifecycle:** The core natively models two-phase transfers (`Pending` $\to$ `Posted` / `Voided`), reserving funds upon authorization and releasing or settling upon capture/void. This prevents double-spending and enables real-world card and settlement workflows.
4. **Single-Writer Deterministic State Machine:** Mutations are processed sequentially by a single execution core without thread locks. Given an identical log of transfers, replay yields identical state.
5. **Zero `unwrap()` / `panic!`:** All errors are represented as strongly typed variants using `thiserror`.

## Consequences

### Benefits
- **Zero Lock Contention:** Eliminates mutex and row-lock overhead, allowing throughput exceeding 1,000,000 transfers/sec in memory.
- **Provable Correctness:** Balances cannot leak value. Every debit has an equal credit.
- **Deterministic Replay:** Replaying transfers from a write-ahead log will always converge to the exact same account balances.

### Costs & Trade-Offs
- State modifications must be funneled through a single-writer event loop (managed in later phases via micro-batching and Raft consensus).
- All amounts must be normalized or validated against account scale factors.

## Alternatives Considered

- **Relational SQL Database (Postgres with ACID transactions):** Rejected due to hot account row-lock serialization, MVCC tuple bloat, and connection pool starvation under high concurrency.
- **Floating-Point Representation (`f64`):** Rejected due to IEEE 754 rounding errors, precision loss, and non-deterministic serialization.
- **One-Phase Immediate Transfers Only:** Rejected because it cannot accurately model real-world card authorisations, pending holds, or payout lifecycles.

## Amendments

These ADRs refine the decisions above without changing the architecture:

- **ADR-0002** — account/transfer indexes are `BTreeMap` for deterministic iteration and replay.
- **ADR-0003** — journal payloads become versioned before the WAL phase; replay is fail-loud on corrupted or out-of-order journals.
- **ADR-0004** — availability reads return zero when overdrawn (view, not guard); `close_account` owns the account lifecycle with a journal event.
- **ADR-0005** — transfer-state gates stay runtime-checked and typed; typestate seeds deferred.
- **ADR-0006** — state-transition ordering is compute-then-write (state flip last); event timestamps are monotonic under `LedgerError::TimestampBehindPrior`.

Noted in passing: `Transfer` no longer carries a per-transfer `Scale`; the
ledger-wide scale (decision 2) is the single scale authority.
