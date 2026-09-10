# ADR-0010: Merkle Mountain Range (MMR) and Solana On-Chain Settlement Finality

- **Status:** Accepted
- **Date:** 2026-09-10
- **Author:** Jephter Olaifa

---

## Context

TrustLedger's accounting guarantees are enforced by its deterministic double-entry state machine
(`ledger-core`), durable write-ahead log (`wal`), and distributed consensus replication (`raft`).
However, in modern fintech and settlement systems (e.g. Visa/Mastercard settling card rails via USDC,
Stripe/Bridge cross-border payments), an operator-controlled database alone requires users to blindly
trust the company hosting the ledger.

In `plans/trustledger/01_PROJECT_PLAN.md` (Pillars D & E, Phase 5), we require:
- A cryptographic data structure over ledger transfer events that supports continuous append-only
  operations with logarithmic proofs;
- A verifiable anchor on a public high-throughput blockchain (Solana) that commits the batch state root;
- A Program Derived Address (PDA) storing a tamper-evident chain of settlement roots;
- An on-chain verification instruction and a standalone verifier CLI that allows any auditor, merchant,
  or client to independently verify that their transfer is included in an on-chain settlement batch without
  trusting the operator.

## Decision

We adopt a dual-state architecture: **Off-chain authoritative double-entry accounting with an incremental
Merkle Mountain Range (MMR)** paired with **On-chain settlement state commitment on Solana**.

### 1. Merkle Mountain Range (MMR) vs Naive Trees and Hash Chains

- **Why MMR?**
  - A traditional Merkle tree requires a fixed leaf count or expensive rebalancing/re-hashing of the entire
    tree upon every append ($O(N)$).
  - A linear hash chain ($H_n = \text{hash}(H_{n-1} || T_n)$) allows appends in $O(1)$, but proving inclusion
    of transfer $k$ requires replaying all $N - k$ intermediate events ($O(N)$ proof size).
  - A Merkle Mountain Range (Peter Todd / Grin / Zebra) represents the append-only event stream as a sequence
    of perfect binary trees ("mountains") whose heights correspond to the set bits in the binary representation
    of the leaf count.
  - Adding a leaf takes amortized $O(1)$ time ($O(\log N)$ worst-case merges).
  - Proving inclusion requires only $O(\log N)$ sibling hashes and the list of peaks (~640 bytes for $10^6$ entries).
- **Domain Separation (RFC 6962):**
  - To prevent second pre-image attacks where an internal node is passed as a leaf or a bagged peak is
    passed as an internal node, strict 1-byte domain separation prefixes are prepended:
    - Leaves: `Sha256(0x00 || leaf_data)`
    - Internal nodes: `Sha256(0x01 || left_child || right_child)`
    - Peak bagging: `Sha256(0x02 || left_peak || right_peak)`
- **Peak Bagging:**
  - Multiple mountain peaks are folded right-to-left into a single 32-byte MMR state root.

### 2. Solana On-Chain Settlement Program (`crates/solana-settle`)

- **PDA Root Commitment Account:**
  - Derived from canonical seeds `[b"settlement_root", authority.as_ref()]`.
  - Account state stores:
    `{ authority, epoch, batch_seq, merkle_root, previous_root, transfer_count, total_settled_amount, chain_tip, settled_at, bump }`.
  - Serialized size is exactly 177 bytes.
- **Tamper-Evident Hash Chain:**
  - Each settlement batch submission enforces `batch_seq == current.batch_seq + 1` and
    `previous_root == current.merkle_root`.
  - Forging or rewriting any past batch requires forking the Solana blockchain.
- **On-Chain Inclusion Verification:**
  - The program provides a `VerifyInclusion { leaf_hash, proof_bytes }` instruction.
  - Verifies the proof against the committed PDA root directly in Solana's BPF runtime using under ~4,000
    compute units (well below the 200,000 CU limit).

### 3. Standalone Verifier CLI & Client Receipts

- **`TransferReceipt`:**
  - A portable JSON receipt containing `{ transfer_id, amount, debit_account, credit_account, leaf_hash, batch_seq, merkle_root, proof }`.
- **`verifier` Binary (`src/bin/verifier.rs`):**
  - A zero-dependency CLI written with `clap` that can verify receipts locally or against on-chain roots:
    `verifier --receipt receipt.json`
  - Returns exit code 0 and `[VERIFIED]` on cryptographic match, or exit code 1 and `[FAILED]` on tamper.

## Consequences

- **Positive:**
  - **Zero-Trust Auditability:** Anyone can verify that a specific transfer was included in a settlement batch
    without having access to the private database or trusting the operator.
  - **Minimal On-Chain Footprint:** Only 177 bytes are stored in the Solana account, and a single transaction
    commits hundreds or thousands of off-chain transfers.
  - **Supply-Chain & License Safety:** Complies with `deny.toml` (MIT OR Apache-2.0).
  - **MSRV Compatibility:** Uses `solana-program v2.2.0` with `rust-version = 1.79` and Apache-2.0 licensing,
    satisfying workspace MSRV 1.80.

- **Trade-offs:**
  - MMR node arrays grow linearly with the number of leaves ($2N$ total nodes for $N$ leaves, ~64 MB for $10^6$ entries).
    For multi-gigabyte event logs, leaf positions and internal nodes can be persisted to an append-only index file.
  - On-chain settlement introduces Solana transaction confirmation latency (typically ~400ms on Solana).
