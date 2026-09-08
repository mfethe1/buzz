"""Single-use start acknowledgement, local receipt and bounded qualification."""

from contextlib import contextmanager
import copy
import json
import os
from pathlib import Path
import re
import sys
import time

from admission import Buzz, EXTENSION, HEX, file_digest, grant, publish_transition, require
from process import ProcessError, run
from state import Journal, canonical, digest, receipt_backend

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


def publish_receipt(buzz, journal, request, result):
    """Freeze the signed receipt identity; a lost reply only retries delivery."""
    scope = request["snapshot"]["extensions"][EXTENSION]
    qualification = result.get("qualification")
    wire = {"attempt_id": request["attempt_id"], "task_id": request["task_id"],
            "plan_event_id": request["head"], "start_event_id": result.get("start_event_id"),
            "machine_id": scope["machine_id"], "policy_digest": request["policy_digest"],
            "status": result["status"], "completed_at": result["completed_at"],
            "error": result.get("error"), "qualification": None}
    if qualification:
        wire["qualification"] = {key: qualification[key] for key in
                                  ("repository", "head_sha", "tracked_files", "python")}
    transition = "receipt:" + ("unknown" if wire["status"] == "unknown" else "terminal")
    item = journal.freeze(request["attempt_id"], transition, str(int(time.time())), wire)
    frozen = json.loads(item["snapshot"])
    tags = [["protocol", "buzz-fleet-receipt", "1"], ["h", request["channel"]],
            ["d", request["attempt_id"]]]
    def event_id(published_at):
        return digest([0, buzz.policy["worker_pubkey"], published_at, 43004, tags, canonical(frozen)])
    def delivered(projection):
        receipt = projection.get("receipt")
        if not isinstance(receipt, dict) or receipt.get("content") != canonical(frozen):
            return None
        published_at = receipt.get("created_at")
        require(isinstance(published_at, int) and receipt.get("pubkey") == buzz.policy["worker_pubkey"]
                and receipt.get("kind") == 43004 and receipt.get("tags") == tags,
                "receipt_projection_binding_mismatch")
        expected = event_id(published_at)
        require(receipt.get("id") == expected == projection.get("receipt_event_id"),
                "receipt_projection_id_mismatch")
        return expected
    if item["event_id"]:
        return item["event_id"]
    projection = buzz.attempt(request)
    accepted = delivered(projection)
    if accepted:
        journal.sent(request["attempt_id"], transition, accepted)
        return accepted
    # Ordinary ingest rejects stale envelopes. After observing no matching
    # committed receipt, refresh only its publication time; retain completion
    # time and the immutable result. A delayed delivery cannot renew a start.
    published_at = int(item["previous"])
    if time.time() - published_at > 600:
        published_at = int(time.time())
        journal.refresh_receipt_delivery(request["attempt_id"], transition, item["previous"], published_at)
    expected = event_id(published_at)
    try:
        response = buzz.receipt(request, frozen, published_at)
        require(response == {"accepted": True, "event_id": expected}, "receipt_not_accepted")
        accepted = expected
    except (ProcessError, ValueError, OSError):
        # Display reconciliation is safe only for result delivery, never start.
        accepted = delivered(buzz.attempt(request))
        require(accepted, "receipt_delivery_unknown")
    journal.sent(request["attempt_id"], transition, accepted)
    return accepted


def unknown_outcome(buzz, journal, request):
    """A prior start intent or started receipt never permits a second probe."""
    attempt = request["attempt_id"]
    journal.update(attempt, "outcome_unknown")
    with journal.connect() as db:
        start = db.execute("SELECT event_id FROM outbox WHERE attempt=? AND transition='start'",
                           (attempt,)).fetchone()
    if start and start["event_id"]:
        # Even a known accepted start is not proof of running or stopping. Publish
        # exactly that uncertainty when the relay is available; any delivery
        # failure propagates, retaining the durable outbox for a later retry.
        publish_receipt(buzz, journal, request, {"status": "unknown", "error": "outcome_unknown",
                        "start_event_id": start["event_id"], "completed_at": int(time.time())})
    return {"attempt_id": attempt, "state": "outcome_unknown", "automatic_replay": False}


