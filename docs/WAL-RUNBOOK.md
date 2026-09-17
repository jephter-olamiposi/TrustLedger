# WAL runbook

The WAL is the durable record stream for the money path. It is not a convenience layer; it is the crash-recovery boundary for committed ledger history.

## What to expect on restart

When the system starts up, the WAL may show one of three states:

1. clean startup
2. torn tail after an unclean shutdown
3. sequence mismatch or file mutation that requires operator attention

The first case is normal. The second is usually recoverable without lost history. The third is a serious integrity event.

## Torn-write recovery

If a crash interrupts a write, the WAL keeps the verified prefix and truncates only the corrupted trailing tail. This is the expected recovery model for durable financial logs.

The key invariant is: recovery must never silently invent state beyond the last verified frame.

## Sequence mismatch

If the WAL reports a sequence mismatch, do not truncate blindly. That usually indicates a more serious file integrity issue or external mutation of the log. Restore from a known-good snapshot and verified replay path instead of guessing.

## Operational guidance

- preserve the log artifact before any repair attempt
- replay from a clean backup when the prefix is uncertain
- validate ledger invariants after recovery
- confirm the rebuilt journal matches the expected sequence range

## Why this matters

In a ledger system, the storage layer is part of the correctness story. A WAL that cannot recover cleanly is not just an operational nuisance; it is a financial integrity risk.