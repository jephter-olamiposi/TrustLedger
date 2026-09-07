# ADR-0006: Transactional State-Transition Ordering and Oversight Timestamps

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

Two invariants protect the in-memory state machine against the class of bug
where state and economics diverge silently:

1. In `post_pending` and `void_pending` the pending transfer's `state` was
   flipped to `Posted` / `Voided` before the four account-balance mutations
   ran, and each account mutation was reached via a `?`. Any `Err` past the
   midpoint could return with the transfer already advanced while balances
   were untouched (or half-applied). The mutations are provably infallible
   under current invariants, but the ordering made partial-application
   structurally possible.
2. Event `timestamp` fields are caller-supplied with no ordering rule. A
   settlement ledger whose audit trail accepts out-of-order or reused
   timestamps cannot be trusted chronologically.

## Decision

- Every transition now runs a two-phase commit: **compute** all balance deltas
  (every fallible operation, every `?`) in a read-only pass, then **write** the
  deltas — pure assignments — and finally flip the transfer `state`. The state
  flag is the last effect, so it can never describe effects that did not land.
- `Ledger` tracks `last_timestamp`, the maximum event timestamp seen, and
  rejects any operation whose timestamp is behind that watermark with
  `LedgerError::TimestampBehindPrior`. Ties are allowed (wall-clock-granularity
  events are legitimate). The watermark is a *derived* field: it is advanced
  only by the same path that appends a journal event, so replay reconstructs
  it exactly.

## Consequences

### Benefits
- No code path exists (reachable or not) in which a pending transfer is
  simultaneously half-settled; the write phase has no fallible operations.
- A journal with out-of-order timestamps fails loud at replay instead of
  producing a plausible-but-wrong history.

### Costs & Trade-Offs
- The compute-then-write pattern performs two index probes per account
  instead of one. Measured in `benches/throughput.rs`, two-phase round-trips
  remain above 3.8M/sec, so the ordering guarantee costs nothing measurable.
- Timestamp monotonicity assumes a sane clock; clients that advance their own
  timestamps must order them.

## Alternatives Considered
- **Hold `&mut` to both accounts simultaneously (`get_many_mut`):** requires
  MSRV > 1.80; the compute-then-write pattern keeps MSRV and clarity.
- **Assign timestamps inside the ledger:** rejected; wall-clock provenance
  belongs to the caller, ordering integrity belongs to the ledger.

## References
- `post_pending`, `void_pending`, `check_timestamp`, `advance_timestamp`,
  `replay` in `crates/ledger-core/src/ledger.rs`;
  `LedgerError::TimestampBehindPrior`;
  tests in `crates/ledger-core/tests/unit.rs`.