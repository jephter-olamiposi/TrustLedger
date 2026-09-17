# Benchmarks

This document tracks recorded hot-path measurements for the ledger core and related durability paths. The figures are reference results from the benchmark harness, not capacity guarantees for a production deployment.

The results are machine- and load-dependent. Re-run the harnesses before using them for capacity planning.

## ledger-core

The main criterion harness is in `crates/ledger-core/benches/throughput.rs` and is intended to measure the critical transfer and replay paths in release mode.

| Workload | Result |
| --- | --- |
| `create_transfer` | ~4.4M transfers/sec |
| two-phase pending + post round trip | ~1.6M pairs/sec |
| `apply_batch` (256 transfers) | ~3.4M transfers/sec |
| `Ledger::replay` | ~4.9M events/sec |

These are the hot paths most relevant to the accounting engine: mutation creation, batch execution, and rebuild after crash recovery.

## WAL durability

The WAL benchmark is implemented in `crates/wal/benches/persistence.rs` and measures fsync behavior on durable appends. The documented values are recorded reference results; a local run may differ substantially by filesystem and host load.

| Workload | Result |
| --- | --- |
| fsync-per-append | ~4.5 MB/s |
| buffered append | ~660–780 MB/s |
| recovery replay | ~0.5–1.3 GB/s |

The important point is that the durability ceiling is dominated by device sync latency. The checksum and record framing are relatively cheap compared with a real synchronous write.

## MMR and settlement proofs

The Merkle and settlement layer is measured separately because its workload is proof generation and verification rather than ledger mutation.

| Operation | Profile |
| --- | --- |
| MMR append | ~820 ns / leaf |
| root computation | ~0.84 ms / 1024-leaf batch |
| inclusion proof generation | ~4.2 µs / proof |
| proof verification | ~3.6 µs |

This is small enough to be practical in a settlement flow without making the system inherently proof-bound.

## Deterministic simulation testing

The simulator measures resilience under faults such as partitions, leader loss, packet loss, and torn-write recovery.

| Scenario | Result |
| --- | --- |
| `NetworkPartition` | passed, zero drift |
| `CrashTornWrite` | passed, full recovery |
| `ChaosSoak` | passed, log agreement maintained |
| fuzz campaign | 0 violations across seeded scenarios |

This is the project’s main way to validate that failure behavior is not just described but actively tested.

## Reading the numbers

- These measurements are regression targets, not marketing metrics.
- The WAL durability numbers reflect raw storage latency more than application logic.
- The accounting path is fast enough to support micro-batching without sacrificing deterministic behavior.
- The MMR and proof layer stays comfortably within practical settlement-latency constraints.

The purpose of benchmarking here is to protect correctness and avoid silent regressions in high-leverage paths.