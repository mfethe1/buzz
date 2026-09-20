"""Fault-injection tests of the production CLI, journal, host executor, and git probe.

The fake relay CLI models transport failures; it does not replace native Buzz
cryptographic tests or the required live signed-event qualification run.
"""

import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
import uuid

import fleet
from state import CompatibleReceipts, Journal, canonical, digest


FAKE_BUZZ = r'''
import hashlib,json,os,pathlib,sys
path=pathlib.Path(os.environ['FLEET_FAKE_RELAY'])
data=json.loads(path.read_text())
args=sys.argv[1:]
if 'reduce' in args:
 print(json.dumps(data['current']));sys.exit(0)
if 'publish' not in args:sys.exit(9)
transition=args[args.index('--transition')+1]
previous=args[args.index('--prev')+1]
if previous!=data['current']['head']:sys.exit(7)
snapshot=json.load(sys.stdin)
head=hashlib.sha256(json.dumps([transition,previous,snapshot],sort_keys=True).encode()).hexdigest()
data['current']={'verdict':'ok','head':head,'snapshot':snapshot}
data['published'].append(transition)
path.write_text(json.dumps(data))
if os.environ.get('FLEET_LOSE_REPLY')==transition and not data.get('lost'):
 data['lost']=True;path.write_text(json.dumps(data));sys.exit(3)
print(json.dumps({'accepted':True,'event_id':head}))
'''


class QualificationTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="buzz-fleet-test-")
        self.root = Path(self.tmp.name)
        self.env = os.environ.copy()
        self.repo = self.root / "repository"
        self.repo.mkdir()
        self.git = shutil.which("git")
        subprocess.run([self.git, "init", "-q", str(self.repo)], check=True)
        (self.repo / "file.txt").write_text("fixture\n")
        subprocess.run([self.git, "-C", str(self.repo), "add", "file.txt"], check=True)
        subprocess.run([self.git, "-C", str(self.repo), "-c", "user.name=Fleet Test",
                        "-c", "user.email=fleet@example.invalid", "-c", "commit.gpgsign=false",
                        "commit", "-qm", "fixture"], check=True)
        self.fake = self.root / "buzz"
        self.fake.write_text("#!" + sys.executable + "\n" + FAKE_BUZZ)
        self.fake.chmod(0o700)
        self.relay = self.root / "relay.json"
        os.environ["FLEET_FAKE_RELAY"] = str(self.relay)
        self.channel = str(uuid.uuid4())
        self.task_id = str(uuid.uuid4())
        now = int(time.time())
        self.snapshot = {
            "id": self.task_id, "status": "planned", "updated_at": now,
            "title": "Qualify this repository", "objective": "Observe repository state without edits",
            "priority": "P2", "protocol": "buzz-cml", "version": 1,
            "roles": {"planner": "1" * 64, "worker": "2" * 64,
                      "reviewer": "3" * 64, "fixer": None},
            "git": {"repo": "mfethe1/buzz", "base_sha": "a" * 40,
                    "branch": "codex/qualification", "head_sha": None, "worktree_alias": "qualification"},
            "extensions": {fleet.EXTENSION: {"target": "mack", "repository": "buzz",
                                             "capability": "qualify", "expires_at": now + 300}},
            "evidence": [], "blockers": [], "acceptance": [], "lease": None,
            "review": {"round": 0, "max_rounds": 3},
            "runtime": {"host_id": None, "last_heartbeat_at": None, "presence": "offline", "ttl_seconds": 180},
        }
        self.relay.write_text(json.dumps({"current": {"verdict": "ok", "head": "a" * 64,
                                                       "snapshot": self.snapshot}, "published": []}))
        self.host = {"version": 1, "alias": "mack", "relay": "http://relay.invalid",
                     "buzz_binary": str(self.fake), "git_binary": self.git,
                     "buzz_binary_sha256": fleet.file_digest(self.fake),
                     "state_dir": str(self.root / "host"), "receipts_path": str(self.root / "host/receipts.sqlite"),
                     "planner_pubkeys": ["1" * 64], "worker_pubkey": "2" * 64,
                     "channels": [self.channel],
                     "repositories": {"buzz": {"id": "mfethe1/buzz", "path": str(self.repo)}}}
        self.host_path = self.root / "host-policy.json"
        self.host_path.write_text(json.dumps(self.host))
        self.scheduler = dict(self.host, state_dir=str(self.root / "scheduler"),
                              receipts_path=str(self.root / "scheduler/receipts.sqlite"))
        self.scheduler["hosts"] = {"mack": dict(self.host, command=[sys.executable, fleet.__file__,
                                                                  "--policy", str(self.host_path), "execute"])}
        self.scheduler_path = self.root / "scheduler-policy.json"
        self.scheduler_path.write_text(json.dumps(self.scheduler))

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.env)
        self.tmp.cleanup()

    def admit(self):
        return fleet.admit(self.scheduler, self.channel, self.task_id)["request"]

    def test_cli_dispatch_duplicate_and_receipt_retrieval(self):
        def cli(*args):
            result = subprocess.run([sys.executable, fleet.__file__, "--policy", str(self.scheduler_path), *args],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return json.loads(result.stdout)
        admitted = cli("admit", "--channel", self.channel, "--task", self.task_id)
        attempt = admitted["request"]["attempt_id"]
        first = cli("dispatch", "--attempt", attempt)
        self.assertEqual(first["state"], "delivered")
        self.assertEqual(first["result"]["qualification"]["tracked_files"], 1)
        self.assertFalse(first["result"]["qualification"]["agent_inference_exercised"])
        second = cli("dispatch", "--attempt", attempt)
        self.assertEqual(second["result"], first["result"])
        self.assertEqual(json.loads(self.relay.read_text())["published"], ["claim", "start", "submit"])
        self.assertEqual(cli("result", "--attempt", attempt)["result"], first["result"])
        self.assertEqual(json.loads(self.relay.read_text())["current"]["snapshot"]["status"], "review")
        print("FLEET_QUALIFICATION_OK: actual git probe, durable receipt, CML review, duplicate suppressed")

    def test_lost_submit_reply_recovers_frozen_outbox_without_probe(self):
        request = self.admit()
        os.environ["FLEET_LOSE_REPLY"] = "submit"
        with self.assertRaises(fleet.ProcessError):
            fleet.execute(self.host, request)
        receipt = CompatibleReceipts(self.host["receipts_path"]).get(request["attempt_id"])
        self.assertEqual(receipt["state"], "finished")
        # A rerun of the index probe would now produce a different answer.
        (self.repo / "second.txt").write_text("added after execution\n")
        subprocess.run([self.git, "-C", str(self.repo), "add", "second.txt"], check=True)
        result = fleet.execute(self.host, request)
        self.assertEqual(result["state"], "delivered")
        self.assertEqual(result["result"]["qualification"]["tracked_files"], 1)
        self.assertEqual(json.loads(self.relay.read_text())["published"], ["claim", "start", "submit"])

    def test_lost_claim_reply_resumes_before_execution(self):
        request = self.admit()
        os.environ["FLEET_LOSE_REPLY"] = "claim"
        with self.assertRaises(fleet.ProcessError):
            fleet.execute(self.host, request)
        self.assertEqual(fleet.execute(self.host, request)["state"], "delivered")
        self.assertEqual(json.loads(self.relay.read_text())["published"], ["claim", "start", "submit"])

    def test_unknown_execution_refuses_replay_after_restart(self):
        request = self.admit()
        Journal(self.host["state_dir"]).admit(request)
        CompatibleReceipts(self.host["receipts_path"]).claim(request["attempt_id"], request)
        result = fleet.execute(self.host, request)
        self.assertEqual(result["state"], "outcome_unknown")
        self.assertFalse(result["automatic_replay"])
        self.assertEqual(json.loads(self.relay.read_text())["published"], [])

    def test_transport_preserves_unknown_execution_response(self):
        request = self.admit()
        Journal(self.host["state_dir"]).admit(request)
        CompatibleReceipts(self.host["receipts_path"]).claim(request["attempt_id"], request)
        response = fleet.dispatch(self.scheduler, request["attempt_id"])
        self.assertEqual(response["state"], "outcome_unknown")
        self.assertFalse(response["automatic_replay"])

    def test_reject_scope_tampering_and_unknown_capability(self):
        request = self.admit()
        for key, value in (("target", "rosie"), ("repository", "../../"), ("capability", "shell"),
                           ("expires_at", 1), ("expires_at", int(time.time()) + 99999)):
            with self.subTest(key=key, value=value):
                bad = copy.deepcopy(request)
                bad["snapshot"]["extensions"][fleet.EXTENSION][key] = value
                with self.assertRaises(ValueError):
                    fleet.execute(self.host, bad)
        bad = copy.deepcopy(request)
        bad["snapshot"]["roles"]["planner"] = "9" * 64
        with self.assertRaisesRegex(ValueError, "planner_not_allowed"):
            fleet.execute(self.host, bad)
        bad = copy.deepcopy(request)
        bad["policy_digest"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "policy_digest_mismatch"):
            fleet.execute(self.host, bad)
        self.assertEqual(json.loads(self.relay.read_text())["published"], [])

    def test_admission_rejects_an_otherwise_valid_unknown_capability_plan(self):
        data = json.loads(self.relay.read_text())
        data["current"]["snapshot"]["extensions"][fleet.EXTENSION]["capability"] = "shell"
        self.relay.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "capability_not_allowed"):
            self.admit()

    def test_policy_binary_pin_is_enforced_at_cli_entry(self):
        self.fake.write_text(self.fake.read_text() + "\n# modified binary\n")
        with self.assertRaisesRegex(ValueError, "buzz_binary_digest_mismatch"):
            fleet.load_policy(self.host_path)

    def test_inherited_git_environment_cannot_redirect_repository(self):
        request = self.admit()
        os.environ["GIT_DIR"] = str(self.root / "does-not-exist")
        os.environ["GIT_WORK_TREE"] = str(self.root / "another-repository")
        result = fleet.execute(self.host, request)
        self.assertEqual(result["result"]["status"], "success")
        self.assertEqual(result["result"]["qualification"]["repository"], "mfethe1/buzz")

    def test_repository_clean_filter_cannot_execute(self):
        marker = self.root / "filter-executed"
        (self.repo / ".gitattributes").write_text("file.txt filter=probe\n")
        import shlex
        subprocess.run([self.git, "-C", str(self.repo), "config", "filter.probe.clean",
                        "touch " + shlex.quote(str(marker)) + "; cat"], check=True)
        (self.repo / "file.txt").write_text("mutated\n")
        future = time.time() + 5
        os.utime(self.repo / "file.txt", (future, future))
        result = fleet.execute(self.host, self.admit())
        self.assertEqual(result["result"]["status"], "success")
        self.assertFalse(marker.exists(), "repository clean filter ran inside qualification")
        self.assertNotIn("tracked_changes", result["result"]["qualification"])

    def test_windows_admission_fails_before_process_or_publication(self):
        request = self.admit()
        with mock.patch("fleet.os.name", "nt"):
            with self.assertRaisesRegex(ValueError, "windows_containment_unqualified"):
                fleet.grant(self.host, request, local=True)
        self.assertEqual(json.loads(self.relay.read_text())["published"], [])

    def test_changed_relay_head_does_not_execute(self):
        request = self.admit()
        data = json.loads(self.relay.read_text())
        data["current"]["head"] = "b" * 64
        self.relay.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "cml_head_changed"):
            fleet.execute(self.host, request)
        self.assertEqual(json.loads(self.relay.read_text())["published"], [])

    def test_cancellation_requires_signed_state_and_acknowledges_no_spawn(self):
        request = self.admit()
        with self.assertRaisesRegex(ValueError, "signed_planner_cancellation_required"):
            fleet.cancel_local(self.host, request)
        data = json.loads(self.relay.read_text())
        data["current"]["snapshot"]["status"] = "cancelled"
        data["current"]["head"] = "c" * 64
        self.relay.write_text(json.dumps(data))
        result = fleet.cancel_remote(self.scheduler, request["attempt_id"])
        self.assertEqual(result["state"], "cancelled_before_execution")
        self.assertEqual(fleet.execute(self.host, request)["state"], "cancelled_before_execution")
        self.assertEqual(json.loads(self.relay.read_text())["published"], [])

    def test_duplicate_payload_conflict_is_durable(self):
        request = self.admit()
        Journal(self.host["state_dir"]).admit(request)
        bad = copy.deepcopy(request)
        bad["snapshot"]["objective"] = "changed"
        with self.assertRaisesRegex(ValueError, "attempt_payload_conflict"):
            fleet.execute(self.host, bad)

    def test_running_cancellation_reaps_process_before_acknowledgement(self):
        request = self.admit()
        marker = self.root / "probe-started"
        wrapper = self.root / "slow-git"
        wrapper.write_text("#!" + sys.executable + "\n" +
                           "import pathlib,subprocess,sys,time\n" +
                           "if 'ls-files' in sys.argv:\n" +
                           " pathlib.Path(" + repr(str(marker)) + ").write_text('started')\n" +
                           " time.sleep(30)\n" +
                           "sys.exit(subprocess.call([" + repr(self.git) + "]+sys.argv[1:]))\n")
        wrapper.chmod(0o700)
        self.host["git_binary"] = str(wrapper)
        outcome = []
        worker = threading.Thread(target=lambda: outcome.append(fleet.execute(self.host, request)))
        worker.start()
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.025)
        self.assertTrue(marker.exists(), "production git probe never started")
        data = json.loads(self.relay.read_text())
        data["current"]["snapshot"]["status"] = "cancelled"
        data["current"]["head"] = "c" * 64
        self.relay.write_text(json.dumps(data))
        self.assertEqual(fleet.cancel_local(self.host, request)["state"], "cancel_requested")
        worker.join(timeout=5)
        self.assertFalse(worker.is_alive(), "cancel did not finish the running process")
        self.assertEqual(outcome[0]["state"], "cancel_acknowledged")
        self.assertEqual(outcome[0]["result"]["status"], "cancelled")
        self.assertEqual(CompatibleReceipts(self.host["receipts_path"]).get(request["attempt_id"])["state"], "finished")

    def test_output_and_timeout_bounds(self):
        with self.assertRaisesRegex(fleet.ProcessError, "output_limit"):
            fleet.run([sys.executable, "-c", "print('x'*300000)"], timeout=3)
        with self.assertRaisesRegex(fleet.ProcessError, "timeout"):
            fleet.run([sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.1)


if __name__ == "__main__":
    unittest.main()
