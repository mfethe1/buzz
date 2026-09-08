"""Signed plan scope and native relay transport for fixed fleet qualification."""

import hashlib
import json
import os
from pathlib import Path
import re
import time
import uuid

from process import run
from state import Journal, canonical, digest

EXTENSION = "org.buzz.fleet.v2"
HEX = re.compile(r"[0-9a-f]{64}\Z")
REQUEST_FIELDS = {"attempt_id", "channel", "task_id", "head", "snapshot", "relay", "policy_digest", "protocol"}


def require(value, error):
    if not value:
        raise ValueError(error)


def file_digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def load_policy(path):
    policy = json.loads(Path(path).read_text(encoding="utf-8"))
    require(policy.get("version") == 2, "unsupported_policy_version")
    require(policy.get("planner_pubkeys") and all(HEX.fullmatch(k) for k in policy["planner_pubkeys"]),
            "trusted_planners_required")
    require(policy.get("channels"), "allowed_channels_required")
    for channel in policy["channels"]:
        require(str(uuid.UUID(channel)) == channel, "invalid_channel")
    for key in ("buzz_binary", "state_dir", "receipts_path"):
        require(Path(policy[key]).is_absolute(), "absolute_policy_paths_required")
    require(Path(policy["buzz_binary"]).is_file(), "buzz_binary_missing")
    require(policy.get("buzz_binary_sha256") == file_digest(policy["buzz_binary"]), "buzz_binary_digest_mismatch")
    require(policy.get("relay", "").startswith(("http://", "https://", "ws://", "wss://")),
            "relay_required")
    # Credentials are resolved locally by buzz-cli's existing environment.
    require("@" not in policy["relay"] and "?" not in policy["relay"], "relay_credentials_forbidden")
    return policy


class Buzz:
    """Native signed transport and primary admission; reduction is for recovery."""

    def __init__(self, policy):
        self.policy = policy

    def command(self, args, data=None):
        raw = run([self.policy["buzz_binary"], "--relay", self.policy["relay"], "cml", "events", *args],
                  data=data, timeout=30)
        return json.loads(raw)

    def reduce(self, channel, task):
        require(channel in self.policy["channels"], "channel_not_allowed")
        require(str(uuid.UUID(task)) == task, "invalid_task_id")
        result = self.command(["reduce", "--channel", channel, "--task", task])
        require(result.get("verdict") == "ok", "cml_conflicted")
        require(HEX.fullmatch(result.get("head", "")), "invalid_cml_head")
        require(result["snapshot"].get("id") == task, "task_scope_mismatch")
        return result

    def task_command(self, args):
        return json.loads(run([self.policy["buzz_binary"], "--relay", self.policy["relay"],
                               "tasks", *args], timeout=30))

    def attempt(self, request):
        # Display projection identifies the attempt and reconciles receipts.
        # It is never sufficient authority to start a process.
        detail = self.task_command(["get", request["task_id"]])
        matches = [item for item in detail.get("attempts", [])
                   if item.get("plan_event_id") == request["head"]
                   and item.get("task_id") == request["task_id"]]
        require(len(matches) == 1, "relay_attempt_projection_missing")
        result = matches[0]
        require(result.get("worker") == request["snapshot"]["roles"]["worker"], "relay_worker_mismatch")
        require(result.get("machine_id") == request["snapshot"]["extensions"][EXTENSION]["machine_id"],
                "relay_machine_mismatch")
        if "attempt_id" in request:
            require(result["id"] == request["attempt_id"], "relay_attempt_mismatch")
        return result

    def admission(self, request, start):
        response = self.task_command(["admission", request["task_id"], "--attempt", request["attempt_id"],
                                      "--start-event", start])
        scope = request["snapshot"]["extensions"][EXTENSION]
        expected = {"attempt_id": request["attempt_id"], "task_id": request["task_id"],
                    "plan_event_id": request["head"], "start_event_id": start,
                    "worker": self.policy["worker_pubkey"], "machine_id": scope["machine_id"],
                    "policy_digest": request["policy_digest"], "expires_at": scope["expires_at"]}
        require(response == expected, "primary_admission_binding_mismatch")
        require(time.time() < scope["expires_at"], "grant_expired")
        return response

    def receipt(self, request, snapshot, published_at):
        return self.command(["receipt", "--channel", request["channel"], "--receipt-file", "-",
                             "--created-at", str(published_at)], canonical(snapshot))

    def publish(self, request, transition, previous, snapshot):
        return self.command(["publish", "--channel", request["channel"],
                             "--transition", transition, "--prev", previous,
                             "--task-file", "-"], canonical(snapshot))


