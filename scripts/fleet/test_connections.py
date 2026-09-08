"""Real retained SQLite handles and actual executor ownership regressions."""
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import fleet
import state
import test_fleet


class ConnectionOwnershipTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fleet-connections-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.native = sqlite3.connect
        self.handles = []
        self.configure = lambda db: None

        def connect(*args, **kwargs):
            db = self.native(*args, **kwargs)
            self.handles.append(db)  # Do not allow GC to mask missing close().
            self.configure(db)
            return db

        self.addCleanup(lambda: [db.close() for db in self.handles])
        patcher = patch.object(state.sqlite3, 'connect', connect)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.request = {'attempt_id': 'attempt', 'text': 'café'}

    def assert_closed(self):
        self.assertTrue(self.handles)
        for db in self.handles:
            with self.assertRaisesRegex(sqlite3.ProgrammingError, 'closed database'):
                db.execute('SELECT 1')

    def test_journal_success_conflict_cancel_and_outbox_preserve_commits(self):
        journal = state.Journal(self.root / 'journal')
        journal.admit(self.request)
        self.assertEqual(journal.get('attempt')['request'], self.request)
        with self.assertRaisesRegex(ValueError, 'attempt_payload_conflict'):
            journal.admit(dict(self.request, text='changed'))
        journal.update('attempt', 'running', {'value': 1})
        journal.cancel('attempt')
        self.assertTrue(journal.cancelled('attempt'))
        before = journal.freeze('attempt', 'receipt:terminal', '1', {'status': 'success'})
        journal.refresh_receipt_delivery('attempt', 'receipt:terminal', '1', 2)
        after = journal.freeze('attempt', 'receipt:terminal', '3', {'status': 'different'})
        self.assertEqual(after['snapshot'], before['snapshot'])
        self.assertEqual(after['previous'], '2')
        with self.assertRaisesRegex(ValueError, 'receipt_delivery_generation_changed'):
            journal.refresh_receipt_delivery('attempt', 'receipt:terminal', '1', 4)
        journal.sent('attempt', 'receipt:terminal', 'receipt-id')
        self.assertEqual(journal.freeze('attempt', 'receipt:terminal', '4', {})['event_id'], 'receipt-id')
        restarted = state.Journal(self.root / 'journal')
        self.assertEqual(restarted.get('attempt')['result'], {'value': 1})
        self.assertTrue(restarted.cancelled('attempt'))
        self.assert_closed()

    def test_receipts_success_restart_and_conflict_preserve_commits(self):
        store = state.CompatibleReceipts(self.root / 'receipts.sqlite')
        self.assertEqual(store.claim('attempt', self.request), {'state': 'new'})
        self.assertEqual(store.claim('attempt', self.request)['state'], 'started')
        self.assertEqual(store.claim('attempt', {'changed': True}), {'state': 'conflict'})
        store.finish('attempt', self.request, {'status': 'success'})
        prior = store.get('attempt')
        with self.assertRaisesRegex(ValueError, 'receipt_payload_conflict'):
            store.finish('attempt', {'changed': True}, {'status': 'error'})
        self.assertEqual(state.CompatibleReceipts(store.path).get('attempt'), prior)
        self.assertEqual(store.get('missing')['state'], 'not_recorded')
        self.assert_closed()

    def test_setup_pragma_failure_closes_both_connection_factories(self):
        def deny(db):
            db.set_authorizer(lambda action, a, b, database, trigger:
                sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_PRAGMA and a == 'synchronous'
                else sqlite3.SQLITE_OK)
        self.configure = deny
        for cls, path in [(state.Journal, self.root / 'journal'),
                          (state.CompatibleReceipts, self.root / 'receipts.sqlite')]:
            with self.subTest(cls=cls.__name__):
                with self.assertRaisesRegex(sqlite3.DatabaseError, 'not authorized'):
                    cls(path)
                self.assert_closed()

    def test_schema_failure_closes_both_owned_connections(self):
        self.configure = lambda db: db.execute('PRAGMA query_only=ON')
        for cls, path in [(state.Journal, self.root / 'journal'),
                          (state.CompatibleReceipts, self.root / 'receipts.sqlite')]:
            with self.subTest(cls=cls.__name__):
                with self.assertRaisesRegex(sqlite3.OperationalError, 'readonly'):
                    cls(path)
                self.assert_closed()

    def test_commit_failure_rolls_back_then_closes_both_stores(self):
        for cls, path, table, column in [
            (state.Journal, self.root / 'journal', 'attempts', 'id'),
            (state.CompatibleReceipts, self.root / 'receipts.sqlite', 'receipts', 'task_id'),
        ]:
            with self.subTest(cls=cls.__name__):
                store = cls(path)
                with closing(self.native(store.path)) as db, db:
                    db.executescript(f"""
                        CREATE TABLE fixture_parent (id INTEGER PRIMARY KEY);
                        CREATE TABLE fixture_child (parent_id INTEGER REFERENCES fixture_parent(id)
                            DEFERRABLE INITIALLY DEFERRED);
                        CREATE TRIGGER defer_invalid_child AFTER INSERT ON {table}
                        WHEN NEW.{column} = 'broken'
                        BEGIN INSERT INTO fixture_child VALUES (99); END;
                    """)
                self.configure = lambda db: db.execute('PRAGMA foreign_keys=ON')
                with self.assertRaisesRegex(sqlite3.IntegrityError, 'FOREIGN KEY constraint failed'):
                    if cls is state.Journal:
                        store.admit(dict(self.request, attempt_id='broken'))
                    else:
                        store.claim('broken', self.request)
                self.assertEqual(store.get('broken')['state'], 'not_recorded')
                with closing(self.native(store.path)) as db:
                    self.assertEqual(db.execute('SELECT count(*) FROM fixture_child').fetchone()[0], 0)
                self.assert_closed()

    def fixture(self):
        fixture = test_fleet.QualificationTest()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        return fixture

    def test_actual_executor_and_duplicate_close_direct_journal_reads(self):
        fixture = self.fixture()
        request = fixture.admit()
        first = fleet.execute(fixture.host, request)
        self.assertEqual(first['state'], 'delivered')
        self.assertEqual(first['result']['qualification']['tracked_files'], 1)
        again = fleet.execute(fixture.host, request)
        self.assertEqual(again['result'], first['result'])
        self.assertEqual(json.loads(fixture.relay.read_text())['published'], ['claim', 'start', 'submit'])
        self.assert_closed()

    def test_actual_lost_start_and_unknown_recovery_close_direct_reads(self):
        fixture = self.fixture()
        request = fixture.admit()
        with patch.dict('os.environ', {'FLEET_LOSE_REPLY': 'start'}):
            self.assertEqual(fleet.execute(fixture.host, request)['state'], 'outcome_unknown')
        self.assertEqual(fleet.execute(fixture.host, request)['state'], 'outcome_unknown')
        self.assertEqual(json.loads(fixture.relay.read_text())['published'], ['claim', 'start'])
        self.assert_closed()


if __name__ == '__main__':
    unittest.main(verbosity=2)
