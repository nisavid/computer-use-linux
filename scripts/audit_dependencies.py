#!/usr/bin/env python3
"""Enforce cargo-audit with a temporary, exact maintenance-notice exception."""

from __future__ import annotations

import datetime as dt
import json
import pathlib
import subprocess
import sys


# Reviewed 2026-10-21. AT-SPI's published releases and upstream main still
# require zbus 5; zbus 6 consolidated these standalone crates. These advisories
# are informational maintenance notices, not vulnerability reports.
# Remove this exception during https://github.com/nisavid/computer-use-linux/issues/31.
EXPIRES = dt.date(2026, 11, 21)
EXCEPTIONS = {
    "RUSTSEC-2026-0370": ("zvariant", "5.11.0"),
    "RUSTSEC-2026-0371": ("zbus_names", "4.3.2"),
    "RUSTSEC-2026-0372": ("zvariant_derive", "5.11.0"),
}
# Preserve the existing, independently justified build-time quick-xml policy.
EXISTING_IGNORES = {"RUSTSEC-2026-0194", "RUSTSEC-2026-0195"}


def check_report(report: dict, today: dt.date) -> list[str]:
    """Reject vulnerabilities and every warning outside the reviewed tuples."""
    if today >= EXPIRES:
        raise ValueError(f"maintenance exception expired on {EXPIRES}; resolve issue #31")

    settings = report["settings"]
    if (
        set(settings["ignore"]) != EXISTING_IGNORES
        or settings["target_arch"]
        or settings["target_os"]
        or settings["severity"] is not None
        or not {"unmaintained", "unsound", "notice"}.issubset(
            settings["informational_warnings"]
        )
    ):
        raise ValueError("unexpected audit exclusions or warning configuration")

    vulnerabilities = report["vulnerabilities"]
    if (
        vulnerabilities["found"] is not False
        or type(vulnerabilities["count"]) is not int
        or vulnerabilities["count"] != 0
        or vulnerabilities["list"] != []
    ):
        raise ValueError(f"vulnerabilities found: {json.dumps(vulnerabilities)}")

    accepted = []
    warnings = report["warnings"]
    if not isinstance(warnings, dict):
        raise ValueError("invalid audit warnings")
    for kind, entries in warnings.items():
        if not isinstance(entries, list):
            raise ValueError("invalid audit warning entries")
        for warning in entries:
            advisory = warning["advisory"]
            package = warning["package"]
            advisory_id = advisory["id"]
            if (
                kind != "unmaintained"
                or warning["kind"] != kind
                or advisory["informational"] != kind
                or advisory["package"] != package["name"]
                or EXCEPTIONS.get(advisory_id) != (package["name"], package["version"])
            ):
                raise ValueError(f"unaccepted audit warning: {json.dumps(warning)}")
            accepted.append(advisory_id)

    if set(accepted) != set(EXCEPTIONS):
        raise ValueError("maintenance notices changed; review and remove stale exceptions")
    return accepted


def main() -> int:
    try:
        result = subprocess.run(
            ["cargo", "audit", "--json", "--deny", "warnings"],
            cwd=pathlib.Path(__file__).resolve().parent.parent,
            capture_output=True,
            text=True,
            check=False,
        )
        sys.stderr.write(result.stderr)
        if result.returncode not in (0, 1):
            raise ValueError(f"cargo-audit failed with exit {result.returncode}")
        report = json.loads(result.stdout)
        accepted = check_report(report, dt.datetime.now(dt.timezone.utc).date())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"dependency audit failed: {error}", file=sys.stderr)
        return 1

    for advisory_id in accepted:
        print(f"Temporary maintenance exception: {advisory_id} {EXCEPTIONS[advisory_id]} (expires {EXPIRES})")
    print("Dependency audit passed; migration tracked in issue #31.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
