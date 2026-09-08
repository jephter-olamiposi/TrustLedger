# ADR-0007: Write-Ahead Log — Framing, Durability, and Recovery

- **Status:** Accepted
- **Date:** 2026-09-08
- **Author:** Jephter Olaifa

---

## Context

The ledger is memory-first: state is an in-memory `BTreeMap` journal, and there
is no durable representation. Restarting loses every account and transfer. The
raft layer (Phase 3) and the demo surface both need a side-effect-free,
crash-safe commit path:

1. A mutation must be durable **before** it is acknowledged to the caller, so
   an acknowledged write survives process death and machine failure.
2. On restart, state must rebuild to exactly the last acknowledged write —
   no acknowledged money movement can be lost, and no partial write can be
   accepted.
3. The journal format must be stable, versioned, and self-describing enough
   that a torn tail is detectable and distinguishable from logical corruption.

ADR-0003 already fixed the ledger's *event* format (postcard schema version 1).
This ADR fixes the *transport*: the byte framing that makes an event batch
indexable, verifiable, and recoverable on disk.

## Decision

A new crate, `crates/wal`, owns the durable record stream. The ledger stays
pure; it never opens files. The write-ahead flow is:

1. **compute** — `ledger.prepare_batch(&mut self, &[Transfer])` speculatively
   applies the batch against live state (an O(batch) undo-log apply, no
   whole-ledger clone), captures the `Vec<LedgerEvent>` it produced, and rolls
   the speculation back — a prepare changes no state (ADR-0006's
   compute-write ordering, reused, not duplicated);
2. **durable** — the caller encodes the batch with `ledger_core::codec`
   (postcard, ADR-0003) and appends the opaque byte slice to the wal;
3. **commit** — `ledger.commit_events(&events)` re-validates and applies the
   same event list to memory, atomically (a failed mid-batch event rolls the
   ledger back). Nothing may mutate the ledger between prepare and commit
   (single-writer contract, enforced by the type system on the `&mut Ledger`
   side and documented on the wal crate); commit re-checks live state after
   every event, so a drifted ledger fails cleanly instead of double-applying.

### Record framing

Each record is a prefix-marker frame plus a checksum trailer, **without** an
explicit record count or length map — length recovery is a linear scan over the
length fields, so no index can drift from the bytes:

