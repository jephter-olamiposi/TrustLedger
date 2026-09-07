# Benchmarks

Measured on this development machine (Apple silicon, `profile.release` with
`lto="thin"`, `codegen-units=1`, `overflow-checks=true`). Benchmarks are
one-shot runs of the zero-dependency harness at
`crates/ledger-core/benches/throughput.rs`; single-run variance is
load-dependent. Repeat several times and report the median when enforcing a
regression gate.

Last measured run:

| Workload | Throughput | ns/op |
| --- | --- | --- |
| `create_transfer` (immediate) | ~4.9–5.3M transfers/sec | ~188–205 |
| two-phase `create_pending` + `post_pending` round-trip | ~3.9M pairs/sec | ~257 |
| `apply_batch` (256 transfers/batch) | ~0.8M transfers/sec | ~1225 |
| `Ledger::replay` (journal events) | ~5.6M events/sec | ~177 |

## Reading these numbers

- **`create_transfer` and two-phase round-trip** exceed the >1M transfers/sec
  claim in ADR-0001, confirming the `BTreeMap` determinism decision (ADR-0002)
  costs nothing measurable on the hot path.
- **`create_transfer` sits ~5–10% slower than the pre-encapsulation number**:
  the mutation boundary re-validates deserialized transfers (fresh ID, distinct
  accounts, non-zero amount, hold link) as the price of ADR-0005's
  constructor-only guarantee. Single-run variance spans the table range.
- **`apply_batch` is ~6x slower per transfer than single `create_transfer`**
  because the current implementation snapshots the whole ledger to roll back
  atomically (`self.clone()` per batch). The rollback cost scales with ledger
  size, not batch size. Before `ingest` micro-batching (Phase 3) leans on
  `apply_batch`, replace the clone with an undo log on the batch boundary
  (see ADR-0002's trade-off note).
- **`replay` at ~5.6M events/sec** validates journal-as-truth economics: a
  1M-employee ledger replays in under a second, so state rebuild on restart is
  cheap.

## Regression protocol

`scripts/check` runs `cargo bench --workspace`. A proper regression gate
(e.g., CI failing when median throughput drops >10% below a committed
baseline) is tracked in Phase 3 of `plans/trustledger/01_PROJECT_PLAN.md`.