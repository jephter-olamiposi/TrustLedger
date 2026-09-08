# Benchmarks

## ledger-core hot paths (criterion)

Measured with the criterion harness at `crates/ledger-core/benches/throughput.rs`
under `profile.release` (`lto="thin"`, `codegen-units=1`, `overflow-checks=true`).
Criterion reports statistical medians over 100 samples (warmup 3 s,
measurement 5 s), which removes the warm-up and frequency effects that made the
old one-shot harness load-sensitive.

Last measured run: 2026-09-08 (criterion 0.7.0, post undo-log `apply_batch`).

| Workload | Throughput | ns/op |
| --- | --- | --- |
| `create_transfer` (immediate) | ~4.4M transfers/sec | ~229 |
| two-phase `create_pending` + `post_pending` round-trip | ~1.6M pairs/sec | ~627 |
| `apply_batch` (256 transfers/batch) | ~3.4M transfers/sec | ~297 |
| `Ledger::replay` (journal events) | ~4.9M events/sec | ~206 |

`apply_batch` is reported per transfer: the criterion iteration is a 256-batch
(76.1 µs per iteration), and `~297 ns/op` is that median divided by the batch
size. `replay` is 41.2 ms per 200,000-event journal (~206 ns/event), including
`verify_invariants` over the rebuilt ledger.

## wal persistence (zero-dependency harness)

`crates/wal/benches/persistence.rs` (`cargo bench -p wal`), same release
profile and machine. 10,000 batches of 16 KiB each. This stays a plain
`Instant` harness on purpose: `append` measures the device fsync rate, which
criterion's iteration model would not make more meaningful (ADR-0008).

| Workload | Throughput |
| --- | --- |
| `append` (fsync per batch, `sync_per_append: true`) | ~4.5 MB/s (~274 batches/s, ~3.7 ms/batch) |
| `append` (buffered, `sync_per_append: false`) | ~660–780 MB/s (~40–47k batches/s) |
| recover + replay 10,000 batches | ~0.5–1.3 GB/s (load-varying) |

### Reading these numbers

- **The per-batch fsync is the durability boundary** (ADR-0007): the ~3.7 ms
  is the device sync latency, not the codec or checksum. CRC32C checksum cost
  is ~2 orders of magnitude below the sync, so verifying every frame is
  essentially free next to durability.
- **Recovery** is a streaming one-frame-at-a-time scan
  ([`record::next_record`] shares one scanner with `read_records`, so neither
  allocates the whole log). The measured spread (~0.5–1.3 GB/s) is machine
  load, not the scan. A 100 MB journal recovers in well under a second even at
  the low end, which is why snapshots only matter for far larger journals.
- A busy node wanting higher ack throughput must batch fsyncs explicitly
  (ingest, Phase 3) — that is a policy choice, not a wal contract change.

## Reading these numbers

- **`create_transfer` exceeds the >1M transfers/sec claim in ADR-0001**,
  confirming the `BTreeMap` determinism decision (ADR-0002) costs nothing
  measurable on the hot path. All four workloads clear it.
- **`apply_batch` runs within ~30% of single `create_transfer`**: it applies
  speculatively with an undo log of per-event pre-images instead of
  snapshotting the whole ledger, so rollback cost scales with *batch size*, not
  ledger size. The remaining gap is the batch's event `Vec` and undo
  allocations; the ledger's clone path is gone from `apply_batch`.
- **`replay` at ~4.9M events/sec** validates journal-as-truth economics: a
  1M-event ledger replays in ~0.2 s, so state rebuild on restart is cheap.
- The two-phase round-trip (~627 ns) is the only workload that moves two
  transfers per iteration (pending create + post), so it is ~2.7x the single
  `create_transfer` cost — the state machine, not the math.

## Regression protocol

The ledger-core numbers are a **CI regression gate** (`scripts/bench_gate.py`
in the quality job, ADR-0008):

1. Run `cargo bench -p ledger-core --bench throughput -- --save-baseline gate`.
2. Each measured median is compared against `docs/benchmarks/baseline.json`;
   the gate fails when a median exceeds `recorded * tolerance_factor` (2.0).
3. The 2x factor deliberately absorbs the difference between CI's
   `ubuntu-latest` runner and developer machines; it is a loud early-warning
   net for ≥2x regressions, not a precise comparator.

Re-record the baseline after **intentional** performance work: record the new
`target/criterion/*/gate/estimates.json` medians into `baseline.json` with the
protocol in the script header, and update the table above.