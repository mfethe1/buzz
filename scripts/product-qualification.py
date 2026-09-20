#!/usr/bin/env python3
"""Fail closed on selected CI jobs and record the exact tested change.

The receipt is CI evidence, not independent review or deployment authorization.
It aggregates CI results; protected review of the workflow definition remains
required. Its base-pinned evaluator cannot itself attest hostile PR workflow code.
A copied JSON receipt has no authority.
"""

import argparse
import json
import os
from pathlib import Path
import re
import sys


def qualify(needs, event, env):
    """Return a qualification receipt for GitHub's job results and event."""
    failures = []
    repository = env["GITHUB_REPOSITORY"]
    if event["repository"]["full_name"] != repository:
        raise ValueError("event repository differs from workflow repository")
    event_name = env["GITHUB_EVENT_NAME"]
    if event_name == "pull_request":
        pr = event["pull_request"]
        if pr["base"]["repo"]["full_name"] != repository:
            raise ValueError("pull request targets a different repository")
        identity = {
            "repository": repository,
            "pull_request": event["number"],
            "base_sha": pr["base"]["sha"],
            "head_sha": pr["head"]["sha"],
            "base_ref": pr["base"]["ref"],
        }
    elif event_name == "push":
        identity = {
            "repository": repository,
            "pull_request": None,
            "base_sha": event["before"],
            "head_sha": event["after"],
            "base_ref": event["ref"].removeprefix("refs/heads/"),
        }
        if event["after"] != env["GITHUB_SHA"]:
            raise ValueError("push event does not match tested commit")
    else:
        raise ValueError(f"unsupported event: {event_name}")
    identity["tested_sha"] = env["GITHUB_SHA"]
    identity["evaluator_sha"] = env["QUALIFICATION_EVALUATOR_SHA"]
    if identity["evaluator_sha"] != identity["base_sha"]:
        raise ValueError("qualification evaluator must come from the frozen base")
    for key in ("base_sha", "head_sha", "tested_sha"):
        if not re.fullmatch(r"[0-9a-f]{40}", identity[key]) or identity[key] == "0" * 40:
            raise ValueError(f"missing or invalid {key}")

    changes = needs.get("changes", {})
    flags = changes.get("outputs", {})
    selected = {}
    for key in ("rust", "desktop", "desktop-rust", "web", "mobile"):
        value = flags.get(key)
        if value not in ("true", "false"):
            failures.append(f"changes.{key}: missing boolean path result")
        selected[key] = value == "true" or event_name == "push"
    rust = selected["rust"]
    desktop_rust = selected["desktop-rust"]
    desktop = rust or desktop_rust or selected["desktop"]

    # Read actual job results exported by reusable workflows. Their Results job
    # only transports outputs and can succeed after a test job failed.
    lanes = [
        ("changes", None, True),
        ("dead-token-guard", None, True),
        ("rust", "rust_lint_result", rust or desktop_rust),
        ("rust", "unit_tests_result", rust),
        ("rust", "windows_rust_result", rust or desktop_rust),
        ("rust-cross-compile-domain", "server_cross_compile_result", rust),
        ("desktop-domain", "desktop_result", desktop),
        ("desktop-domain", "desktop_windows_result", desktop),
        ("desktop-macos-domain", "desktop_macos_result", desktop),
        ("relay-artifacts-domain", "desktop_e2e_relay_result", desktop),
        ("relay-domain", "desktop_e2e_integration_result", desktop),
        ("relay-domain", "backend_integration_result", rust),
        ("relay-domain", "relay_e2e_result", rust),
        ("postgres-domain", "postgres_tests_result", rust),
        ("clients", "web_result", selected["web"]),
        ("clients", "mobile_result", selected["mobile"]),
        ("mobile-swift-domain", "mobile_swift_result", selected["mobile"]),
        ("security-domain", "security_result", rust),
    ]
    evidence = []
    for job, output, required in lanes:
        state = needs.get(job, {})
        result = state.get("outputs", {}).get(output) if output else state.get("result")
        name = f"{job}.{output}" if output else job
        evidence.append({"lane": name, "required": required, "result": result})
        if required and (state.get("result") != "success" or result != "success"):
            failures.append(f"{name}: required success; got {result!r}, wrapper {state.get('result')!r}")
        elif result not in (None, "", "skipped", "success"):
            failures.append(f"{name}: unexpected {result!r}")
    # Never lose a failed/cancelled wrapper merely because the path selector says
    # that domain should not have run.
    for job, state in needs.items():
        if state.get("result") not in ("success", "skipped"):
            failures.append(f"{job}: wrapper {state.get('result')!r}")
    return {
        "schema_version": 1,
        "kind": "ci_qualification",
        **identity,
        "run_id": env["GITHUB_RUN_ID"],
        "run_attempt": env["GITHUB_RUN_ATTEMPT"],
        "qualified": not failures,
        "evidence": evidence,
        "failures": failures,
    }


def main():
    """Consume GitHub's runtime environment and write evidence even on denial."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        needs = json.loads(os.environ["QUALIFICATION_NEEDS"])
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        receipt = qualify(needs, event, os.environ)
    except (KeyError, TypeError, ValueError, OSError) as error:
        receipt = {"qualified": False, "failures": [f"invalid qualification input: {error}"]}
    args.output.write_text(json.dumps(receipt, indent=2) + "\n")
    for failure in receipt["failures"]:
        print(f"FAIL: {failure}", file=sys.stderr)
    if receipt["qualified"]:
        print(f"PASS: CI qualified {receipt['repository']} {receipt['head_sha']} tested as {receipt['tested_sha']}")
    return 0 if receipt["qualified"] else 1


if __name__ == "__main__":
    sys.exit(main())
