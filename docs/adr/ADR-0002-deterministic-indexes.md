# ADR-0002: Deterministic Account and Transfer Index (BTreeMap)

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

`Ledger` holds accounts and transfers as in-memory indexes over an
append-only journal. The journal is the truth and the indexes are a derived,
disposable cache. That design only holds if the indexes are deterministic:
given the same event log, two processes must reconstruct byte-identical state
or the ledger, its verification, and its replay-check cannot agree.

Two standard hash maps were candidates: `HashMap` (default, u128-keyed) and
`BTreeMap`. `HashMap` provides O(1) expected access but its iteration order is
nondeterministic across runs, HashBrown seeds, and rebuilds of the map.

## Decision

Store both `accounts` and `transfers` in `BTreeMap` keyed by the newtype
(`AccountId`, `TransferId`), whose `Ord` derives from their `u128` payload.

Iteration is then key-ordered and identical across processes and runs,
guaranteeing deterministic replay, deterministic `verify_invariants`, and
stable serialization of the balance view.

## Consequences

### Benefits
- Deterministic iteration everywhere the indexes are walked
  (`verify_invariants`, `verify_structure`, replay convergence).
- `Ledger: PartialEq/Eq` and `Clone` are stable across machines, so the
  replay-`assert_eq!` check is a genuine convergence proof.
- BTreeMap requires no hasher and no hasher initialization cost on seed.

### Costs & Trade-Offs
- Access degrades from O(1) to O(log n), and inserts/allocs have higher
  constant factors than `HashMap`. Measured on the development machine
  (`profile.release`), immediate `create_transfer` still exceeds 5M
  transfers/sec, so the ordering guarantee costs no observable throughput at
  the >1M target. If long-term profiling shows the map dominates, shard by a
  fixed number of ordered segments rather than abandoning determinism.

## Alternatives Considered
- **`HashMap` with a fixed seed:** still has arbitrary-but-consistent ordering
  that is not a total order over keys; iteration must sort for determinism,
  adding the very cost BTreeMap pays amortized.
- **Dense Vec with free-list:** deterministic, but re-indexing a journal into
  a Vec requires compaction and loses O(log n) point lookups by ID that the
  two-phase lifecycle depends on.

## References
- ADR-0001, decisions 3 (two-phase lifecycle) and 4 (deterministic replay).
- Bench script `scripts/check`, workload `benches/throughput.rs`.