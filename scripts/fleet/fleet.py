#!/usr/bin/env python3
"""Signed, atomic Buzz admission for a fixed read-only repository qualification.

No task prompt, command, environment or filesystem path is executed.
"""

import argparse
import json
import os
import sys

from admission import (Buzz, EXTENSION, admit, authority_digest, file_digest, grant, load_policy, require)
from execution import cancel_local, execute, qualify, worker_lock
from process import ProcessError, run
from state import Journal, canonical, receipt_backend

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
    policy_digest = commands.add_parser("policy-digest")
    policy_digest.add_argument("--target", required=True)
    add = commands.add_parser("admit")
    add.add_argument("--channel", required=True)
    add.add_argument("--task", required=True)
    for name in ("dispatch", "recover", "result", "cancel"):
        command = commands.add_parser(name)
        command.add_argument("--attempt", required=True)
    commands.add_parser("execute")
    args = parser.parse_args(argv)
    policy = load_policy(args.policy)
    if args.action == "policy-digest":
        host = policy.get("hosts", {}).get(args.target)
        if host is None:
            require(args.target == policy.get("alias"), "host_not_allowed")
            host = policy
        print(canonical({"target": args.target, "machine_id": host["machine_id"],
                         "policy_digest": authority_digest(policy, args.target, host)}))
        return 0
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
