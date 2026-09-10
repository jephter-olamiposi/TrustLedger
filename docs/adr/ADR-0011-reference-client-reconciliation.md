# ADR-0011: Reference Client, Webhook Security, Multi-Rail Settlement, and 3-Way Reconciliation

- **Status:** Accepted
- **Date:** 2026-09-10
- **Author:** Jephter Olaifa

---

## Context

With `ledger-core` (deterministic accounting engine), `wal` (append-only durable log), `ingest` (high-throughput gRPC ingress), `raft` (3-node consensus replication), and `merkle` + `solana-settle` (on-chain cryptographic commitments) in place, TrustLedger requires an end-to-end reference application (`apps/demo`) to:
1. Demonstrate realistic enterprise fintech payment flows (e.g. Stripe/Adyen/Bridge payment lifecycle);
2. Process external card processor webhooks securely with cryptographic signature verification, replay protection, and timestamp freshness windows;
3. Support pluggable multi-rail settlement adapters (Solana USDC via MMR PDA commitment and Mock ACH batch netting);
4. Provide continuous 3-way reconciliation between Application Payments, Ledger Journal, and Solana On-Chain PDA state ($Drift \equiv 0$);
5. Expose an interactive operational dashboard enabling recruiters, partners, and auditors to inspect live balances, audit settlement timelines, and cryptographically verify transfers.

## Decision

We implement the reference client layer in `apps/demo` as an Axum HTTP service and embedded dashboard backed by the TrustLedger engine.

### 1. Payment Lifecycle State Machine
- Payments model standard two-phase card transactions:
  - `Authorized`: Customer funds are reserved on `ledger-core` via a pending hold (`Transfer::new_pending`). Customer available balance decreases immediately, preventing double spending while merchant posted balance remains unaffected.
  - `Captured`: The authorization is executed via `ledger.post_pending()`, finalizing the credit to the merchant's account.
  - `Voided`: The authorization is cancelled via `ledger.void_pending()`, releasing the hold back to the customer's available balance.
  - `Refunded`: Reverse transfer executed from merchant back to customer.
- Idempotency is enforced by an atomic `IdempotencyStore` tracking `(key, state)` transitions (`InFlight` vs `Completed(response)`), preventing duplicate charges under network retries.

### 2. Card Webhook Security
- Webhooks incoming from payment gateways are validated through a multi-tier defense:
  1. **Constant-Time HMAC-SHA256 Verification:** `WebhookVerifier::verify_signature` calculates `HmacSha256` over raw payload bytes and verifies against the hex-encoded header using subtle constant-time comparison (`mac.verify_slice`), mitigating timing attacks.
  2. **Timestamp Freshness:** Webhook event timestamps must fall within a 300-second tolerance window relative to server clock to neutralize delayed replay attacks.
  3. **Atomic Event Deduplication:** `WebhookDeduplicator` records processed `event_id` keys atomically. Duplicate delivery attempts are immediately rejected with `409 Conflict`.

### 3. Pluggable Settlement Rails (`SettlementRail` Trait)
- Defines a uniform interface for external payment and blockchain settlement:
  ```rust
  pub trait SettlementRail: Send + Sync {
      fn name(&self) -> &'static str;
      fn submit_batch(&self, batch_seq: u64, transfers: &[Transfer]) -> Result<RailSubmission, RailError>;
  }
  ```
- **`SolanaUsdcRail`:**
  - Collects settled transfers, constructs an incremental Merkle Mountain Range (MMR), and submits `commit_settlement` instructions to the on-chain Solana PDA.
  - Provides a receipt generation helper that outputs portable cryptographic inclusion proofs (`TransferReceipt`).
- **`MockAchRail`:**
  - Simulates traditional banking rails (Nacha batch netting, routing transit numbers, and clearing settlement delays).

### 4. Continuous Three-Way Reconciliation
- Financial integrity requires automated verification across three independent data stores:
  $$\text{Drift} = \text{Ledger Settled Balance} - \text{App Captured Payments}$$
- `ReconciliationEngine::audit` computes:
  1. `app_total`: Sum of all `PaymentState::Captured` transactions;
  2. `ledger_total`: Aggregate credits posted across merchant accounts in `ledger-core`;
  3. `chain_total`: Total settled volume recorded on the Solana PDA (`SettlementRoot.total_settled_amount`).
- **Invariant:** When $\text{Drift} \equiv 0$ and $\text{chain\_total} == \text{ledger\_total}$, status is `Clean`.
- **Incident Escalation:** Any discrepancy ($\text{Drift} \neq 0$) instantly generates a structured `ReconciliationIncident` with remediation actions for operational triage.

### 5. Interactive Dashboard & Recruiter Surface
- Embedded single-page dashboard rendered via Axum with Tailwind CSS:
  - Live metric cards: Total Settled Volume, Active Merchants, Processed Batches;
  - Real-time transaction journal with state badges;
  - Modal-based transfer verification flow allowing clients to verify their receipt against the on-chain Merkle root directly in the browser or via CLI.

## Consequences

- **Positive:** TrustLedger has a fully executable, senior-citable demonstration surface showcasing full-stack fintech engineering (Axum REST API, state machines, webhooks, multi-rail blockchain settlement, and automated accounting reconciliation).
- **Positive:** Complete test coverage in `tests/e2e.rs` proving end-to-end correctness from payment authorization through on-chain Solana commitment.
- **Negative:** Additional workspace dependencies (`axum`, `serde_json`, `hmac`, `sha2`). All dependencies strictly satisfy MSRV 1.80 and Apache-2.0/MIT licenses audited by `cargo-deny`.