def execute(policy, request):
    journal = Journal(policy["state_dir"])
    scope, repository = grant(policy, request, local=True, check_deadline=False)
    journal.admit(request)
    attempt = request["attempt_id"]
    store = receipt_backend(policy)
    buzz = Buzz(policy)
    with worker_lock(journal):
        existing = store.get(attempt)
        if existing["state"] != "not_recorded":
            require(existing["request_digest"] == store.digest(request), "receipt_payload_conflict")
        if existing["state"] == "finished":
            return finish_delivery(policy, journal, request, existing["result"])
        if existing["state"] != "not_recorded":
            return unknown_outcome(buzz, journal, request)
        # An outbox row is written BEFORE attempting the network start. On any
        # restart, that row denotes consumed-or-unknown authority, never a lease
        # that another process may reuse. This also closes the ACK/local-save gap.
        with journal.connect() as db:
            prior_start = db.execute("SELECT 1 FROM outbox WHERE attempt=? AND transition='start'",
                                     (attempt,)).fetchone()
        if prior_start:
            return unknown_outcome(buzz, journal, request)
        if journal.cancelled(attempt):
            return stopped_before_execution(buzz, journal, request)
        require(time.time() < scope["expires_at"], "grant_expired")
        projection = buzz.attempt(request)
        require(projection["state"] in ("planned", "claimed"), "relay_attempt_not_startable")
        current = buzz.reduce(request["channel"], request["task_id"])
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
        item = journal.freeze(attempt, "start", head, working)
        working = json.loads(item["snapshot"])
        try:
            # Unlike claim/result delivery, start is never reconciled from a
            # reduced snapshot or accepted duplicate. The native CLI rejects
            # duplicate responses; only this call's new ACK may proceed.
            response = buzz.publish(request, "start", item["previous"], working)
            require(response.get("accepted") is True and HEX.fullmatch(response.get("event_id", "")),
                    "start_not_newly_accepted")
            head = response["event_id"]
            journal.sent(attempt, "start", head)
            # The writer read also proves this is an atomic-admission relay;
            # legacy event-only relays cannot enable a probe with a plain OK.
            buzz.admission(request, head)
        except (ProcessError, ValueError, OSError):
            return unknown_outcome(buzz, journal, request)
        if journal.cancelled(attempt):
            # No process has been spawned, but a start was durably accepted.
            # Record a typed cancelled receipt rather than pretending no start.
            result = {"status": "cancelled", "error": "cancelled"}
        else:
            result = None
        receipt = store.claim(attempt, request)
        require(receipt["state"] == "new", "automatic_replay_refused")
        journal.update(attempt, "running")
        if result is None:
            try:
                result = {"status": "success", "qualification": qualify(
                    policy, repository, cancelled=lambda: journal.cancelled(attempt) or time.time() >= scope["expires_at"])}
            except (ValueError, ProcessError, OSError) as error:
                reason = "grant_expired" if str(error) == "cancelled" and time.time() >= scope["expires_at"] else str(error)
                # Fixed bounded error categories avoid exporting paths/secrets.
                reason = "cancelled" if reason == "cancelled" else "qualification_failed"
                result = {"status": "cancelled" if reason == "cancelled" else "error", "error": reason}
        result.update(attempt_id=attempt, task_id=request["task_id"], plan_event_id=request["head"],
                      start_event_id=head, host=policy["alias"], completed_at=int(time.time()),
                      policy_digest=request["policy_digest"], protocol=request["protocol"],
                      reducer_sha256=file_digest(policy["buzz_binary"]))
        store.finish(attempt, request, result)
        journal.update(attempt, "result_pending", result)
        return finish_delivery(policy, journal, request, result)


def finish_delivery(policy, journal, request, result):
    """Persist a typed outcome before advancing CML; never approve our own work."""
    attempt = request["attempt_id"]
    if journal.get(attempt)["state"] == "delivered":
        return {"attempt_id": attempt, "state": "delivered", "result": result}
    buzz = Buzz(policy)
    receipt_id = publish_receipt(buzz, journal, request, result)
    current = buzz.reduce(request["channel"], request["task_id"])
    if current["snapshot"]["status"] == "cancelled":
        journal.update(attempt, "cancel_acknowledged", result)
        return {"attempt_id": attempt, "state": "cancel_acknowledged", "receipt_event_id": receipt_id, "result": result}
    with journal.connect() as db:
        start = db.execute("SELECT event_id,snapshot FROM outbox WHERE attempt=? AND transition='start'",
                           (attempt,)).fetchone()
    require(start and start["event_id"], "start_receipt_missing")
    snapshot = json.loads(start["snapshot"])
    snapshot["updated_at"] = max(result["completed_at"], snapshot["updated_at"])
    snapshot["evidence"].append({"kind": "fleet-qualification-receipt", "reference": receipt_id})
    if result["status"] == "success":
        snapshot["status"] = "review"
        snapshot["git"]["head_sha"] = result["qualification"]["head_sha"]
        transition = "submit"
    else:
        snapshot["status"] = "blocked"
        snapshot["blockers"].append({"id": "fleet-qualification", "text": result.get("error", "cancelled"),
                                     "reference": receipt_id})
        transition = "block"
    event_id, _ = publish_transition(buzz, journal, request, transition, start["event_id"], snapshot)
    journal.update(attempt, "delivered", result)
    return {"attempt_id": attempt, "state": "delivered", "event_id": event_id,
            "receipt_event_id": receipt_id, "result": result}


def stopped_before_execution(buzz, journal, request):
    """Called only under the local worker lock after signed cancellation."""
    projection = buzz.attempt(request)
    require(projection.get("cancel_event_id"), "signed_planner_cancellation_required")
    if projection.get("start_event_id"):
        return unknown_outcome(buzz, journal, request)
    result = {"status": "cancelled_before_execution", "completed_at": int(time.time())}
    receipt_id = publish_receipt(buzz, journal, request, result)
    journal.update(request["attempt_id"], "cancelled_before_execution", result)
    return {"attempt_id": request["attempt_id"], "state": "cancelled_before_execution",
            "receipt_event_id": receipt_id}


def cancel_local(policy, request):
    """Intent is not acknowledgement; only a persisted terminal receipt is."""
    grant(policy, request, local=True, check_deadline=False)
    journal = Journal(policy["state_dir"])
    journal.admit(request)
    buzz = Buzz(policy)
    current = buzz.reduce(request["channel"], request["task_id"])
    require(current["snapshot"]["status"] == "cancelled", "signed_planner_cancellation_required")
    for field in ("roles", "extensions", "id"):
        require(current["snapshot"][field] == request["snapshot"][field], "cancellation_contract_changed")
    journal.cancel(request["attempt_id"])
    try:
        with worker_lock(journal):
            receipt = receipt_backend(policy).get(request["attempt_id"])
            if receipt["state"] == "finished":
                return finish_delivery(policy, journal, request, receipt["result"])
            if receipt["state"] == "not_recorded":
                return stopped_before_execution(buzz, journal, request)
            return unknown_outcome(buzz, journal, request)
    except BlockingIOError:
        # A running holder observes the flag and reaps its process tree before
        # writing its final receipt. We can only report intent until then.
        return {"attempt_id": request["attempt_id"], "state": "cancel_requested"}
