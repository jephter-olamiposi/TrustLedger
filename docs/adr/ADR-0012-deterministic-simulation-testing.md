# ADR-0012: Deterministic Simulation Testing (DST), Telemetry, and Production Operations

- **Status:** Accepted
- **Date:** 2026-09-15
- **Author:** Jephter Olaifa

---

## Context

Financial ledgers and distributed consensus engines operate under hostile conditions: sudden node power failures, asymmetric network partitions, disk torn writes, and unpredictable client bursts. Traditional integration testing frameworks suffer from fatal shortcomings when validating distributed systems:
1. **Flakiness and Non-Determinism:** Tests that rely on `std::thread::sleep` or operating system wall-clock timers fluctuate based on host CPU contention and background workloads.
2. **Irreproducibility:** Rare edge-case bugs (e.g. partition transitions during leader election with concurrent in-flight writes) cannot be reliably reproduced once detected in CI.
3. **Slow Execution:** Simulating time-dependent failure recovery (e.g. heartbeat timeouts, partition healing) requires real wall-clock seconds, severely restricting the number of state machine interleavings that can be explored.

To achieve Tier-1 systems verification comparable to TigerBeetle and FoundationDB, TrustLedger requires a fully deterministic, seeded simulation harness (DST) capable of exploring millions of state transitions under chaos fault injection in milliseconds.

---

## Decision

We implement a discrete-event Deterministic Simulation Testing harness (`simulator/`) and production telemetry suite (`crates/observability`).

### 1. Discrete Virtual Time Scheduler (`SimClock`)
- Eliminates reliance on `std::time::Instant` and system wall clocks.
- Virtual time is measured in discrete integer ticks (`SimInstant(u64)`).
- Events are scheduled in a priority min-heap (`BinaryHeap<ScheduledEvent<T>>`).
- Time dilates deterministically: 1 hour of simulated cluster execution completes in ~20ms of CPU time without blocking threads.

### 2. Seeded Pseudo-Random Number Generator (`SimRng`)
- All randomness is governed by `rand_chacha::ChaCha8Rng` initialized with an immutable 64-bit seed (`u64`).
- Every scheduling decision, message delivery delay, packet drop, disk stall, and crash decision derives deterministically from this PRNG stream.
- **Guarantee:** Given seed $S$, running the simulation yields byte-for-byte identical execution traces on any machine.

### 3. Fault-Injected Network Transport (`SimNetwork`)
- Models inter-node communication with configurable fault parameters:
  - Latency jitter: packets arrive at `now + range(min_latency..=max_latency)`.
  - Packet loss: random drop with probability $P_{\text{drop}}$.
  - Packet duplication: duplicates scheduled with delay jitter with probability $P_{\text{dup}}$.
  - Dynamic network partitions: `partition(set_a, set_b)` severs bidirectional connectivity; `isolate(node)` completely disconnects a node; `heal()` restores full connectivity.

### 4. Fault-Injected Storage Engine (`SimDisk`)
- Simulates volatile OS page cache buffering vs. non-volatile durable storage:
  - `write(data)`: Appends to un-fsynced volatile buffer.
  - `sync()`: Flushes volatile buffer to durable media (simulating fsync).
  - `crash(rng, torn_write)`: Power failure discards un-fsynced volatile memory. If `torn_write` is active, persists a partial prefix of the buffer, truncating the frame to exercise CRC32C recovery.

### 5. Strictly Sequential State Machine Replication (`SimCluster`)
- Simulates a 3-node cluster executing consensus proposals and state machine updates.
- **Sequential Commit Invariant:** Nodes buffer out-of-order quorum commit notifications in `pending_commits` and apply them to `ledger-core` strictly in monotonic log index order ($1, 2, 3 \dots$).
- Canonical commit timestamps are assigned by the leader, guaranteeing that all replicas compute byte-for-byte identical journals and state hashes.

### 6. Invariant Checking Oracles (`Oracle`)
- Inspected at every tick boundary and upon simulation quiescence:
  1. **Wealth Conservation Invariant:** $\sum_{a \in \text{Accounts}} \text{posted\_balance}(a) \equiv \text{initial\_supply}$. Zero balance drift.
  2. **Non-Negative Balances:** No account ever drops below zero unless overdraft flags permit.
  3. **Log Agreement (Linearizability):** For all committed log indices $0 \dots K$ across all active nodes, journal events must match identically.

### 7. Production Telemetry & Observability (`crates/observability`)
- Lock-free Prometheus registry providing atomic `Counter`, `Gauge`, and fixed-bucket `Histogram`.
- Exposes standard `/metrics` endpoint in Prometheus 2.0 text format for scraping by external monitoring systems.

---

## Consequences

### Positive
- **Instant Reproducibility:** Any failure encountered in CI outputs the exact CLI replay command:
  `cargo run -p simulator -- --seed <SEED> --scenario <NAME> --steps <N>`
- **High Test Density:** Fuzz campaigns can exercise 100+ random seeds in under 1 second, validating millions of edge-case transitions.
- **Production Confidence:** Proves that network partitions, packet drops, and torn-write disk crashes never corrupt financial balances.

### Negative / Trade-offs
- Simulation models must be kept synchronized with protocol evolutions in `crates/raft` and `crates/ledger-core`.
