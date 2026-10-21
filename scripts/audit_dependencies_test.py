"""Regression tests for audit acceptance and fail-closed failure handling."""

import contextlib
import copy
import datetime as dt
import io
import json
import subprocess
import unittest
from unittest.mock import patch

import audit_dependencies as audit


def report():
    return {
        "settings": {
            "ignore": sorted(audit.EXISTING_IGNORES),
            "target_arch": [],
            "target_os": [],
            "severity": None,
            "informational_warnings": ["unmaintained", "unsound", "notice"],
        },
        "vulnerabilities": {"found": False, "count": 0, "list": []},
        "warnings": {
            "unmaintained": [
                {
                    "kind": "unmaintained",
                    "package": {"name": package, "version": version},
                    "advisory": {
                        "id": advisory_id,
                        "package": package,
                        "informational": "unmaintained",
                    },
                }
                for advisory_id, (package, version) in audit.EXCEPTIONS.items()
            ]
        },
    }


class AuditPolicyTests(unittest.TestCase):
    def test_only_reviewed_notices_pass_before_expiry(self):
        accepted = audit.check_report(report(), audit.EXPIRES - dt.timedelta(days=1))
        self.assertEqual(set(accepted), set(audit.EXCEPTIONS))

    def test_expiry_is_enforced_on_and_after_deadline(self):
        for today in [audit.EXPIRES, audit.EXPIRES + dt.timedelta(days=1)]:
            with self.subTest(today=today), self.assertRaisesRegex(ValueError, "expired"):
                audit.check_report(report(), today)

    def test_vulnerabilities_and_inconsistent_reports_fail(self):
        for vulnerability in [
            {"found": True, "count": 1, "list": [{"advisory": {"id": "RUSTSEC-2026-0370"}}]},
            {"found": False, "count": 0, "list": [{}]},
            {"found": False, "count": False, "list": []},
        ]:
            candidate = report()
            candidate["vulnerabilities"] = vulnerability
            with self.subTest(vulnerability=vulnerability), self.assertRaises(ValueError):
                audit.check_report(candidate, dt.date(2026, 10, 21))

    def test_changed_id_package_version_or_classification_fails(self):
        changes = [
            ("advisory", "id", "RUSTSEC-2026-9999"),
            ("advisory", "package", "other"),
            ("advisory", "informational", "unsound"),
            ("package", "name", "other"),
            ("package", "version", "5.11.1"),
        ]
        for section, key, value in changes:
            candidate = report()
            candidate["warnings"]["unmaintained"][0][section][key] = value
            with self.subTest(change=(section, key)), self.assertRaises(ValueError):
                audit.check_report(candidate, dt.date(2026, 10, 21))

    def test_unknown_warning_and_stale_exception_fail(self):
        for kind in ["unsound", "notice", "yanked", "future-kind"]:
            candidate = report()
            candidate["warnings"][kind] = [copy.deepcopy(candidate["warnings"]["unmaintained"][0])]
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                audit.check_report(candidate, dt.date(2026, 10, 21))
        candidate = report()
        candidate["warnings"] = {}
        with self.assertRaisesRegex(ValueError, "stale"):
            audit.check_report(candidate, dt.date(2026, 10, 21))

    def test_new_ignores_or_disabled_warning_classes_fail(self):
        for key, value in [
            ("ignore", sorted(audit.EXISTING_IGNORES) + ["RUSTSEC-2026-0370"]),
            ("severity", "high"),
            ("target_os", ["linux"]),
            ("target_arch", ["x86_64"]),
            ("informational_warnings", ["unmaintained"]),
        ]:
            candidate = report()
            candidate["settings"][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                audit.check_report(candidate, dt.date(2026, 10, 21))

    def test_command_failures_and_malformed_output_fail_closed(self):
        results = [
            FileNotFoundError("cargo"),
            subprocess.CompletedProcess([], 2, json.dumps(report()), "tool failure"),
            subprocess.CompletedProcess([], -9, json.dumps(report()), "killed"),
            subprocess.CompletedProcess([], 1, "not JSON", "fetch failure"),
            subprocess.CompletedProcess([], 1, "{}", ""),
            subprocess.CompletedProcess([], 1, "null", ""),
        ]
        for result in results:
            kwargs = {"side_effect": result} if isinstance(result, Exception) else {"return_value": result}
            with (
                self.subTest(result=result),
                patch.object(audit.subprocess, "run", **kwargs),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(audit.main(), 1)


if __name__ == "__main__":
    unittest.main()