def authority_digest(policy, target, host):
    """Bind the same public admission rules on scheduler and execution host."""
    return digest({"version": 2, "target": target, "relay": policy["relay"],
                   "channels": sorted(policy["channels"]), "planners": sorted(policy["planner_pubkeys"]),
                   "worker": host["worker_pubkey"], "machine_id": host["machine_id"], "capability": "qualify",
                   "repositories": {k: v["id"] for k, v in host["repositories"].items()}})


def grant(policy, request, *, local, check_deadline=True):
    """Fail closed on unsigned scope expansion; the snapshot is re-read natively."""
    require(set(request) == REQUEST_FIELDS, "unknown_request_fields")
    require(not (local and os.name == "nt"), "windows_containment_unqualified")
    require(request["protocol"] == "buzz-fleet-qualification-v2", "unsupported_request_protocol")
    require(request["relay"] == policy["relay"], "community_mismatch")
    require(request["channel"] in policy["channels"], "channel_not_allowed")
    task = request["snapshot"]
    require(task["id"] == request["task_id"], "task_scope_mismatch")
    require(task["roles"]["planner"] in policy["planner_pubkeys"], "planner_not_allowed")
    scope = task.get("extensions", {}).get(EXTENSION)
    require(isinstance(scope, dict) and set(scope) == {"target", "machine_id", "repository", "capability", "expires_at", "task_revision", "policy_digest"},
            "explicit_fleet_grant_required")
    require(scope["capability"] == "qualify", "capability_not_allowed")
    require(isinstance(scope["expires_at"], int) and not isinstance(scope["expires_at"], bool)
            and task["updated_at"] < scope["expires_at"] <= task["updated_at"] + 3600,
            "grant_deadline_unbounded")
    if check_deadline:
        require(time.time() < scope["expires_at"], "grant_expired")
    if local:
        require(scope["target"] == policy["alias"], "wrong_host")
        host = policy
    else:
        host = policy.get("hosts", {}).get(scope["target"])
        require(host, "host_not_allowed")
    require(task["roles"]["worker"] == host["worker_pubkey"], "worker_identity_mismatch")
    require(request["policy_digest"] == authority_digest(policy, scope["target"], host), "policy_digest_mismatch")
    require(scope["policy_digest"] == request["policy_digest"], "signed_policy_digest_mismatch")
    require(scope["machine_id"] == host["machine_id"], "machine_home_mismatch")
    require(isinstance(scope["task_revision"], int) and not isinstance(scope["task_revision"], bool)
            and scope["task_revision"] >= 0, "invalid_task_revision")
    repository = host.get("repositories", {}).get(scope["repository"])
    require(repository and repository["id"] == task["git"]["repo"], "repository_not_allowed")
    require(request["attempt_id"].startswith("buzz-qualify-")
            and HEX.fullmatch(request["attempt_id"][13:]), "attempt_binding_mismatch")
    return scope, repository


def admit(policy, channel, task_id):
    reduced = Buzz(policy).reduce(channel, task_id)
    require(reduced["snapshot"]["status"] == "planned", "task_not_planned")
    request = {"channel": channel, "task_id": task_id, "head": reduced["head"],
               "snapshot": reduced["snapshot"], "relay": policy["relay"],
               "protocol": "buzz-fleet-qualification-v2"}
    target = reduced["snapshot"].get("extensions", {}).get(EXTENSION, {}).get("target")
    require(target in policy.get("hosts", {}), "host_not_allowed")
    request["policy_digest"] = authority_digest(policy, target, policy["hosts"][target])
    projection = Buzz(policy).attempt(request)
    require(projection["state"] == "planned", "relay_attempt_not_planned")
    request["attempt_id"] = projection["id"]
    grant(policy, request, local=False)
    require(len(canonical(request).encode()) <= 32768, "request_too_large")
    return Journal(policy["state_dir"]).admit(request)


def publish_transition(buzz, journal, request, transition, previous, snapshot):
    """Freeze before network I/O; retrying identical CML preserves the event ID."""
    item = journal.freeze(request["attempt_id"], transition, previous, snapshot)
    frozen = json.loads(item["snapshot"])
    if item["event_id"]:
        return item["event_id"], frozen
    # The previous call may have committed remotely before its reply was lost.
    current = buzz.reduce(request["channel"], request["task_id"])
    if current["snapshot"] == frozen:
        event_id = current["head"]
    else:
        require(current["head"] == item["previous"], "cml_head_changed")
        response = buzz.publish(request, transition, item["previous"], frozen)
        require(response.get("accepted") is True and HEX.fullmatch(response.get("event_id", "")),
                "publish_not_accepted")
        event_id = response["event_id"]
    journal.sent(request["attempt_id"], transition, event_id)
    return event_id, frozen


