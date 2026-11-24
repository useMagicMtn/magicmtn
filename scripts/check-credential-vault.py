#!/usr/bin/env python3
"""Check the CredentialVault TLA+ model and, optionally, deliberate faults."""

import argparse
from pathlib import Path
import subprocess
import sys

from formal_model import add_tool_arguments, check, verified_tools


MODEL = Path(__file__).resolve().parents[1] / "formal" / "credential-vault"
MODULE = "CredentialVault"

MUTATIONS = [
    *(
        (
            f"omit {field} from scope matching",
            f"/\\ s.{field} = row.scope.{field}",
            "/\\ TRUE",
            "ReadsRequireLiveExactScope",
        )
        for field in ("org", "owner", "connection", "purpose")
    ),
    (
        "allow writes across connections",
        "/\\ s.connection = row.scope.connection",
        "/\\ TRUE",
        "WritesRequireLiveExactScope",
    ),
    (
        "read destroyed rows",
        '    /\\ LiveMatch(s)\n    /\\ event\' = [kind |-> "read"',
        '    /\\ row.status # "absent" /\\ Matches(s)\n    /\\ event\' = [kind |-> "read"',
        "ReadsRequireLiveExactScope",
    ),
    (
        "accept a stale write after preflight",
        "LiveMatch(request.scope) /\\ row.version = request.expected",
        "LiveMatch(request.scope)",
        "WritesRequireCurrentVersion",
    ),
    (
        "accept a pending write after destroy",
        "LiveMatch(request.scope) /\\ row.version = request.expected",
        '(LiveMatch(request.scope) /\\ row.version = request.expected) \\/ row.status = "destroyed"',
        "WritesRequireLiveExactScope",
    ),
    (
        "reuse a destroyed ID",
        '    /\\ row.status = "absent"',
        '    /\\ row.status # "live"',
        "DestroyedIdsAreNeverReused",
    ),
    (
        "retain the encrypted envelope on destroy",
        "!.hasEnvelope = ~destroys",
        "!.hasEnvelope = TRUE",
        "OnlyLiveRowsHaveAnEnvelope",
    ),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_tool_arguments(parser)
    args, java, jar = verified_tools(parser)
    source = (MODEL / f"{MODULE}.tla").read_text()
    config = (MODEL / f"{MODULE}.cfg").read_text()
    check(java, jar, MODULE, source, config, args.timeout)

    if args.mutation_checks:
        for name, original, replacement, invariant in MUTATIONS:
            if source.count(original) != 1:
                raise RuntimeError(f"Mutation needs updating: {name}")
            mutated = source.replace(original, replacement)
            mutant_config = config.split("INVARIANTS\n", 1)[0] + \
                f"INVARIANTS TypeOK {invariant}\nCHECK_DEADLOCK FALSE\n"
            print(f"Mutation: {name}", flush=True)
            check(java, jar, MODULE, mutated, mutant_config, args.timeout, invariant)
    print("Credential vault checks passed.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
