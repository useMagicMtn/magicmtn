#!/usr/bin/env python3
"""Model-check preparation, account isolation, and stale publication."""

import argparse
from pathlib import Path
import subprocess
import sys

from formal_model import add_tool_arguments, check, verified_tools


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_tool_arguments(parser)
    args, java, jar = verified_tools(parser)
    module = "ProviderCredentials"
    model = Path(__file__).resolve().parents[1] / "formal" / "provider-credentials"
    source = (model / f"{module}.tla").read_text()
    config = (model / f"{module}.cfg").read_text()
    check(java, jar, module, source, config, args.timeout)
    if args.mutation_checks:
        anchor = "candidate.account = Owner(c)"
        if source.count(anchor) != 1:
            raise RuntimeError("provider account mutation anchor changed")
        mutated = source.replace(anchor, "TRUE")
        check(java, jar, module, mutated, config, args.timeout, "LiveCredentialsMatchAccounts")
        anchor = "candidate.ticket = slots[c].ticket"
        if source.count(anchor) != 1:
            raise RuntimeError("publication ticket mutation anchor changed")
        mutated = source.replace(anchor, "TRUE")
        action_config = config.replace(
            "INVARIANTS LiveCredentialsMatchAccounts RevokedCredentialsAreUnavailable\n", ""
        )
        check(java, jar, module, mutated, action_config, args.timeout,
              expected_action="NoLatePublication")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
