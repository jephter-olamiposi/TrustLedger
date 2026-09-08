#!/usr/bin/env python3
"""Benchmark regression gate for the ledger-core hot paths (ADR-0008).

Runs the criterion benchmark with `--save-baseline gate`, reads each measured
median from `target/criterion/<bench>/gate/estimates.json`, and fails if any
median exceeds its committed bound. Bounds live in
`docs/benchmarks/baseline.json` and are `recorded * tolerance_factor`: a loud
cross-machine guard, not a precise statistical comparator (CI runners differ
from developer machines, so the factor absorbs platform noise while still
catching 2x+ regressions).

Re-record the baseline after intentional performance work:
    cargo bench -p ledger-core --bench throughput -- --save-baseline gate
then update `recorded` in baseline.json with the new
`target/criterion/*/gate/estimates.json` medians and note date + machine.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = ROOT / "docs" / "benchmarks" / "baseline.json"
CRITERION = ROOT / "target" / "criterion"


def measured_medians() -> dict[str, float]:
    """Median ns/iteration per bench from the most recent gated run."""
    medians: dict[str, float] = {}
    for estimates in CRITERION.glob("*/gate/estimates.json"):
        bench_id = estimates.parent.parent.name
        data = json.loads(estimates.read_text())
        medians[bench_id] = float(data["median"]["point_estimate"])
    return medians


def main() -> int:
    if not BASELINE.exists():
        print(f"missing {BASELINE}: cannot gate", file=sys.stderr)
        return 1
    baseline = json.loads(BASELINE.read_text())
    factor = float(baseline["tolerance_factor"])
    bounds = dict(baseline["recorded"])

    subprocess.run(
        [
            "cargo",
            "bench",
            "--package",
            "ledger-core",
            "--bench",
            "throughput",
            "--",
            "--save-baseline",
            "gate",
        ],
        cwd=ROOT,
        check=True,
    )

    medians = measured_medians()
    missing = sorted(set(bounds) - set(medians))
    if missing:
        print(f"no gate result for: {missing} (bench did not run)", file=sys.stderr)
        return 1

    failures: list[str] = []
    print("\nbenchmark regression gate (criterion medians, ns/iteration):")
    for bench_id, recorded in sorted(bounds.items()):
        median = medians[bench_id]
        limit = recorded * factor
        status = "ok"
        if median > limit:
            status = "FAIL"
            failures.append(bench_id)
        print(f"  {bench_id:<24} {median:>12.1f}  bound {limit:>12.1f}  {status}")
    if failures:
        print(f"performance regression: {', '.join(failures)}", file=sys.stderr)
        return 1

    print("gate passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())