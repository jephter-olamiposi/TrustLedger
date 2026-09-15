# Phase 7 Implementation Plan: Deterministic Simulation Testing (DST), Telemetry & Production Operations

## 1. Context & Architecture

TrustLedger is a distributed settlement ledger with on-chain Solana finality. Phases 1 through 6 completed the core state machine (`ledger-core`), durable storage (`wal`), gRPC ingress & micro-batching (`ingest`), 3-node consensus (`raft`), Merkle Mountain Range & Anchor settlement (`merkle`, `solana-settle`), and reference client & reconciliation (`apps/demo`).

Phase 7 completes the tier-1 distributed systems requirements:
1. **FoundationDB / TigerBeetle-style Deterministic Simulation Testing (DST)** (`simulator/`):
   - Discrete virtual time scheduler (`SimClock`).
   - Deterministic pseudo-random number generator (`rand_chacha::ChaCha8Rng`) seeded with a single `u64`.
   - Virtual fault-injected transport (`SimNetwork`): drops, delays, reordering, duplicate packets, and dynamic network partitions.
   - Virtual fault-injected storage (`SimDisk`): write stalls, crash-restarts, torn-frame recovery, and un-fsynced loss.
   - Financial invariant oracle: wealth conservation ($\sum \text{balances} + \sum \text{holds} \equiv \text{constant}$), zero negative balances, monotonic log equivalence, and byte-for-byte reproducibility across runs with the same seed.
2. **Production Telemetry & Observability** (`crates/observability`):
   - Lightweight, lock-free Prometheus metrics registry (`Counter`, `Gauge`, `Histogram`).
   - Exposes `/metrics` endpoint in Prometheus 2.0 text exposition format for `apps/demo` and `crates/ingest`.
   - Captures transfer throughput, batch size histograms, WAL fsync latencies (p50/p95/p99), and continuous reconciliation drift gauges.
3. **Production Operations & Failure Modes**:
   - `docs/failure-modes.md`: Comprehensive Failure Mode and Effects Analysis (FMEA) covering consensus partitions, storage corruption, backpressure shedding, and reconciliation drift.
   - `docs/OPERATOR-RUNBOOK.md`: SRE incident mitigation and recovery playbooks.
   - `docker/docker-compose.yml`: One-shot orchestration launching multi-node cluster, Prometheus scraper, and demo application.
4. **Architecture Decision Record & Benchmarks**:
   - `docs/adr/ADR-0012-deterministic-simulation-testing.md`.
   - `docs/BENCHMARKS.md`: Updated comprehensive statistical report.
   - `README.md`: Flagship presentation and verification guide.

---

## 2. Risks & Mitigations

1. **Non-Determinism Leakage in Simulator**:
   - *Risk:* Calling `std::time::Instant::now()`, `std::thread::sleep`, or non-deterministic OS hash functions inside the simulated state machine destroys replayability.
   - *Mitigation:* All scheduling and timeouts run through `SimClock` virtual ticks. PRNG uses `ChaCha8Rng` with explicit `u64` seeds. Collections in the simulator use deterministic `BTreeMap` and `BinaryHeap`.
2. **Disk Space Constraint on Host Machine**:
   - *Risk:* Host machine has limited free disk space (~2.8 GiB available in `/System/Volumes/Data`).
   - *Mitigation:* In-memory virtual storage (`SimDisk`) for the simulator without creating gigabyte-sized disk files. Do not build excessive release targets; keep cargo builds incremental and clean.
3. **Supply-Chain & License Gate**:
   - *Risk:* Adding third-party metrics or simulation crates that violate `deny.toml` (e.g. GPL, unmaintained, or MSRV > 1.80).
   - *Mitigation:* `rand_chacha` 0.3.1 / `rand` 0.8.8 are already in `Cargo.lock` (Apache-2.0 / MIT). `crates/observability` will be implemented cleanly using standard atomic primitives and zero unmaintained dependencies, guaranteeing 100% compliance with `deny.toml` and MSRV 1.80.
4. **Code Quality & Comment Discipline**:
   - *Risk:* Missing docs or forbidden unwrap/panic on production paths.
   - *Mitigation:* Adhere strictly to the compulsory `comment-cleanup` skill (doc-comments for every public item, field, and method; `# Errors` sections; why-not-what invariant comments; zero `missing_docs` warnings).

---

## 3. Step-by-Step Implementation Sequence

### Step 1: Telemetry & Observability (`crates/observability`)
- Create `crates/observability/Cargo.toml` and add to workspace.
- Implement atomic `Counter`, `Gauge`, `Histogram` with Prometheus buckets.
- Implement `MetricsRegistry` and Prometheus text exporter formatter (`/metrics`).
- Integrate metrics exporter into `apps/demo` (`/metrics` route) and `crates/ingest`.
- Add unit tests verifying metric incrementation, bucket recording, and Prometheus text rendering.

### Step 2: Deterministic Simulation Testing (DST) (`simulator/`)
- Create `simulator/Cargo.toml` and add to workspace.
- `simulator/src/clock.rs`: Discrete virtual time scheduler (`SimClock`, `SimInstant`).
- `simulator/src/rng.rs`: Seeded PRNG (`SimRng`).
- `simulator/src/network.rs`: Virtual network with packet delays, drops, duplicates, and partitions.
- `simulator/src/storage.rs`: Virtual storage with write stalls, crash-restarts, and torn writes.
- `simulator/src/cluster.rs`: Multi-node cluster coordinator executing consensus and replicated state machines.
- `simulator/src/workload.rs`: Synthetic financial workload generator (transfers, pending holds, voids, captures).
- `simulator/src/oracle.rs`: Financial invariant checkers ($\sum$ money conservation, monotonic journals, no negative balance, state hash equivalence).
- `simulator/src/scenario.rs`: Deterministic scenarios (`partition_recovery`, `crash_torn_write`, `chaos_soak`).
- `simulator/src/main.rs`: CLI binary supporting `--seed <u64>`, `--steps <n>`, `--scenario <name>`, `--fuzz <iterations>`.
- `simulator/tests/dst_suite.rs`: Regression test suite validating invariant enforcement and byte-for-byte reproducibility.

### Step 3: Production Operations & Docker Orchestration
- Create `docs/failure-modes.md` (detailed FMEA for consensus, storage, ingress, settlement).
- Create `docs/OPERATOR-RUNBOOK.md` (incident recovery commands and playbooks).
- Create `docker/docker-compose.yml`, `docker/prometheus.yml`, and `docker/Dockerfile` for one-shot cluster demo.

### Step 4: ADR-0012, Benchmarks & Documentation
- Write `docs/adr/ADR-0012-deterministic-simulation-testing.md`.
- Update `docs/BENCHMARKS.md` with full multi-phase performance metrics.
- Update `README.md` with Phase 7 achievements, architecture diagram, DST usage instructions, and metrics.

### Step 5: Verification Gate
- Run `cargo fmt --check`.
- Run `cargo clippy --workspace --all-targets -- -D warnings`.
- Run `cargo test --workspace`.
- Run `./scripts/check` (all gates passing).
