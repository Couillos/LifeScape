import threading
import unittest

from lifescape.client import Bot
from orchestration import session_worker
from orchestration.session_worker import DENY_METHODS


class CallGuardsTest(unittest.TestCase):
    def setUp(self):
        self.worker = session_worker.SessionWorker("alice")
        self.worker.bot = Bot()

    def test_unknown_method_is_rejected(self):
        reply = self.worker.cmd_call("no_such_method", [], False)
        self.assertFalse(reply["ok"])
        self.assertIn("unknown method", reply["error"])

    def test_denied_methods_are_rejected(self):
        for method in ("login", "relogin", "close", "snapshot", "queue_command"):
            reply = self.worker.cmd_call(method, [], False)
            self.assertFalse(reply["ok"], method)
            self.assertIn("unknown method", reply["error"])

    def test_cheat_refused_for_non_admin(self):
        reply = self.worker.cmd_call("cheat", ["::test"], False)
        self.assertFalse(reply["ok"])
        self.assertIn("forbidden", reply["error"])

    def test_call_while_busy_is_busy(self):
        self.worker.status = "busy"
        reply = self.worker.cmd_call("intend", ["hello"], False)
        self.assertFalse(reply["ok"])
        self.assertIn("busy", reply["error"])

    def test_write_while_unload_is_busy(self):
        self.worker.bot.request_stop()
        try:
            reply = self.worker.cmd_call("intend", ["hello"], False)
        finally:
            self.worker.bot.clear_stop()
        self.assertFalse(reply["ok"])
        self.assertIn("busy", reply["error"])

    def test_read_served_from_snapshot_without_pump(self):
        self.worker.bot._snapshot = {
            "inv": {"3200": {0: (995, 50)}},
            "stats": {},
            "texts": {},
            "position": (0, 1, 2),
            "modal": None,
            "com_ids": {},
            "inv_cap": {},
        }
        reply = self.worker.cmd_call("inventory", [], False)
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["result"], {995: 50})

    def test_write_runs_at_safe_point(self):
        timer = threading.Timer(0.1, self.worker.bot._drain_command)
        timer.start()
        try:
            reply = self.worker.cmd_call("intend", ["hello"], False)
        finally:
            timer.join()
        self.assertTrue(reply["ok"])
        self.assertIsNone(reply["result"])
        self.assertEqual(self.worker.bot._intent, "hello")


class DenyListTest(unittest.TestCase):
    def test_session_internals_never_callable(self):
        for method in ("login", "relogin", "close", "snapshot", "queue_command",
                       "drop_command", "request_stop", "stop_requested",
                       "clear_stop", "invalidate_sends"):
            self.assertIn(method, DENY_METHODS, method)


if __name__ == "__main__":
    unittest.main()
