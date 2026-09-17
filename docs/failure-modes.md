# Failure modes and resilience

This document focuses on the failure conditions that matter most to a ledger system: durability, split-brain, drift, and settlement proof correctness.

## 1. Ingress overload

Under burst traffic, the system must reject or shed load rather than silently consume memory.

TrustLedger uses a bounded ingress queue and a single-writer engine to prevent unbounded buffering. If the queue is saturated, the request fails fast rather than letting the process degrade into an OOM path.

## 2. Partial writes and torn records

The WAL is designed to handle power loss and partial writes. Recovery scans the file, validates checksums, and truncates only the corrupt tail.

This ensures that acknowledged history remains valid and the system can recover without inventing new state.

## 3. Replication and partition failures

In a 3-node raft configuration, a minority node can be isolated without inhibiting the majority from making progress. The project intentionally models leader failover and partition behavior so they are handled as deterministic system events, not surprising edge cases.

## 4. Financial drift

Reconciliation is treated as a core operational concern. A non-zero reconciliation drift is a signal that app-state and ledger-state have diverged and require investigation.

The demo app models this path explicitly because financial systems are only as trustworthy as their reconciliation story.

## 5. Settlement finality risk

The off-chain ledger remains authoritative even when Solana RPC or settlement submission is delayed or temporarily unavailable. The settlement layer is designed to record proof state and reattempt confirmation without blocking core ledger operations.

## 6. Operational summary

The real project risk is not one dramatic bug. It is a combination of small systemic failures:

- queue saturation
- torn storage writes
- partitioned consensus
- data drift between views of the same money
- external settlement delays

TrustLedger addresses these by making the failure boundaries explicit and testable.
