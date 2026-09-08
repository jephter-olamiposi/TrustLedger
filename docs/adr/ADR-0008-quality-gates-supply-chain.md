# ADR-0008: Quality Gates, Supply-Chain Governance, and Benchmark Regression

- **Status:** Accepted
- **Date:** 2026-09-08
- **Author:** Jephter Olaifa

---

## Context

Phase 1 of `plans/trustledger/01_PROJECT_PLAN.md` promised gates that were
**not actually enforced**:

- benchmarks were one-shot, load-sensitive runs with no committed baseline and
  no CI failure on regression ("throughput numbers tracked with statistical
  medians and noise rejection" — promised, absent);
- dependencies were only vetted by the `rust-crate-research` discipline, with
  no automated advisory scan, license gate, or dependency-update automation;
- `rust-version = "1.80"` was declared in `Cargo.toml` but CI only built
  `stable`, so the declared MSRV could silently drift;
- the "README flow compiles" claim in the readme was false (the only real
  doctest lived in `lib.rs`).

## Decision

Four complementary gates, all enforced in
`.github/workflows/ci.yml` and locally via `scripts/check`:

1. **Benchmark regression gate.** The ledger-core hot-path benchmark becomes a
   **criterion** benchmark (`crates/ledger-core/benches/throughput.rs`,
   `criterion ^= 0.7.0`), which reports statistical medians over 100 samples.
   `scripts/bench_gate.py` runs it with `--save-baseline gate` and fails when
   a median exceeds its bound in `docs/benchmarks/baseline.json`, where each
   bound is `recorded median * tolerance_factor`.
   - `tolerance_factor = 2.0` is a deliberate, documented trade: CI runs on
     `ubuntu-latest` while baselines are recorded on developer machines, so the
     gate is a **loud early-warning net for ≥2x regressions**, not a precise
     statistical comparator. Precision is a human job: after intentional
     performance work, re-record the baseline (protocol in the script header).
   - Criterion is pinned to `=0.7.0` because 0.8+ raises the MSRV to 1.86 and
     would break the MSRV gate below. The wal benchmark stays a zero-dependency
     harness: it measures the device fsync rate, which is not meaningful as a
     criterion iteration.
2. **Supply-chain gate.** `deny.toml` + `EmbarkStudios/cargo-deny-action@v2`,
   checked on every PR:
   - `advisories` (RustSec; deny-by-default in cargo-deny v2) — reported, with
     `continue-on-error` so a fresh upstream advisory cannot block unrelated
     work;
   - `bans licenses sources` — hard gate: permissive-only license policy and no
     unknown registries/git sources.
   - `.github/dependabot.yml` updates `Cargo.lock` weekly and the GitHub
     Actions monthly, so the gate always has recent supply-chain data.
   - The gate paid for itself on day one: the pre-existing default
     `heapless-cas` feature of postcard pulled the **unmaintained**
     `atomic-polyfill` (RUSTSEC-2023-0089) into the money path. Fixed by
     `postcard` with `default-features = false` and only `use-std` — the codec
     serializes `Vec<LedgerEvent>` and never used the embedded heap types.
3. **MSRV gate.** A dedicated CI job pins `dtolnay/rust-toolchain@1.80.0` and
   runs `cargo check --workspace --all-targets --locked`, proving the declared
   `rust-version` (and every dev-dependency, including criterion 0.7) compiles
   on 1.80. `Cargo.lock` is committed, so the check is pin-exact. Making the
   gate pass required pinning the lockfile to 1.80-compatible versions:
   `criterion =0.7.0` (0.8 raises MSRV to 1.86), `proptest` 1.8.0 (1.9+ raises
   MSRV to 1.82), `half` 2.4.1 (2.5+ needs 1.81), `clap` 4.5.x
   (`clap_lex` 1.x is edition 2024), and `tempfile` 3.26 (3.27 pulls
   getrandom 0.4, which is edition 2024).
4. **README as compiled doctest.** `lib.rs` embeds the repo root README with
   `#![doc = include_str!("../../README.md")]`, making the readme's `ledger-core`
   example a real rustdoc doctest (`cargo test --doc`) with a single source of
   truth. The readme's wal block is a `text` fence (it cannot run as a doctest
   without writing files), and `missing_docs` still applies to every public item.

## Consequences

- **Good**: the three promised Phase-1 gates are now real; supply chain,
  MSRV, and hot-path regressions are caught by CI instead of review; the README
  can no longer drift from the API it documents.
- **Cost**: the quality job builds and runs criterion on every PR (~1 min
  extra); the 2x tolerance means genuine 1.3x regressions are not auto-caught
  — the benchmark table in `docs/BENCHMARKS.md` remains the human-readable,
  drift-detectable record and must be updated when numbers change.
- **Not decided here**: coverage and mutation-gate thresholds, release/semver
  automation, and MSRV *policy* for future minor releases (the current pins are
  recorded in the lockfile + `Cargo.toml` comments so dependabot's weekly
  updates do not silently re-break the 1.80 gate).