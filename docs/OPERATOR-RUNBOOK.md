# TrustLedger: Production Operator Runbook

> **Target Audience:** On-Call Site Reliability Engineers (SRE), Systems Operators  
> **Classification:** Production Operations Playbook  
> **Last Updated:** 2026-09-15

---

## 1. Quick Incident Response Summary

| Alert / Symptom | Severity | First Action | Secondary Action |
| :--- | :--- | :--- | :--- |
| `trustledger_reconciliation_drift_gauge != 0` | **SEV-1** | Halt automated payout rails (`POST /api/rails/pause`) | Inspect `GET /api/reconciliation` incident report |
| Raft Leader Missing / Election Failure | **SEV-1** | Inspect `docker logs trustledger-raft-1` | Verify network partition via `docker exec` ping |
| Ingress 429 Spike (`RESOURCE_EXHAUSTED`) | **SEV-2** | Check queue backlog metrics on Prometheus `:9090` | Scale consumer threads or adjust `max_batch_size` |
| WAL CRC32C Torn Write on Boot | **SEV-2** | Review automatic recovery logs | If manual truncation needed, follow §4 |

---

## 2. Cluster Health Inspection & Prometheus Dashboards

### 2.1 Live Health Check
```bash
# Check HTTP API and Prometheus Metrics
curl -s http://localhost:8080/metrics | grep trustledger_

# Check Cluster Node Endpoints
curl -s http://localhost:50051/health
curl -s http://localhost:50052/health
curl -s http://localhost:50053/health
```

### 2.2 Key Operational Metrics to Monitor
- `trustledger_transfers_committed_total`: Total transfers posted to immutable ledger.
- `trustledger_reconciliation_drift_gauge`: Must be strictly `0`. Any non-zero value indicates internal accounting divergence.
- `trustledger_ingress_batch_size_bucket`: Inspect batch efficiency (target: > 100 transfers/batch).
- `trustledger_wal_sync_duration_seconds`: Storage fsync latency p99 (target: < 5ms).

---

## 3. Investigating Reconciliation Drift (SEV-1)

When `trustledger_reconciliation_drift_gauge` moves from `0`:

1. **Trigger Immediate Reconciliation Audit:**
   ```bash
   curl -X GET http://localhost:8080/api/reconciliation | jq .
   ```
2. **Review Incident Object:**
   ```json
   {
     "status": "DriftDetected",
     "drift": -10000,
     "incident": {
       "incident_id": "INC-1700000200-9842",
       "drift_amount": -10000,
       "details": "Settled payments total (55000) exceeds ledger recorded credits (45000)",
       "recommended_action": "Triage payment ID discrepancy and replay missing ledger batch"
     }
   }
   ```
3. **Remediation Protocol:**
   - Query payment lifecycle log for payments in `Captured` state without a corresponding `posted_transfer_id`.
   - Replay missing transfer commands using client idempotency keys (`Idempotency-Key` header).

---

## 4. Disaster Recovery & Manual WAL Replay

If a storage node experiences hardware corruption or ungraceful shutdown:

1. **Inspect WAL Integrity:**
   ```bash
   cargo run -p wal --bin wal-inspect -- --path /data/wal/ledger.wal
   ```
2. **Execute Safe Recovery & Truncation:**
   TrustLedger's `wal` engine automatically detects torn frames at EOF and safely truncates corrupt trailing bytes:
   ```bash
   # Start node with recovery mode flag
   RUST_LOG=info trustledger-node --wal-path /data/wal/ledger.wal --recover
   ```
3. **Replay Journal into In-Memory Balance Cache:**
   The state machine reconstructs accounts deterministically from sequence 1:
   ```rust
   let ledger = Ledger::replay(scale, &journal_events)?;
   ledger.verify_invariants()?;
   ```

---

## 5. Running the Deterministic Simulator (DST) for Incident Post-Mortem

When diagnosing an edge-case concurrency or partition race condition discovered in staging:

1. **Run Full Scenario Suite:**
   ```bash
   cargo run -p simulator -- --scenario all --steps 300
   ```
2. **Run Seeded Replay (100% Deterministic Reproducibility):**
   ```bash
   # Re-run exact sequence that produced an invariant violation
   cargo run -p simulator -- --seed 0xDEADBEEF --scenario soak --steps 500
   ```
3. **Run Multi-Seed Fuzz Campaign:**
   ```bash
   # Fuzz 50 consecutive seeds under random network loss and crash faults
   cargo run -p simulator -- --fuzz 50 --steps 150
   ```
