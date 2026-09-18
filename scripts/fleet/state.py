"""Execution journals and a receipt backend compatible with the Mack bridge."""

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import time


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def digest(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


class Journal:
    """Transport/execution facts only; signed CML remains task authority."""

    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self.directory, 0o700)
        self.path = self.directory / "journal.sqlite"
        with self.connect() as db:
            db.executescript("""
                CREATE TABLE IF NOT EXISTS attempts (
                    id TEXT PRIMARY KEY, request TEXT NOT NULL, state TEXT NOT NULL,
                    cancelled INTEGER NOT NULL DEFAULT 0, result TEXT, updated REAL NOT NULL
                );
                CREATE TABLE IF NOT EXISTS outbox (
                    attempt TEXT NOT NULL, transition TEXT NOT NULL,
                    previous TEXT NOT NULL, snapshot TEXT NOT NULL, event_id TEXT,
                    PRIMARY KEY(attempt, transition)
                );
            """)
        os.chmod(self.path, 0o600)

    def connect(self):
        db = sqlite3.connect(str(self.path), timeout=10)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA synchronous=FULL")
        return db

    def admit(self, request):
        attempt = request["attempt_id"]
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            previous = db.execute("SELECT request FROM attempts WHERE id=?", (attempt,)).fetchone()
            encoded = canonical(request)
            if previous and previous[0] != encoded:
                raise ValueError("attempt_payload_conflict")
            db.execute("INSERT OR IGNORE INTO attempts(id,request,state,updated) VALUES(?,?,'queued',?)",
                       (attempt, encoded, time.time()))
        return self.get(attempt)

    def get(self, attempt):
        with self.connect() as db:
            row = db.execute("SELECT * FROM attempts WHERE id=?", (attempt,)).fetchone()
        if not row:
            return {"attempt_id": attempt, "state": "not_recorded"}
        item = dict(row)
        item["request"] = json.loads(item["request"])
        item["result"] = json.loads(item["result"]) if item["result"] else None
        return item

    def update(self, attempt, state, result=None):
        with self.connect() as db:
            db.execute("UPDATE attempts SET state=?,result=?,updated=? WHERE id=?",
                       (state, canonical(result) if result is not None else None, time.time(), attempt))

    def cancel(self, attempt):
        with self.connect() as db:
            db.execute("UPDATE attempts SET cancelled=1,updated=? WHERE id=?", (time.time(), attempt))

    def cancelled(self, attempt):
        return bool(self.get(attempt).get("cancelled"))

    def freeze(self, attempt, transition, previous, snapshot):
        with self.connect() as db:
            db.execute("INSERT OR IGNORE INTO outbox VALUES(?,?,?,?,NULL)",
                       (attempt, transition, previous, canonical(snapshot)))
            row = db.execute("SELECT * FROM outbox WHERE attempt=? AND transition=?",
                             (attempt, transition)).fetchone()
        return dict(row)

    def sent(self, attempt, transition, event_id):
        with self.connect() as db:
            db.execute("UPDATE outbox SET event_id=? WHERE attempt=? AND transition=?",
                       (event_id, attempt, transition))


class CompatibleReceipts:
    """Mack ReceiptStore schema/API for hosts without the Hermes bridge package.

    Started receipts fail closed after restart. They never authorize replay.
    """

    def __init__(self, path):
        self.path = Path(path)
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with self.connect() as db:
            db.execute("""CREATE TABLE IF NOT EXISTS receipts (
                task_id TEXT PRIMARY KEY, digest TEXT NOT NULL, state TEXT NOT NULL,
                started REAL NOT NULL, updated REAL NOT NULL, result TEXT)""")
        os.chmod(self.path, 0o600)

    def connect(self):
        db = sqlite3.connect(str(self.path), timeout=10)
        db.execute("PRAGMA synchronous=FULL")
        return db

    @staticmethod
    def digest(envelope):
        # Matches the installed bridge exactly, including JSON's ASCII escaping.
        return hashlib.sha256(json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

    def claim(self, task_id, envelope):
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT digest,state,result FROM receipts WHERE task_id=?", (task_id,)).fetchone()
            if row:
                if row[0] != self.digest(envelope):
                    return {"state": "conflict"}
                return {"state": row[1], "result": json.loads(row[2]) if row[2] else None}
            now = time.time()
            db.execute("INSERT INTO receipts VALUES(?,?, 'started',?,?,NULL)",
                       (task_id, self.digest(envelope), now, now))
        return {"state": "new"}

    def finish(self, task_id, envelope, result):
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT digest FROM receipts WHERE task_id=?", (task_id,)).fetchone()
            if not row or row[0] != self.digest(envelope):
                raise ValueError("receipt_payload_conflict")
            db.execute("UPDATE receipts SET state='finished',updated=?,result=? WHERE task_id=?",
                       (time.time(), canonical(result), task_id))

    def get(self, task_id):
        with self.connect() as db:
            row = db.execute("SELECT state,started,updated,result,digest FROM receipts WHERE task_id=?",
                             (task_id,)).fetchone()
        if not row:
            return {"task_id": task_id, "state": "not_recorded"}
        return {"task_id": task_id, "state": row[0], "started": row[1], "updated": row[2],
                "result": json.loads(row[3]) if row[3] else None, "request_digest": row[4]}


def receipt_backend(policy):
    """Use Mack's installed class when explicitly configured; never alter its runner."""
    path = policy["receipts_path"]
    if policy.get("receipts_backend", "compatible") == "hermes":
        from hermes_bridge.receipts import ReceiptStore
        return ReceiptStore(path)
    if policy.get("receipts_backend", "compatible") != "compatible":
        raise ValueError("unknown_receipt_backend")
    return CompatibleReceipts(path)
