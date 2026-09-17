# Operator runbook

This runbook is a practical guide for engineers operating the ledger during incidents, restarts, or reconciliation problems.

## 1. Quick checks

Check the API and metrics first:

```bash
curl -s http://localhost:8080/metrics | grep trustledger_
curl -X GET http://localhost:8080/api/reconciliation
```

Focus on:

- reconciliation drift
- ingress rejection rate
- WAL recovery warnings
- raft leader health

## 2. Reconciliation drift

If reconciliation shows non-zero drift, treat it as a priority incident.

1. inspect the reconciliation report
2. identify any captured payments without matching ledger mutations
3. verify payment lifecycle state and transfer IDs
4. replay or correct the missing ledger state using idempotent client keys

This should be resolved before allowing settlement or payout flows to continue automatically.

## 3. WAL recovery

If the process restarts and the WAL reports torn tail or recovery truncation, this is usually expected behavior after a crash.

The usual recovery path is:

1. inspect the WAL error
2. verify the maintained prefix is intact
3. rebuild ledger state from the verified journal
4. confirm invariants still hold before resuming writes

Do not guess past a sequence mismatch; that indicates a more serious file integrity issue.

## 4. Raft health

If the raft cluster loses leadership or cannot form a majority:

1. inspect node logs
2. verify connectivity between peers
3. confirm the cluster is not partitioned beyond quorum tolerance
4. wait for the leader election process to settle before resuming writes

The system is designed for majority-based progress, not single-node certainty.

## 5. Settlement delays

If Solana settlement is delayed:

- keep the off-chain ledger authoritative
- retain pending batch state
- retry settlement with bounded backoff
- continue auditing proof generation and batch continuity

External RPC latency should not block core accounting progress.

## 6. Escalation

Escalate when:

- reconciliation drift remains non-zero after investigation
- WAL sequence mismatch appears
- a cluster cannot elect or maintain a leader
- ledger invariants fail during replay

These are system-level issues, not normal operational noise.
