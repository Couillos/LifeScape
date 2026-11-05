import json
import os
import tempfile
import unittest

from orchestration import paths, web
from orchestration.daemon import Orchestrator
from orchestration.jobs import Job


def _static(name):
    with open(os.path.join(os.path.dirname(web.__file__), "static", name)) as handle:
        return handle.read()


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
        page = _static("index.html")
        script = _static("spectator.js")
        self.assertIn("reconnecting", script)
        self.assertIn("disconnected", script)
        self.assertIn("last error", script)
        self.assertNotIn("needs_human", page + script)
        self.assertNotIn("restarting", page + script)
        self.assertNotIn("cleaning", page + script)
        self.assertNotIn("repairing", page + script)

    def test_web_renders_map_image_without_detail_mode(self):
        page = _static("index.html")
        script = _static("spectator.js")
        self.assertIn("minimap.png", script)
        self.assertNotIn("detailbtn", page + script)
        self.assertNotIn("detaily", page + script)

    def test_web_serves_session_controls(self):
        page = _static("index.html")
        script = _static("spectator.js")
        for marker in ("btn-open", "btn-run", "btn-stop", "btn-pause", "btn-resume", "btn-close", "drawer-close", "/api/pause"):
            self.assertIn(marker, page + script)
        # same widget on both sides: two <select> dropdowns, no free-text field
        self.assertIn('<select id="account"', page)
        self.assertIn('<select id="script"', page)
        self.assertNotIn("<datalist", page)


if __name__ == "__main__":
    unittest.main()
