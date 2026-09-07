# ADR-0003: Journal Event Versioning Before the WAL

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

The durable, append-only sequence of `LedgerEvent` is the source of truth.
It is already serialized with `serde` (bincode-style) and, in Phase 2, will be
the record schema of the write-ahead log. The event payloads have already
changed shape once: the `TransferPendingPosted` event dropped a redundant
`void_residual` field. That silent format change is harmless pre-1.0 but would
be unrecoverable corruption once a WAL exists that can no longer be replayed.

Replay is now also strict: `Ledger::replay` rejects a journal whose recorded
void amount contradicts the state it derives, and rejects out-of-order
timestamps.

## Decision

- Treat every event payload as immutable once first released.
- Tag the journal container with an explicit `schema_version` field before
  Phase 2 begins. Version 0 is the current payload set; a new version is
  required for any field addition, removal, or semantic change, and decoding
  must reject unknown versions.
- No field may be repurposed or reinterpreted; adding a field requires a new
  version with a migration path.

## Consequences

### Benefits
- The WAL can be built against a stable byte format (Phase 2 DoD: replay
  produces byte-for-byte identical state).
- Corrupted or wrong-version journals fail loudly instead of replaying to a
  wrong-but-plausible state.

### Costs & Trade-Offs
- Version tagging is a small, one-time encoding cost on the not-yet-written
  serialization layer (owned by the future `wal` crate, not `ledger-core`).

## Alternatives Considered

- **Leave payloads unversioned:** acceptable only until Phase 2; every future
  change becomes a silent format migration.
- **Self-describing (semver'd) events:** overkill; a journal is one schema,
  not a mixed stream.

## References
- ADR-0001 decision 4 (journal-as-truth); Phase 2 WAL DoD in
  `plans/trustledger/01_PROJECT_PLAN.md`.
- `Ledger::replay` fail-loud behavior in `crates/ledger-core/src/ledger.rs`.