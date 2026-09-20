#!/usr/bin/env python3
"""Signed Buzz CML admission and fixed, read-only fleet qualification.

No prompt, command, environment, or filesystem path from a Buzz task is
executed. The only capability in this release is repository qualification.
"""

import argparse
from contextlib import contextmanager
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time
import uuid

from process import ProcessError, run
from state import Journal, canonical, digest, receipt_backend

EXTENSION = "org.buzz.fleet.v1"
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
    require(policy.get("version") == 1, "unsupported_policy_version")
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


@contextmanager
def worker_lock(journal):
    """A kernel-released exclusive lock; stale lock files do not permit overlap."""
    path = journal.directory / "worker.lock"
    with path.open("a+b") as file:
        os.chmod(path, 0o600)
        file.seek(0)
        file.write(b"0")
        file.flush()
        file.seek(0)
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(file.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            yield
        finally:
            if os.name == "nt":
                file.seek(0)
                msvcrt.locking(file.fileno(), msvcrt.LK_UNLCK, 1)


class Buzz:
    """Native Buzz signature, role, and chain validation is the admission authority."""

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

    def publish(self, request, transition, previous, snapshot):
        return self.command(["publish", "--channel", request["channel"],
                             "--transition", transition, "--prev", previous,
                             "--task-file", "-"], canonical(snapshot))


def authority_digest(policy, target, host):
    """Bind the same public admission rules on scheduler and execution host."""
    return digest({"version": 1, "target": target, "relay": policy["relay"],
                   "channels": sorted(policy["channels"]), "planners": sorted(policy["planner_pubkeys"]),
                   "worker": host["worker_pubkey"], "capability": "qualify",
                   "repositories": {k: v["id"] for k, v in host["repositories"].items()}})


def grant(policy, request, *, local, check_deadline=True):
    """Fail closed on unsigned scope expansion; the snapshot is re-read natively."""
    require(set(request) == REQUEST_FIELDS, "unknown_request_fields")
    require(not (local and os.name == "nt"), "windows_containment_unqualified")
    require(request["protocol"] == "buzz-fleet-qualification-v1", "unsupported_request_protocol")
    require(request["relay"] == policy["relay"], "community_mismatch")
    require(request["channel"] in policy["channels"], "channel_not_allowed")
    task = request["snapshot"]
    require(task["id"] == request["task_id"], "task_scope_mismatch")
    require(task["roles"]["planner"] in policy["planner_pubkeys"], "planner_not_allowed")
    scope = task.get("extensions", {}).get(EXTENSION)
    require(isinstance(scope, dict) and set(scope) == {"target", "repository", "capability", "expires_at"},
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
    repository = host.get("repositories", {}).get(scope["repository"])
    require(repository and repository["id"] == task["git"]["repo"], "repository_not_allowed")
    expected = "buzz-qualify-" + digest({k: request[k] for k in ("relay", "channel", "task_id", "head")})
    require(request["attempt_id"] == expected, "attempt_binding_mismatch")
    return scope, repository


def admit(policy, channel, task_id):
    reduced = Buzz(policy).reduce(channel, task_id)
    require(reduced["snapshot"]["status"] == "planned", "task_not_planned")
    request = {"channel": channel, "task_id": task_id, "head": reduced["head"],
               "snapshot": reduced["snapshot"], "relay": policy["relay"],
               "protocol": "buzz-fleet-qualification-v1"}
    target = reduced["snapshot"].get("extensions", {}).get(EXTENSION, {}).get("target")
    require(target in policy.get("hosts", {}), "host_not_allowed")
    request["policy_digest"] = authority_digest(policy, target, policy["hosts"][target])
    request["attempt_id"] = "buzz-qualify-" + digest({k: request[k] for k in ("relay", "channel", "task_id", "head")})
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


def qualify(policy, repository, *, cancelled):
    """Only this fixed read-only operation is available to remote signed tasks."""
    path = Path(repository["path"])
    require(path.is_absolute(), "repository_path_not_absolute")
    path = path.resolve(strict=True)
    require(path.is_dir() and path.parent != path, "repository_path_invalid")
    git = policy["git_binary"]
    require(Path(git).is_absolute() and Path(git).is_file(), "git_binary_missing")
    argv = [git, "--no-optional-locks", "-c", "core.fsmonitor=false",
            "-c", "core.hooksPath=" + os.devnull, "-c", "submodule.recurse=false"]
    safe_names = {"PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP", "TMPDIR",
                  "HOME", "USERPROFILE", "LANG", "LC_ALL", "USER", "LOGNAME"}
    environment = {key: value for key, value in os.environ.items() if key in safe_names}
    environment.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                       GIT_CONFIG_SYSTEM=os.devnull, GIT_TERMINAL_PROMPT="0", GIT_OPTIONAL_LOCKS="0")
    top = run(argv + ["rev-parse", "--show-toplevel"], cwd=str(path), cancelled=cancelled, env=environment).strip()
    require(Path(top).resolve() == path, "repository_must_be_worktree_root")
    head = run(argv + ["rev-parse", "--verify", "HEAD"], cwd=str(path), cancelled=cancelled, env=environment).strip()
    require(re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", head), "invalid_repository_head")
    # Do not refresh working-tree content: even `git status` can run untrusted
    # repository clean filters. This operation reads the index only.
    # A constant per-entry format avoids collecting filenames and keeps the
    # current Buzz checkout comfortably below the bounded-output budget.
    tracked = run(argv + ["ls-files", "--cached", "--format=x", "-z"],
                  cwd=str(path), cancelled=cancelled, env=environment)
    return {"repository": repository["id"], "head_sha": head,
            "tracked_files": tracked.count("\0"), "python": sys.version.split()[0],
            "capability": "qualify", "agent_inference_exercised": False}


def execute(policy, request):
    journal = Journal(policy["state_dir"])
    scope, repository = grant(policy, request, local=True, check_deadline=False)
    journal.admit(request)
    attempt = request["attempt_id"]
    store = receipt_backend(policy)
    with worker_lock(journal):
        existing = store.get(attempt)
        if existing["state"] != "not_recorded":
            require(existing["request_digest"] == store.digest(request), "receipt_payload_conflict")
        if existing["state"] == "finished":
            return finish_delivery(policy, journal, request, existing["result"])
        if existing["state"] != "not_recorded":
            return {"attempt_id": attempt, "state": "outcome_unknown", "automatic_replay": False}
        if journal.cancelled(attempt):
            journal.update(attempt, "cancelled_before_execution")
            return {"attempt_id": attempt, "state": "cancelled_before_execution"}
        require(time.time() < scope["expires_at"], "grant_expired")
        buzz = Buzz(policy)
        current = buzz.reduce(request["channel"], request["task_id"])
        # A prior publish may already be durable; the frozen outbox below
        # reconciles only our exact snapshots, never another writer's head.
        with journal.connect() as db:
            has_outbox = bool(db.execute("SELECT 1 FROM outbox WHERE attempt=?", (attempt,)).fetchone())
        if not has_outbox:
            require(current["head"] == request["head"] and current["snapshot"] == request["snapshot"],
                    "cml_head_changed")
            require(current["snapshot"]["status"] == "planned", "task_not_planned")
        now = int(time.time())
        claimed = copy.deepcopy(request["snapshot"])
        claimed.update(status="claimed", updated_at=max(now, claimed["updated_at"]))
        claimed["lease"] = {"id": attempt, "holder": policy["worker_pubkey"],
                            "issued_at": now, "expires_at": scope["expires_at"]}
        head, claimed = publish_transition(buzz, journal, request, "claim", request["head"], claimed)
        working = copy.deepcopy(claimed)
        working.update(status="working", updated_at=max(int(time.time()), working["updated_at"]))
        head, working = publish_transition(buzz, journal, request, "start", head, working)
        current = buzz.reduce(request["channel"], request["task_id"])
        require(current["head"] == head and current["snapshot"] == working, "cml_head_changed")
        require(time.time() < scope["expires_at"], "grant_expired")
        if journal.cancelled(attempt):
            journal.update(attempt, "cancelled_before_execution")
            return {"attempt_id": attempt, "state": "cancelled_before_execution"}
        receipt = store.claim(attempt, request)
        require(receipt["state"] == "new", "automatic_replay_refused")
        journal.update(attempt, "running")
        try:
            result = {"status": "success", "qualification": qualify(
                policy, repository, cancelled=lambda: journal.cancelled(attempt) or time.time() >= scope["expires_at"])}
        except (ValueError, ProcessError, OSError) as error:
            reason = "grant_expired" if str(error) == "cancelled" and time.time() >= scope["expires_at"] else str(error)
            result = {"status": "cancelled" if reason == "cancelled" else "error", "error": reason}
        result.update(attempt_id=attempt, task_id=request["task_id"], plan_event_id=request["head"],
                      host=policy["alias"], completed_at=int(time.time()),
                      policy_digest=request["policy_digest"], protocol=request["protocol"],
                      reducer_sha256=file_digest(policy["buzz_binary"]))
        store.finish(attempt, request, result)
        journal.update(attempt, "result_pending", result)
        return finish_delivery(policy, journal, request, result)


def finish_delivery(policy, journal, request, result):
    """A successful process submits evidence for review; it cannot approve itself."""
    attempt = request["attempt_id"]
    if journal.get(attempt)["state"] == "delivered":
        return {"attempt_id": attempt, "state": "delivered", "result": result}
    buzz = Buzz(policy)
    current = buzz.reduce(request["channel"], request["task_id"])
    if current["snapshot"]["status"] == "cancelled":
        journal.update(attempt, "cancel_acknowledged", result)
        return {"attempt_id": attempt, "state": "cancel_acknowledged", "result": result}
    with journal.connect() as db:
        start = db.execute("SELECT event_id,snapshot FROM outbox WHERE attempt=? AND transition='start'",
                           (attempt,)).fetchone()
    require(start and start["event_id"], "start_receipt_missing")
    snapshot = json.loads(start["snapshot"])
    snapshot["updated_at"] = max(result["completed_at"], snapshot["updated_at"])
    snapshot["evidence"].append({"kind": "fleet-qualification-receipt", "reference": digest(result)})
    if result["status"] == "success":
        snapshot["status"] = "review"
        snapshot["git"]["head_sha"] = result["qualification"]["head_sha"]
        transition = "submit"
    else:
        snapshot["status"] = "blocked"
        snapshot["blockers"].append({"id": "fleet-qualification", "text": result.get("error", "cancelled"),
                                     "reference": digest(result)})
        transition = "block"
    event_id, _ = publish_transition(buzz, journal, request, transition, start["event_id"], snapshot)
    journal.update(attempt, "delivered", result)
    return {"attempt_id": attempt, "state": "delivered", "event_id": event_id, "result": result}


def dispatch(policy, attempt, *, recover=False):
    journal = Journal(policy["state_dir"])
    with worker_lock(journal):
        item = journal.get(attempt)
        require(item["state"] != "not_recorded", "attempt_not_recorded")
        request = item["request"]
        scope, _ = grant(policy, request, local=False, check_deadline=not recover)
        if item["state"] == "delivered":
            return item
        require(recover or item["state"] == "queued", "retrieve_outcome_before_retry")
        host = policy["hosts"][scope["target"]]
        # This entire command is operator-installed policy, never task content.
        argv = host["command"]
        require(isinstance(argv, list) and argv and all(isinstance(x, str) for x in argv), "host_command_missing")
        journal.update(attempt, "delivery_started")
        try:
            response = json.loads(run(argv, data=canonical({"action": "execute", "request": request}), timeout=180))
        except (ProcessError, ValueError, OSError):
            journal.update(attempt, "delivery_unknown")
            return {"attempt_id": attempt, "state": "delivery_unknown", "automatic_replay": False}
        require(response.get("attempt_id") == attempt, "response_attempt_mismatch")
        journal.update(attempt, response["state"], response.get("result"))
        return response


def cancel_local(policy, request):
    """Only an already signed planner cancellation can request local termination."""
    grant(policy, request, local=True, check_deadline=False)
    journal = Journal(policy["state_dir"])
    journal.admit(request)
    current = Buzz(policy).reduce(request["channel"], request["task_id"])
    require(current["snapshot"]["status"] == "cancelled", "signed_planner_cancellation_required")
    for field in ("roles", "extensions", "id"):
        require(current["snapshot"][field] == request["snapshot"][field], "cancellation_contract_changed")
    journal.cancel(request["attempt_id"])
    receipt = receipt_backend(policy).get(request["attempt_id"])
    state = "cancel_acknowledged" if receipt["state"] == "finished" else "cancel_requested"
    if journal.get(request["attempt_id"])["state"] == "queued" and receipt["state"] == "not_recorded":
        # The executor checks this flag immediately before committing its start.
        # A lock is required to prove that it is not between that check and spawn.
        try:
            with worker_lock(journal):
                journal.update(request["attempt_id"], "cancelled_before_execution")
                state = "cancelled_before_execution"
        except (BlockingIOError, OSError):
            pass
    return {"attempt_id": request["attempt_id"], "state": state,
            "note": "cancel_requested is intent; only a terminal receipt acknowledges stopping."}


def cancel_remote(policy, attempt):
    journal = Journal(policy["state_dir"])
    item = journal.get(attempt)
    require(item["state"] != "not_recorded", "attempt_not_recorded")
    request = item["request"]
    scope, _ = grant(policy, request, local=False, check_deadline=False)
    # Host performs fresh signature/chain validation before touching its flag.
    response = json.loads(run(policy["hosts"][scope["target"]]["command"],
                              data=canonical({"action": "cancel", "request": request}), timeout=45))
    require(response.get("attempt_id") == attempt, "response_attempt_mismatch")
    if response["state"] in ("cancel_acknowledged", "cancelled_before_execution"):
        journal.update(attempt, response["state"], item.get("result"))
    return response


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True)
    commands = parser.add_subparsers(dest="action", required=True)
    add = commands.add_parser("admit")
    add.add_argument("--channel", required=True)
    add.add_argument("--task", required=True)
    for name in ("dispatch", "recover", "result", "cancel"):
        command = commands.add_parser(name)
        command.add_argument("--attempt", required=True)
    commands.add_parser("execute")
    args = parser.parse_args(argv)
    policy = load_policy(args.policy)
    if args.action == "admit":
        result = admit(policy, args.channel, args.task)
    elif args.action == "execute":
        raw = sys.stdin.buffer.read(33025)
        require(len(raw) <= 33024, "request_too_large")
        message = json.loads(raw)
        require(set(message) == {"action", "request"}, "invalid_transport_message")
        require(message["action"] in ("execute", "cancel"), "unknown_transport_action")
        result = (execute if message["action"] == "execute" else cancel_local)(policy, message["request"])
    elif args.action in ("dispatch", "recover"):
        result = dispatch(policy, args.attempt, recover=args.action == "recover")
    elif args.action == "result":
        result = receipt_backend(policy).get(args.attempt)
        if result["state"] == "not_recorded":
            result = Journal(policy["state_dir"]).get(args.attempt)
    else:
        result = cancel_remote(policy, args.attempt)
    print(canonical(result))
    # A host transport reply can successfully report a failed/unknown attempt.
    # The scheduler must receive that state instead of mistaking it for a lost RPC.
    if args.action == "execute":
        return 0
    return 0 if result.get("state") in ("queued", "delivered", "finished", "cancelled_before_execution", "cancel_acknowledged") else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, KeyError, OSError, ProcessError) as error:
        print(canonical({"state": "rejected", "error": str(error)}))
        raise SystemExit(2)
