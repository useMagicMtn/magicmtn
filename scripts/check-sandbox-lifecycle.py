#!/usr/bin/env python3
"""Check the sandbox lifecycle TLA+ model and, optionally, deliberate faults."""

import argparse
from pathlib import Path
import subprocess
import sys

from formal_model import add_tool_arguments, check, verified_tools


ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / "formal" / "sandbox-lifecycle"
MODULE = "SandboxLifecycle"

# Each fault is applied to a temporary copy. Check its specific contract so
# another invariant cannot accidentally make a broken mutation test pass.
MUTATIONS = [
    (
        "accept stale tokens",
        "/\\ db.token = workers[w].token",
        "/\\ TRUE \\* fault: accept any token",
        "OnlyCurrentLeaseCommits",
    ),
    (
        "accept expired leases",
        "/\\ db.live\n",
        "/\\ TRUE \\* fault: ignore lease expiry\n",
        "OnlyCurrentLeaseCommits",
    ),
    (
        "reset an active termination lease on repeated cancellation",
        'IF db.operation = "inspect" \\/ db.complete THEN',
        "IF TRUE THEN",
        "RepeatedCancellationPreservesLease",
    ),
    (
        "treat provider errors as absence",
        '!.sandbox = IF apply THEN IF terminal THEN "ended" ELSE result',
        '!.sandbox = IF accepted /\\ result = "error" THEN "ended"\n'
        '                         ELSE IF apply THEN IF terminal THEN "ended" ELSE result',
        "ErrorsPreserveObservation",
    ),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_tool_arguments(parser)
    args, java, jar = verified_tools(parser)
    source = (MODEL / "SandboxLifecycle.tla").read_text()
    config = (MODEL / "SandboxLifecycle.cfg").read_text()
    print("Checking sandbox lifecycle safety (two workers, at most three claims)...", flush=True)
    check(java, jar, MODULE, source, config, args.timeout)
    if args.mutation_checks:
        for name, original, replacement, invariant in MUTATIONS:
            valid = source.split("Valid(w) ==\n", 1)[1].split("\n\n", 1)[0]
            count = (valid if invariant == "OnlyCurrentLeaseCommits" else source).count(original)
            expected_count = 2 if invariant == "RepeatedCancellationPreservesLease" else 1
            if count != expected_count:
                raise RuntimeError(f"Mutation needs updating: {name} (found {count} anchors)")
            # Token/expiry mutations weaken acceptance at Finish only. BeginCall
            # still validates the real lease, and the invariant uses real Valid.
            if invariant == "OnlyCurrentLeaseCommits":
                weakened = valid.replace(original, replacement)
                mutated = source.replace("accepted == Valid(w)", "accepted == Accepts(w)")
                mutated = mutated.replace("(* One committed transaction:",
                                          f"Accepts(w) ==\n{weakened}\n\n(* One committed transaction:")
            else:
                mutated = source.replace(original, replacement)
            mutant_config = config.split("INVARIANTS\n", 1)[0] + \
                f"INVARIANTS TypeOK {invariant}\nCHECK_DEADLOCK FALSE\n"
            print(f"Mutation: {name}", flush=True)
            check(java, jar, MODULE, mutated, mutant_config, args.timeout, invariant)
    print("Sandbox lifecycle checks passed.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
