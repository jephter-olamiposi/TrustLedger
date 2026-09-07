# TrustLedger

A distributed double-entry settlement ledger in Rust with on-chain (Solana)
finality. Public portfolio flagship — production-grade, senior-citable code.

## Architecture

| Layer | Crate | Status |
| --- | --- | --- |
| Ledger core | `ledger-core` | v0.1 shipped |
| Write-ahead log | `crates/wal` | planned |
| Raft consensus | `crates/raft` | planned |
| gRPC ingress | `crates/ingest` | planned |
| Merkle roots | `crates/merkle` | planned |
| Solana settlement | `crates/solana-settle` | planned |
| Observability | `crates/observability` | planned |
| Reference demo | `apps/demo` | planned |

## ledger-core

The correctness-critical heart: accounts, two-phase transfers, `u128`
scaled-decimal math, and balances-can't-leak invariants.

```rust
use ledger_core::account::{AccountFlags, AccountType};
use ledger_core::amount::{Amount, Scale};
use ledger_core::id::{AccountId, TransferId};
use ledger_core::transfer::Transfer;
use ledger_core::Ledger;

let mut ledger = Ledger::new(Scale::usdc());
let vault = AccountId::new(1);
let alice = AccountId::new(2);

ledger.create_account(vault, AccountType::Asset, AccountFlags::bank_asset(), Scale::usdc(), 0)?;
ledger.create_account(alice, AccountType::Liability, AccountFlags::customer(), Scale::usdc(), 0)?;

// Two-phase pending reservation followed by settlement capture
let hold = Transfer::new_pending(TransferId::new(1), vault, alice, Amount::new(1_000_000), 0)?;
ledger.create_pending(hold)?;
ledger.post_pending(TransferId::new(1), TransferId::new(2), Amount::new(1_000_000), 0)?;
```

This two-phase flow is compiled as a doctest in the crate docs
(`cargo test --doc`), so the documented API cannot drift from the code.

### Invariants

The ledger enforces, and a property suite proves:

- **Conservation** — total debits equal total credits for both posted and
  pending balances.
- **Structure** — every pending transfer is exactly backed by its debit and
  credit accounts' pending reservations, and a posted transfer may only link a
  hold that exists and is itself posted (checked eagerly at the mutation
  boundary, not just at `verify_invariants`).
- **Encapsulation** — `Scale`, `Amount`, `AccountId`, `TransferId` and every
  `Transfer` field are private; transfers are only created through validated
  constructors and the Pending→Posted/Voided lifecycle is a single
  state machine that rejects illegal moves. A deserialized transfer is
  re-validated (fresh ID, distinct accounts, non-zero amount, valid hold link)
  before it can be applied.
- **Determinism** — accounts and transfers live in `BTreeMap` (ADR-0002), so
  identical journal output always reproduces identical state;
  `Ledger::replay` equals the original ledger by value, byte-for-byte.
- **Fail-loud replay** — a journal whose voided amount contradicts the
  reservation it encodes, or whose timestamps run backwards, is rejected with
  a typed `LedgerError` instead of replaying to a wrong-but-plausible state.
- **Monotonic timestamps** — every journal event must carry a non-decreasing
  `timestamp`; older events are rejected (`TimestampBehindPrior`).
- **Zero overdraft and zero leakage** — `available_liability`/`available_asset`
  never report negative money (they read zero when empty, ADR-0004), and
  breaches are typed `InsufficientFunds` errors.
- **Account lifecycle** — `close_account` freezes an account (both legs)
  through the same journaled, replayable path as every other mutation.

Design decisions are recorded in the ADRs under [`docs/adr`](docs/adr/):

- [ADR-0001](docs/adr/ADR-0001-core-ledger.md) — core architecture and financial invariants
- [ADR-0002](docs/adr/ADR-0002-deterministic-indexes.md) — `BTreeMap` for deterministic iteration
- [ADR-0003](docs/adr/ADR-0003-journal-versioning.md) — journal versioning before the WAL
- [ADR-0004](docs/adr/ADR-0004-availability-lifecycle.md) — availability semantics and close lifecycle
- [ADR-0005](docs/adr/ADR-0005-transfer-state-typing.md) — runtime state gates (typestate deferred)
- [ADR-0006](docs/adr/ADR-0006-transition-ordering-timestamps.md) — compute-then-write ordering, monotonic timestamps

### Testing

- **Unit** (`tests/unit.rs`, 21 tests) — exact-error matrix for every rejection
  path: double-post, post-after-void, void-after-post, duplicate IDs,
  same-account, zero-amount, closed-account, partial-exceeds-pending,
  timestamp-in-past, corrupted journal.
- **Property** (`tests/invariants.rs`, 3 proptests, 200 cases) — a shadow
  model asserts the ledger equals an independent balance computation after
  *every* action; a corrupted-journal variant proves replay fails loudly.
- **Doctest** — the README flow above compiles and runs.

Requires the pinned Rust toolchain (`rust-toolchain.toml`); run
`./scripts/check` for the full gate (fmt + clippy `-D warnings` + tests +
benchmarks + docs). CI runs the same gate on every push.

### Benchmark

Hot-path throughput is measured by the zero-dependency harness at
`benches/throughput.rs` (see [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) for
methodology and variance notes). Representative numbers on this development
machine (Apple silicon, `profile.release` with LTO + overflow checks):

- `create_transfer`: ~4.9–5.3M transfers/sec (~188–205 ns/op)
- two-phase round-trip: ~3.9M pairs/sec (~257 ns/op)
- `apply_batch` (256/batch): ~0.8M transfers/sec (~1225 ns/op)
- `Ledger::replay`: ~5.6M events/sec (~177 ns/op)

All exceed the >1M transfers/sec claim in ADR-0001, confirming the
`BTreeMap` determinism cost is not observable on the hot path.