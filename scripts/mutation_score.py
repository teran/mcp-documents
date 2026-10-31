#!/usr/bin/env python3
"""Hard-gate score checker for the cargo-mutants run (`make mutation` / CI).

Reads `mutants.out/outcomes.json` produced by cargo-mutants and fails (non-zero
exit) unless the mutation score meets the minimum hard-gate threshold and the
run actually produced viable mutants and a report. Exits 0 with
"PASS: mutation gate satisfied" otherwise.

Usage:
    python3 scripts/mutation_score.py <min_score>
"""

import json
import os
import sys


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: mutation_score.py <min_score>", file=sys.stderr)
        return 2
    min_score = float(sys.argv[1])

    path = "mutants.out/outcomes.json"
    if not os.path.exists(path):
        print(f"ERROR: {path} missing; the mutation run produced no report")
        return 1

    with open(path) as fh:
        d = json.load(fh)

    total = int(d.get("total_mutants", 0))
    caught = int(d.get("caught", 0))
    missed = int(d.get("missed", 0))
    timeout = int(d.get("timeout", 0))
    unviable = int(d.get("unviable", 0))

    viable = total - unviable
    score = (100.0 * caught / viable) if viable > 0 else 0.0

    print(
        "mutation report: total=%d caught=%d missed=%d timeout=%d unviable=%d"
        % (total, caught, missed, timeout, unviable)
    )
    print(
        "mutation score = %.1f%% (hard-gate minimum %.0f%%)"
        % (score, min_score)
    )

    if total == 0:
        print("ERROR: no mutants were generated/tested; the gate would be a fiction")
        return 1
    if viable <= 0:
        print("ERROR: no viable mutants (all unviable); the gate would be a fiction")
        return 1
    if score < min_score:
        print(
            "FAIL: mutation score %.1f%% is below the %.0f%% hard gate; "
            "inspect mutants.out/missed.txt and close the gap (add tests, or a "
            "justified #[mutants::skip]/exclude for unreachable stubs)."
            % (score, min_score)
        )
        return 1

    print("PASS: mutation gate satisfied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
