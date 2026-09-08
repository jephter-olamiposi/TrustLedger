# WAL Runbook

Operational reference for `crates/wal`, the durable record stream behind the
ledger. The wal is part of the **money path**: it carries every committed
journal batch, so treat diagnostics here as incident response, not routine
debugging.

## How the wal understands its own file

A wal file is a sequence of framed records: 18-byte header (`magic "LETW"`,
`version`, `seq`, `payload_len`) + opaque payload + CRC32C trailer. Recovery
scans from offset 0 and re-derives the committed prefix from those frames; the
length fields are the only navigation, with a checksum bounding every record.

The three machine states an operator will meet on restart:

1. **Clean tail** — `Recovery.truncated_bytes == 0`. Every frame verified.
   Normal shutdown path: `Wal::open` reports nothing truncated.
2. **Torn tail** — `Recovery.truncated_bytes > 0`. A crash interrupted an
   append; the wal truncated the craft to the last verified record. **This is
   expected and safe**: acknowledged records were fsynced to disk before the
   caller heard ack, so a crash can only truncate the *unacknowledged* tail.
3. **`SeqMismatch` / `ChangedWhileOpen`** — the file describes a sequence the
   recovery scan cannot reconcile: a gap, a rewritten prefix, or a second
   writer. The wal refuses to guess (see ADR-0007).

## Failure modes

| Symptom | Meaning | Risk |
| --- | --- | --- |
| `WalError::Io` during `open`/`append` | disk errors, ENOSPC, permissions | **Acked money may not be durable** if writes stop landing |
| `Recovery.truncated_bytes > 0` | torn tail after crash | none — verified prefix is intact |
| `WalError::SeqMismatch { offset, actual, expected }` | foreign writer / rewritten file / corrupted length field | the acked history cannot be extended safely |
| `WalError::ChangedWhileOpen` | file mutated under an open handle | same, surfaced at read time |
| snapshot missing/stale/corrupt | crash mid-snapshot-write, bit rot | none — full wal replay rebuilds state |
| append paging stalls (no errors) | fsync-per-append backpressure | planned: ingest will amortize fsyncs (Phase 3) |

## Incident responses

### The journal does not open after a crash

1. Preserve the artifact: copy the wal file (and any snapshot) to a quarantine
   directory *before* touching the live node. Do not let recovery rewrite the
   original until a copy is taken.
2. Read the error. Are `SeqMismatch` variants involved? Do **not** truncate a
   divergent prefix; that can destroy acked history. Restore from the last
   snapshot + wal backup pair and verify:
   - `last_seq` in the snapshot ≤ the wal's verified prefix, and
   - the snapshot's CRC32C verifies.
3. Rebuild state by replaying: snapshot records up to `last_seq`, wal records
   after it (the wal exposes `read_records`).
4. Confirm the rebuilt ledger passes `ledger.verify_invariants()` with a non-
   zero expected journal, then resume writes.

### A checksum is reported

Checksum failures are a *soft* recovery event — they end the committed prefix
and the tail is truncated. If you see them while the node is **running** (not
on restart), the disk returned changed data under an open handle; stop the node
and investigate storage health before continuing.

### Suspected lost acknowledgment

Run the byte-boundary crash sweep from `crates/ledger-core/tests/wal_e2e.rs`
against a reproduction: truncate a copy of the wal at arbitrary offsets and
verify `recovered == acked_prefix` for every offset. The DoD guarantees this
invariant; the test is the proof, so a real violation points to the truncation
logic, not to the acknowledgement contract.

## Why not truncate the tail in `SeqMismatch`?

A verified-but-diverged prefix is history a previous run acknowledged. If a
foreign writer rewrote bytes, the frame boundaries are unknowable; truncating
at the divergence point could delete acked records. Failing open (surfacing
`SeqMismatch`) forces an operator to choose the restore source consciously.

## Monitoring

- Alert on any `WalError` on the commit path (left to observability wiring,
  Phase 3).
- Track `Recovery.truncated_bytes` on restart: sustained tornadoes indicate a
  crash loop over the same batch.
- Track fsync-per-append latency and batch throughput in
  `crates/wal/benches/persistence.rs`; the durability ceiling is the device
  sync rate (~4.5 MB/s for 16 KiB batches on this machine).

## Related

- [ADR-0007](adr/ADR-0007-wal-persistence.md) — framing, durability, recovery.
- `crates/wal/tests/crash.rs`, `crates/wal/tests/properties.rs`,
  `crates/ledger-core/tests/wal_e2e.rs` — the invariance proofs.
- `docs/BENCHMARKS.md` — durability and recovery numbers.