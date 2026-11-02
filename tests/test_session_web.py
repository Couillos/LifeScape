import json
import os
import tempfile
import unittest

from orchestration import paths, web
from orchestration.daemon import Orchestrator
from orchestration.jobs import Job


class SessionWebTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.saved = (paths.USERS_DIR, paths.LOGS_DIR)
        paths.USERS_DIR = os.path.join(self.directory, "users")
        paths.LOGS_DIR = os.path.join(self.directory, "logs")
        paths.ensure_dirs()
        self.daemon = Orchestrator()
        self.job = Job("alice")
        self.job.phase = "running"
        self.job.script = "fishing"
        self.job.status = "running"
        self.job.last_error = {"where": "run", "type": "ValueError", "message": "boom", "at": []}
        self.daemon.jobs["alice"] = self.job

    def tearDown(self):
        paths.USERS_DIR, paths.LOGS_DIR = self.saved

    def test_status_exposes_session_fields_and_error(self):
        reply = self.daemon.dispatch({"cmd": "status"})
        self.assertTrue(reply["ok"])
        entry = reply["jobs"][0]
        self.assertEqual(entry["script"], "fishing")
        self.assertEqual(entry["status"], "running")
        self.assertEqual(entry["last_error"]["type"], "ValueError")
        payload = json.dumps(reply)
        self.assertNotIn("needs_human", payload)

    def test_web_renders_new_statuses_and_last_error(self):
        self.assertIn("reconnecting", web.INDEX_HTML)
        self.assertIn("disconnected", web.INDEX_HTML)
        self.assertIn("last error", web.INDEX_HTML)
        self.assertNotIn("needs_human", web.INDEX_HTML)
        self.assertNotIn("restarting", web.INDEX_HTML)
        self.assertNotIn("cleaning", web.INDEX_HTML)
        self.assertNotIn("repairing", web.INDEX_HTML)


if __name__ == "__main__":
    unittest.main()