| Field | Size | Encoding |
| --- | --- | --- |
| magic | 4 | `u32` LE = `0x4C_4554_57` (`"LETW"`) |
| version | 1 | `u8` = 1 (ADR-0003's schema version, incremented with it) |
| flags | 1 | `u8` reserved, `0` (future: compression, codec id) |
| seq | 8 | `u64` LE, monotonically `+1` per record, first record = 0 |
| payload_len | 4 | `u32` LE, ≤ `DEFAULT_MAX_PAYLOAD_LEN` (256 KiB) |
| payload | `payload_len` | opaque event-batch bytes |
| crc | 4 | `u32` CRC32C over header + payload |

The header is a fixed 18 bytes; `CRC32C`'s 64-bit software fallback and
hardware instruction on Apple silicon cost ~1.5 GB/s (order of magnitude
cheaper than fsync, so checksumming is not the durability bottleneck; see
`docs/BENCHMARKS.md`).

### Durability

- `WalOptions { sync_per_append: bool }`, default `true`: `sync_data` after
  every append. An `append` acknowledges the caller only when the frame has
  reached the journal device — that syscall, not the copy to the OS page cache,
  is the durability boundary.
- Buffered `BufWriter` amortizes the write syscalls; the fsync cannot be
  elided by the runtime. `sync_per_append: false` exists for tests and
  benchmarks only and is documented as such (the ingest layer, Phase 3, may
  batch fsyncs explicitly when it owns the acknowledgment policy).
- `SetLen` and metadata were deliberately avoided on the append path (O_DIRECT
  at append time is deferrable); a `Wal::commit_len` API lets a snapshot any
  caller truncate an already-verified prefix. Recovery remains the source of
  truth for torn tails.

### Recovery

`Wal::open` streams the file from offset 0, one frame at a time (so recovery
holds at most a payload chunk in memory, never the whole log), and classifies
every frame:

- valid header + matching `seq` + matching checksum → verified record, advance
  scan;
- anything else (bad magic, impossible `payload_len`, checksum mismatch, or a
  non-incrementing `seq`) → **soft end-of-prefix**: the bytes from this offset
  to EOF are a torn or partial tail from a crash. The wal truncates to the
  verified prefix and reports `Recovery { next_seq, verified_records,
  truncated_bytes }`.
- a valid frame whose `seq` is *ahead* of the expected value (a gap within a
  well-formed prefix) → **hard error** `WalError::SeqMismatch`: a foreign
  writer or a corrupted length field; guessing the boundary would gamble money,
  so open fails.

After recovery, `append` resumes at `next_seq`. `read_records(&mut self)`
replays the committed prefix through the open handle (no path re-open) using
the same streaming scanner, and returns the file position to the end so the
next `append` stays contiguous — this lets callers rebuild state after restart.

### Codec-agnostic layering

The wal stores `&[u8]` and knows nothing about ledger events. The ledger owns
the postcard mapping (`ledger_core::codec`), so the wire format can evolve
under ADR-0003's versioning without touching the storage layer. A malformed
payload that still checksums is a *ledger* error, not a *wal* error.

### Snapshots

Restart replay is a linear scan; to avoid a forever-growing replay, a
`SnapshotFile` persists the post-version-ified journal as a single record
(`SNAP_MAGIC "LTSN"`, length-prefixed, CRC32C trailer):

- written to a temp file, `sync_data`, atomically `rename`d over the target,
  then the parent directory is fsynced — so a reader never sees a torn
  snapshot and the rename is crash-durable;
- carries `last_seq`: the final wal `seq` covered by the snapshot. Recovery
  with a snapshot starts replay from `last_seq + 1`, events before that
  boundary come from the snapshot;
- a missing, truncated, corrupted (checksum), or stale (mid-write, `last_seq`
  beyond the wal's verified prefix) snapshot degrades to the full wal replay.

### Error surface

`WalError` distinguishes environment failures (`Io { path, source }`) from
format failures (`InvalidHeader`, whose `HeaderError` carries `BadMagic`,
`UnsupportedVersion`, `PayloadTooLarge`, `TruncatedFrame`; and
`ChecksumMismatch`), from the logically forbidden (`SeqMismatch`,
`ChangedWhileOpen` — appending after a foreign writer modified the file). Only
the format and append paths may produce errors; recovery turns format
anomalies into truncation or a hard `SeqMismatch`, never into silence.

## Consequences

### Benefits
- Acknowledged = durable: the fsync boundary means a crash can only roll
  acknowledged work back to a *verified record boundary*, never to a torn
  frame. `tests/wal_e2e.rs` proves it by truncating the wal at *every* byte
  offset of a 16-batch mixed journal and asserting the rebuilt ledger equals
  exactly the acked prefix, byte-for-byte, plus `verify_invariants`.
- The ledger stays pure and deterministic (ADR-0002, ADR-0003): replay from
  wal records reproduces state exactly, because replay rides the same
  `apply_event` code path as live commits.
- Torn-tail and corruption are detected with CRC32C on every frame; cost is
  negligible next to fsync.
- Snapshot + wal compose with raft later: one proposer owns the wal, followers
  replay the same journal.

### Costs & Trade-Offs
- Per-append fsync bounds append throughput to the device's sync rate
  (~4.5 MB/s for 16 KiB batches on this machine, ~3.6 ms/batch). This is the
  honest price of durable acknowledgment; the ingest layer can amortize it
  with explicit sync-batching (Phase 3) without changing this contract.
- No CRC of the *logical* payload content (postcard already encodes schema
  version; a valid-checksum, wrong-semantics payload fails at ledger replay,
  which is the correct failure domain).
- Recovery scan is O(file); snapshots bound restart replay for large journals.

## Runbook

Failure modes and operator response (see also `docs/WAL-RUNBOOK.md`):

| Symptom | Cause | Response |
| --- | --- | --- |
| `WalError::Io` on open/append | disk failure, permissions, ENOSPC | the wal path is the money path; alert, do not auto-retry commits |
| `Recovery.truncated_bytes > 0` after crash | torn tail from a crash mid-append | expected; recovery already truncated to the last verified record, acked work is intact |
| `WalError::SeqMismatch` on open | foreign writer or corrupted length field | fail the node; do not truncate blindly — restore from backup/snapshot + verify |
| `WalError::ChangedWhileOpen` on append | a second flush/rename touched the file | single-writer contract violated; restart the node |
| snapshot missing/stale/corrupt on load | mid-write, crash, or bit rot | transparent fallback: full wal replay rebuilds state |
| snapshot shrinks available space | journal growth | checkpoint on a schedule (raft term boundary, Phase 3) |

## Alternatives Considered

- **bincode (`2.0`) as the batch codec:** rejected at the crate-research gate —
  v3.0.0 is a tombstone that does not compile; the maintainers retired the
  project. postcard `1.1.3` (verified on docs.rs) is actively maintained,
  derives `serde`, and its 1.x wire format is stable.
- **`rkyv` zero-copy records:** a niche speedup with no value at wal/write
  rates dominated by fsync; more unsafe surface for no measured gain.
- **Length-map header / record count:** indexes can drift from bytes; prefix
  scan recovery is simpler to prove correct.
- **Tail-trim on every `SeqMismatch`:** a verified-but-diverged prefix is
  history owned by a previous run — truncating it would silently destroy
  acked money, so it is a hard error instead.
- **CRC32 (IEEE) instead of CRC32C:** equivalent integrity, worse hardware
  support on ARM/AArch64; CRC32C wins on both.
- **`fsync` the file after recovery truncation:** done — `set_len` is followed
  by a barrier so a crash cannot resurrect the deleted tail on some
  filesystems (ext4 historically).

## References

- `crates/wal/src/{record.rs, wal.rs, snapshot.rs, error.rs}` — framing,
  recovery, snapshots, error types.
- `crates/wal/tests/{crash.rs, properties.rs}` — byte-boundary crash sweeps and
  prefix invariants; `crates/wal/benches/persistence.rs` — durability numbers.
- `crates/ledger-core/src/{codec.rs, ledger.rs}` — postcard batch mapping and
  the `prepare_batch`/`commit_events`/`apply_event` split.
- `crates/ledger-core/tests/wal_e2e.rs` — end-to-end ack/recovery/replay proof.
- ADR-0003 (journal versioning), ADR-0006 (compute-then-write ordering).