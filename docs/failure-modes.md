# TrustLedger: Failure Modes and Effects Analysis (FMEA)

> **Document Classification:** Engineering Architecture & Operational Resilience  
> **Target Audience:** Staff/Principal Infrastructure Engineers, SRE, Security Auditors  
> **Status:** Production Standard

This document catalogs all critical failure modes across TrustLedger's five architectural layers:
1. **Network Ingress & Load Shedding (`crates/ingest`)**
2. **Consensus & Replication (`crates/raft`)**
3. **Storage & Durability (`crates/wal`)**
4. **On-Chain Solana Settlement (`crates/solana-settle`)**
5. **Continuous 3-Way Reconciliation (`apps/demo`)**

---

## 1. Network Ingress & Gateway Layer

### 1.1 Ingress Queue Saturation & Memory Exhaustion (OOM)
- **Failure Trigger:** Sudden 10x–50x burst of concurrent transfer requests exceeding system throughput capacity.
- **Vulnerability without Protection:** Unbounded buffering accumulates millions of heap allocations, triggering OS OOM Killer.
- **TrustLedger Mitigation:** Bounded channel ingress (`IngestQueue` backed by fixed capacity Tokio `mpsc`). When channel reaches capacity, excess requests are rejected immediately with canonical `RESOURCE_EXHAUSTED` (HTTP `429 Too Many Requests`), dropping latency for inflight requests to near zero.
- **Recovery:** Automatic load shedding protects memory; downstream clients back off via jittered exponential backoff.
- **SLI / Alert:** `trustledger_transfers_rejected_total` rate > 5% over 1-minute window.

### 1.2 Webhook Signature Tampering & Delayed Replay
- **Failure Trigger:** Man-in-the-middle attacker tampers with card payment webhook payload or replays a captured successful webhook from 24 hours ago.
- **Vulnerability without Protection:** Double-crediting accounts or processing fraudulent transactions.
- **TrustLedger Mitigation:**
  1. *Constant-Time Verification:* HMAC-SHA256 evaluated using `subtle::ConstantTimeEq` to prevent timing side-channel analysis.
  2. *Timestamp Freshness Window:* Requests with timestamps older than 300 seconds are rejected with `400 Bad Request`.
  3. *Atomic Event Deduplication:* `WebhookDeduplicator` tracks `event_id` in memory, returning `409 Conflict` on duplicate replay.

---

## 2. Distributed Consensus & Replication Layer (`crates/raft`)

### 2.1 Minority Node Isolation (Network Partition)
- **Failure Trigger:** Cross-rack switch failure isolates 1 node in a 3-node cluster.
- **Behavior:**
  - Majority cluster ($N=2$) retains quorum ($\ge 2/3$) and continues processing proposals and committing transactions uninterrupted.
  - Isolated minority node ($N=1$) cannot gather quorum acknowledgments; all isolated proposals stall or reject.
- **Split-Brain Mitigation:** OpenRaft leader lease requires active heartbeats to majority; minority cannot elect a leader or commit entries.
- **Healing:** When partition heals, the isolated node reconnects and receives an incremental catch-up log stream, resuming normal state.

### 2.2 Leader Crash During In-Flight Quorum Commit
- **Failure Trigger:** Leader hardware kernel panic or power loss immediately after broadcasting proposals.
- **Behavior:**
  - Follower heartbeat election timer expires (< 500ms).
  - A follower increments consensus term and solicits votes.
  - A new leader is elected by majority quorum.
  - Any proposal that achieved majority replication before the crash is committed by the new leader; un-replicated proposals are dropped safely.
- **Zero Loss Guarantee:** Clients only receive a successful response AFTER quorum commit. Unconfirmed proposals can be retried safely using idempotent keys.

---

## 3. Storage & Durability Layer (`crates/wal`)

### 3.1 Power Cut Mid-Write (Torn Write / Frame Truncation)
- **Failure Trigger:** Physical power outage while the kernel is flushing an uncompleted 4KB disk sector.
- **Vulnerability without Protection:** Half-written binary record corrupts the entire log file, preventing engine restart.
- **TrustLedger Mitigation:**
  - Fixed binary framing: `[ Magic (4B) | Seq (8B) | Length (4B) | CRC32C (4B) | Payload (NB) ]`.
  - On restart, `Wal::recover()` sequentially scans and validates CRC32C checksums for every record.
  - Upon encountering the first torn/truncated record, recovery verifies that the trailing garbage is at EOF, truncates the file back to the last intact frame, and reports clean recovery.
- **Verification:** Proven by `crates/wal/tests/crash.rs` and the deterministic DST harness (`simulator/src/storage.rs`).

### 3.2 Disk Exhaustion (ENOSPC)
- **Failure Trigger:** Influx of transfers fills available volume storage.
- **TrustLedger Mitigation:** Checkpoint snapshots. Periodic compaction exports in-memory `Ledger` state into an immutable snapshot file (`.snap`), truncating historical WAL segments.
- **Runbook Action:** Run `trustledger-admin snapshot compact` and expand mount point volume.

---

## 4. On-Chain Settlement Layer (`crates/solana-settle`)

### 4.1 Solana RPC Node Timeout & Dropped Transactions
- **Failure Trigger:** Public or private Solana RPC node drops or times out during network congestion.
- **Mitigation:**
  - Off-chain double-entry ledger is authoritative and continues processing transactions without blocking on external RPC.
  - Settlement adapter retains batch state in `PendingSettlement` status.
  - Worker re-queries recent blockhashes and retries settlement instruction submission with exponential backoff.
- **Auditability:** Inclusion proofs remain verifiable once the batch root transaction confirms.

### 4.2 On-Chain Settlement Sequence Collision
- **Failure Trigger:** Multiple settlement workers attempt concurrent commits for the same epoch.
- **Mitigation:** Anchor PDA seeds `[b"settlement_root", authority.key(), &epoch.to_le_bytes()]` enforce deterministic single-ownership per epoch. Attempted duplicate writes fail on-chain with `AccountAlreadyInitialized`.

---

## 5. Continuous 3-Way Reconciliation (`apps/demo`)

### 5.1 Financial Balance Drift ($\text{Drift} \neq 0$)
- **Failure Trigger:** External payment gateway logs capture event, but ledger transaction fails, or internal state is tampered with.
- **Detection:** `ReconciliationEngine::audit()` runs every 60 seconds comparing:
  $$\text{Drift} = \text{Ledger Settled Balance} - \text{Application Captured Payments}$$
- **Automated Escalation:**
  - Any discrepancy instantly updates gauge `trustledger_reconciliation_drift_gauge` to non-zero.
  - Increments `trustledger_reconciliation_incidents_total`.
  - Emits high-priority structured alert with incident ID, drifted accounts, and recommended operational rollback.
