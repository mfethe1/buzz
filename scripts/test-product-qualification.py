#!/usr/bin/env python3
"""Exercise the production qualification CLI, including deceptive green wrappers."""

import copy
import importlib.util
import itertools
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("product-qualification.py")
spec = importlib.util.spec_from_file_location("qualification", SCRIPT)
qualification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualification)


def fixture():
    """Build a full-CI result set using the exported production lane inventory."""
    env = {
        "GITHUB_REPOSITORY": "mfethe1/buzz",
        "GITHUB_EVENT_NAME": "pull_request",
        "GITHUB_SHA": "c" * 40,
        "QUALIFICATION_EVALUATOR_SHA": "a" * 40,
        "GITHUB_RUN_ID": "100",
        "GITHUB_RUN_ATTEMPT": "2",
    }
    event = {
        "repository": {"full_name": "mfethe1/buzz"},
        "number": 18,
        "pull_request": {
            "base": {"sha": "a" * 40, "ref": "product/main", "repo": {"full_name": "mfethe1/buzz"}},
            "head": {"sha": "b" * 40},
        },
    }
    needs = {"changes": {"result": "success", "outputs": {
        key: "true" for key in ("rust", "desktop", "desktop-rust", "web", "mobile")
    }}}
    for evidence in qualification.qualify(needs, event, env)["evidence"]:
        job, _, output = evidence["lane"].partition(".")
        state = needs.setdefault(job, {"result": "success", "outputs": {}})
        if output:
            state["outputs"][output] = "success"
    return needs, event, env


