# ADR-0004: Availability Semantics and Account Lifecycle

- **Status:** Accepted
- **Date:** 2026-09-07
- **Author:** Jephter Olaifa

---

## Context

Two behavioral questions needed explicit decisions:

1. What should `available_liability` / `available_asset` read when debits
   exceed credits? Two designs were rejected: (a) returning `InsufficientFunds`
   conflated a *read* with a *write guard* and forced callers to decode an
   error for a routine balance view; (b) returning an underflow error made the
   arithmetic reach into signed territory, which the unsigned component model
   deliberately avoids.
2. `AccountFlags.is_closed` was honored by the write guards
   (`ensure_can_debit`/`ensure_can_credit`) but could never be set through the
   ledger: the flag was unrepresentable by design.

## Decision

- `available_liability` and `available_asset` return `Amount::ZERO` when the
  opposing leg exceeds the available leg, and only error (typed) if the
  summation itself would overflow `u128`. Availability is a view, not a guard.
- The ledger owns the close lifecycle: `Ledger::close_account(id, timestamp)`
  sets `is_closed`, appends a `LedgerEvent::AccountClosed` journal event, and
  is replayed by the same path. Closed accounts reject all new transfers on
  both legs. Closing an already-closed or unknown account is a typed error.

## Consequences

### Benefits
- Availability reads are total (cannot fail on ordinary state); money can never
  be reported as a negative available balance.
- The close lifecycle is auditable end-to-end and survives replay, so a
  closed account can never silently reopen on restart.

### Costs & Trade-Offs
- A read that hides an empty balance returns zero rather than distinguishing
  "empty" from "overdrawn"; reconciliation uses `net_settled` when a signed
  view is required.

## Alternatives Considered
- **Availability as a fallible read:** rejected (conflates view and guard).
- **Deleting `is_closed` until needed:** rejected after Phase 1 analysis;
  freeze/close is a core wallet flow and the flag already existed.

## References
- `Balance::available_liability`, `Balance::available_asset`,
  `Ledger::close_account` in `crates/ledger-core`.
- ADR-0001 decision 1 (two-entity schema) and decision 4 (journal-as-truth).