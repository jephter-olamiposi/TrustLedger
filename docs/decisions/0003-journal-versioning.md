# Decision 0003: Journal Event Versioning Before the WAL

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

The durable, append-only sequence of `LedgerEvent` is the source of truth.
Event batches are serialized with postcard and stored as opaque WAL payloads.
The event payloads have already changed shape once: the
`TransferPendingPosted` event dropped a redundant `void_residual` field. That
silent format change is harmless pre-1.0 but would be unrecoverable corruption
once a WAL exists that can no longer be replayed.

Replay is now also strict: `Ledger::replay` rejects a journal whose recorded
void amount contradicts the state it derives, and rejects out-of-order
timestamps.

## Decision

- Treat every event payload as immutable once first released.
- The WAL frame carries an explicit schema version byte. The current frame
  version is `1`; a new version is required for any field addition, removal, or
  semantic change, and decoding must reject unknown versions.
- No field may be repurposed or reinterpreted; adding a field requires a new
  version with a migration path.

## Consequences

### Benefits
- The WAL uses a stable, versioned byte format and replay rejects malformed
  batches instead of silently accepting a shorter history.
- Corrupted or wrong-version journals fail loudly instead of replaying to a
  wrong-but-plausible state.

### Costs & Trade-Offs
- Version tagging is a small, one-time encoding cost on the not-yet-written
  serialization layer (owned by the future `wal` crate, not `ledger-core`).

## Alternatives Considered

- **Leave payloads unversioned:** rejected because every future change would
  become a silent format migration.
- **Self-describing (semver'd) events:** overkill; a journal is one schema,
  not a mixed stream.

## References
- Decision 0001 decision 4 (journal-as-truth); the WAL framing and codec in
  `crates/wal/src/` and `crates/ledger-core/src/codec.rs`.
- `Ledger::replay` fail-loud behavior in `crates/ledger-core/src/ledger.rs`.