class QualificationTest(unittest.TestCase):
    def run_cli(self, needs, event, env):
        with tempfile.TemporaryDirectory() as directory:
            event_file = Path(directory) / "event.json"
            output = Path(directory) / "receipt.json"
            event_file.write_text(json.dumps(event))
            process = subprocess.run(
                [sys.executable, str(SCRIPT), "--output", str(output)],
                env={**os.environ, **env, "GITHUB_EVENT_PATH": str(event_file),
                     "QUALIFICATION_NEEDS": json.dumps(needs)},
                capture_output=True, text=True, timeout=10,
            )
            return process, json.loads(output.read_text())

    def test_cli_binds_change_and_tested_merge_commit(self):
        process, receipt = self.run_cli(*fixture())
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertTrue(receipt["qualified"])
        self.assertEqual(receipt["base_sha"], "a" * 40)
        self.assertEqual(receipt["head_sha"], "b" * 40)
        self.assertEqual(receipt["tested_sha"], "c" * 40)
        self.assertEqual(receipt["run_attempt"], "2")

    def test_every_actual_lane_fails_closed_under_green_wrapper(self):
        baseline, event, env = fixture()
        evidence = qualification.qualify(baseline, event, env)["evidence"]
        # A lane deleted from the evaluator must also fail this regression.
        self.assertEqual({lane["lane"] for lane in evidence}, {
            "changes", "dead-token-guard", "rust.rust_lint_result",
            "rust.unit_tests_result", "rust.windows_rust_result",
            "rust-cross-compile-domain.server_cross_compile_result",
            "desktop-domain.desktop_result", "desktop-domain.desktop_windows_result",
            "desktop-macos-domain.desktop_macos_result",
            "relay-artifacts-domain.desktop_e2e_relay_result",
            "relay-domain.desktop_e2e_integration_result",
            "relay-domain.backend_integration_result", "relay-domain.relay_e2e_result",
            "postgres-domain.postgres_tests_result", "clients.web_result",
            "clients.mobile_result", "mobile-swift-domain.mobile_swift_result",
            "security-domain.security_result",
        })
        for lane in evidence:
            job, _, output = lane["lane"].partition(".")
            for bad in ("failure", "cancelled", "skipped", "", None):
                with self.subTest(lane=lane["lane"], result=bad):
                    needs = copy.deepcopy(baseline)
                    if output:
                        needs[job]["outputs"][output] = bad
                    else:
                        needs[job]["result"] = bad
                    self.assertFalse(qualification.qualify(needs, event, env)["qualified"])

    def test_cli_rejects_skipped_required_lane_and_keeps_failure_receipt(self):
        needs, event, env = fixture()
        needs["relay-domain"]["outputs"]["desktop_e2e_integration_result"] = "skipped"
        process, receipt = self.run_cli(needs, event, env)
        self.assertEqual(process.returncode, 1)
        self.assertFalse(receipt["qualified"])
        self.assertIn("desktop_e2e_integration_result", process.stderr)

    def test_documentation_only_change_allows_explicit_out_of_scope_skips(self):
        needs, event, env = fixture()
        for key in needs["changes"]["outputs"]:
            needs["changes"]["outputs"][key] = "false"
        for job, state in needs.items():
            if job not in ("changes", "dead-token-guard"):
                state.update(result="skipped", outputs={})
        process, receipt = self.run_cli(needs, event, env)
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertTrue(receipt["qualified"])
        needs["changes"]["outputs"].pop("rust")
        self.assertFalse(qualification.qualify(needs, event, env)["qualified"])

    def test_out_of_scope_failure_is_not_hidden(self):
        needs, event, env = fixture()
        needs["changes"]["outputs"]["web"] = "false"
        needs["clients"]["outputs"]["web_result"] = "failure"
        self.assertFalse(qualification.qualify(needs, event, env)["qualified"])

    def test_missing_wrapper_denies(self):
        needs, event, env = fixture()
        needs.pop("postgres-domain")
        self.assertFalse(qualification.qualify(needs, event, env)["qualified"])

    def test_bad_identity_and_unsupported_event_deny_at_cli(self):
        for field, value in (("GITHUB_REPOSITORY", "other/repo"),
                             ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                             ("GITHUB_SHA", "main"),
                             ("QUALIFICATION_EVALUATOR_SHA", "c" * 40)):
            with self.subTest(field=field):
                needs, event, env = fixture()
                env[field] = value
                process, receipt = self.run_cli(needs, event, env)
                self.assertEqual(process.returncode, 1)
                self.assertFalse(receipt["qualified"])

    def test_push_requires_all_lanes_even_when_paths_are_false(self):
        needs, _, env = fixture()
        env["GITHUB_EVENT_NAME"] = "push"
        env["GITHUB_SHA"] = "b" * 40
        event = {"repository": {"full_name": "mfethe1/buzz"},
                 "before": "a" * 40, "after": "b" * 40, "ref": "refs/heads/product/main"}
        needs["changes"]["outputs"] = {key: "false" for key in needs["changes"]["outputs"]}
        process, _ = self.run_cli(needs, event, env)
        self.assertEqual(process.returncode, 0, process.stderr)
        wrong_repository = {**env, "GITHUB_REPOSITORY": "other/repo"}
        process, receipt = self.run_cli(needs, event, wrong_repository)
        self.assertEqual(process.returncode, 1)
        self.assertFalse(receipt["qualified"])
        needs["mobile-swift-domain"]["outputs"]["mobile_swift_result"] = "skipped"
        self.assertFalse(qualification.qualify(needs, event, env)["qualified"])

    def test_all_path_combinations_require_their_native_surfaces(self):
        keys = ("rust", "desktop", "desktop-rust", "web", "mobile")
        for values in itertools.product((False, True), repeat=len(keys)):
            needs, event, env = fixture()
            flags = dict(zip(keys, values))
            needs["changes"]["outputs"] = {key: str(value).lower() for key, value in flags.items()}
            evidence = {item["lane"]: item["required"] for item in qualification.qualify(needs, event, env)["evidence"]}
            with self.subTest(paths=flags):
                self.assertEqual(evidence["clients.web_result"], flags["web"])
                self.assertEqual(evidence["mobile-swift-domain.mobile_swift_result"], flags["mobile"])
                self.assertEqual(evidence["postgres-domain.postgres_tests_result"], flags["rust"])
                self.assertEqual(evidence["desktop-domain.desktop_windows_result"], any(flags[key] for key in ("rust", "desktop", "desktop-rust")))


if __name__ == "__main__":
    unittest.main(verbosity=2)
