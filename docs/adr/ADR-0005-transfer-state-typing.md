# ADR-0005: Keep Runtime Transfer-State Checks (Defer Typestate Seeds)

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

`create_transfer` and `create_pending` accept a `Transfer` and reject a
wrong `state` at runtime (`InvalidTransferState`). A stricter design would
make the invalid call unrepresentable: separate seed types
(`PostedTransfer`, `PendingTransfer`) consumed by the matching method.

The roadmap (§5.5) asks that "invalid states be unrepresentable in safe Rust,"
which the runtime check does not strictly satisfy.

## Decision

Defer the typestate refactor to a later phase, and document the current
runtime enforcement as a deliberate, tested guard. Rationale:

- The journal is the source of truth; `Transfer.state` is the *recorded* state,
  not a hypothetical one. Typestate seeds would duplicate the journal's
  authority at the API boundary.
- The pending-lifecycle transitions (`Pending` -> `Posted` / `Voided`) are
  already unrepresentable through the API: `post_pending` and `void_pending`
  do not accept a `Transfer`, they operate on IDs and fix the outcome state.
- `Transfer` fields are private and construction goes only through validated
  constructors; the lifecycle move itself is a single crate-internal
  `transition_to` that rejects every illegal transition, so the state machine
  lives with the type.
- A `Transfer` can still cross the public API via deserialization, so
  `create_transfer`/`create_pending` re-validate identity (fresh ID,
  distinct accounts, non-zero amount) and eagerly check a posted transfer's
  hold link (`PendingLinkMissing` / `InvalidPendingLink`) at the mutation
  boundary. Invalid states are therefore rejected the moment they are applied,
  not later at `verify_invariants`.

## Consequences

### Benefits
- Keeps the hot-path API minimal until downstream phases constrain it.
- Constructor-only creation plus boundary re-validation makes the invalid-state
  surface unrepresentable through supported usage and fail-loud at the
  boundary otherwise.

### Costs & Trade-Offs
- `create_transfer(Pending)` compiles but fails at runtime, and a deserialized
  transfer is re-validated on every apply (a few comparisons per call,
  measured noise). If `ingest` later shows a real classification bug rate,
  revisit with seed types under an ADR.

## Alternatives Considered
- **Typestate now:** rejected as premature API surface for an authority the
  journal already holds.
- **Panic on invalid state:** rejected — all errors are typed in this codebase
  (ADR-0001 decision 5).

## References
- `create_transfer` / `create_pending` in `crates/ledger-core/src/ledger.rs`;
  `LedgerError::InvalidTransferState`;
  tests in `crates/ledger-core/tests/unit.rs`.