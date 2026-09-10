# ADR-0009: Distributed Consensus via OpenRaft and Replicated State Machine

- **Status:** Accepted
- **Date:** 2026-09-10
- **Author:** Jephter Olaifa

---

## Context

TrustLedger's settlement guarantees require zero lost committed transfers and strict single-writer
consistency even when individual server nodes crash, restart, or experience network partitions.
Single-node WAL persistence (ADR-0007) guarantees crash recovery on a single machine, but hardware
failure, data center loss, or machine maintenance requires distributed active replication.

In `plans/trustledger/01_PROJECT_PLAN.md` (Pillar C, Phase 4), we require:
- A 3-node consensus cluster where $Q = \lfloor N/2 \rfloor + 1 = 2$ nodes form a quorum;
- Strongly consistent log replication where entries commit only after durable replication across a quorum;
- A replicated state machine that applies entries strictly in committed sequence;
- Automatic failover in under 2 seconds upon leader crash;
- Resilient recovery under network partitions without split-brain or data drift.

## Decision

We adopt **OpenRaft 0.9** (`openraft = "0.9.25"`) with the `"storage-v2"` decoupled storage architecture:

1. **Raft Algorithm vs Alternatives (e.g. VSR, Paxos, Zab):**
   - We selected Raft over Viewstamped Replication (VSR) and Multi-Paxos due to its strict leader-driven
     invariants, explicit term/index ordering, well-defined membership transitions, and extensive industry
     verification (used in Databend, TiKV, etc.).
   - Raft's single-leader model aligns directly with TrustLedger's single-writer ledger execution core: only
     the elected leader accepts client writes, commits them to the distributed log, and replicates to followers.

2. **Decoupled Storage Architecture (`storage-v2`):**
   - `RaftLogStorage`: Manages append-only Raft log entries, index ranges, truncations, and persistent votes.
   - `RaftStateMachine`: Manages the deterministic in-memory `Ledger` (`ledger-core`), applying committed entries
     sequentially and producing point-in-time snapshots using compact `postcard` serialization.
   - Separating log storage from the state machine allows independent optimizations: log replication is sequential
     and disk-bound, while state machine execution is pure CPU and cache-locality bound.

3. **Deterministic Fault Injection via In-Memory Network Router:**
   - To rigorously prove consensus invariants (no split-brain, zero lost transfers, minority partition tolerance),
     we provide a simulated `NetworkRouter` that intercepts cross-node messages.
   - The router supports dynamic network partitioning (`partition([1], [2, 3])`), packet drops, and healing,
     allowing fast deterministic Jepsen-style tests in CI without flaky socket bindings or OS port exhaustion.

4. **Quorum & Split-Brain Prevention:**
   - In a 3-node cluster, a partitioned leader with 1 node cannot acquire quorum acknowledgments ($1 < 2$) and
     cannot commit client writes.
   - The majority partition (2 nodes) detects missed heartbeats, elects a new leader with an incremented term,
     and continues committing operations.
   - When the partition heals, the old leader receives higher-term heartbeats, reverts to follower status,
     truncates any uncommitted divergent log entries, and replays missing committed entries to synchronize its ledger.

## Consequences

- **Positive:**
  - Zero lost commits under single-node failure ($N=3, F=1$).
  - Provable linearizability of ledger operations through the Raft log commit index.
  - Testable partition and leader-kill scenarios executed deterministically in unit and integration tests.
  - License and supply-chain compliant with `deny.toml` (MIT OR Apache-2.0).

- **Trade-offs:**
  - Client write latency includes Raft network replication round-trip time across a quorum before commit.
  - Reads must go through the leader or use lease reads/read-index checks to ensure linearizable reads.
