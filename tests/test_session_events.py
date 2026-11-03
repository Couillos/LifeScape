import json
import os
import tempfile
import unittest

from orchestration import paths
from orchestration.daemon import Orchestrator
from orchestration.jobs import Job
from orchestration import session_worker
import orchestration.daemon as daemon_mod


class StubWorld:
    def __init__(self):
        self.ticks = 0
        self.position = (0, 3200, 3200)


class StubBot:
    def __init__(self):
        self.world = StubWorld()

    def intend(self, _intention):
        pass

    def note_run_thread(self):
        pass


class WorkerEventTest(unittest.TestCase):
    def test_random_event_abort_unloads_for_rescue(self):
        worker = session_worker.SessionWorker("alice")
        worker.bot = StubBot()

        def abort(bot, *args):
            raise SystemExit("random event unresolved at (0,3200,3200)")

        worker.phase = "running"
        worker.script = "fishing"
        worker.run_thread(abort, "fishing", ["1"])
        self.assertEqual(worker.status, "idle")
        self.assertEqual(worker.phase, "idle")
        self.assertIsNone(worker.script)
        self.assertFalse(worker.last_event["resolved"])

    def test_mapped_rescue_name_comes_from_registry(self):
        from orchestration import registry

        self.assertEqual(registry.RESCUES["maze"], "rescue_maze")
        self.assertEqual(registry.RESCUES["tower"], "rescue_down")
        self.assertEqual(registry.RESCUES["mime"], "rescue_mime")


class DaemonRescueTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.saved = (paths.USERS_DIR, paths.LOGS_DIR, daemon_mod.USERS_DIR)
        paths.USERS_DIR = os.path.join(self.directory, "users")
        paths.LOGS_DIR = os.path.join(self.directory, "logs")
        daemon_mod.USERS_DIR = paths.USERS_DIR
        paths.ensure_dirs()
        self.daemon = Orchestrator()
        self.sent = []
        self.daemon.send_worker = lambda account, request, timeout=40: self.record(account, request)
        self.job = Job("alice")
        self.job.process = None
        self.job.phase = "running"
        self.job.script = "fishing"
        self.job.script_args = ["1"]
        self.job.status = "running"
        self.daemon.jobs["alice"] = self.job

    def tearDown(self):
        paths.USERS_DIR, paths.LOGS_DIR, daemon_mod.USERS_DIR = self.saved
        self.daemon.running = False

    def record(self, account, request):
        self.sent.append(request)
        return {"ok": True}

    def test_unresolved_maze_runs_mapped_rescue(self):
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "maze", "resolved": False})
        self.assertEqual(self.job.rescue_for, "rescue_maze")
        self.assertEqual(self.job.interrupted, ("fishing", ["1"]))
        self.assertEqual([request["cmd"] for request in self.sent], ["unload", "run"])

    def test_unmapped_event_runs_fallback(self):
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "swarm", "resolved": False})
        self.assertEqual(self.job.rescue_for, "rescue_fallback")

    def test_successful_rescue_resumes_interrupted_script(self):
        self.job.rescue_for = "rescue_maze"
        self.job.interrupted = ("fishing", ["1"])
        self.job.phase = "idle"
        self.job.status = "finished"
        self.daemon.rescue_step(self.job)
        self.assertIsNone(self.job.rescue_for)
        self.assertEqual(self.sent[-1], {"cmd": "run", "script": "fishing", "args": ["1"]})

    def test_exhausted_fallback_fails_without_blocking_others(self):
        self.job.rescue_for = "rescue_fallback"
        self.job.interrupted = ("fishing", ["1"])
        self.job.phase = "idle"
        self.job.status = "failed"
        self.daemon.rescue_step(self.job)
        self.assertEqual(self.job.status, "failed")
        self.assertIsNone(self.job.rescue_for)
        other = Job("bob")
        other.phase = "running"
        other.script = "mining"
        other.script_args = []
        other.status = "running"
        self.daemon.handle_event(other, {"kind": "random_event", "event": "swarm", "resolved": False})
        self.assertEqual(other.rescue_for, "rescue_fallback")

    def test_second_event_waits_for_ongoing_rescue(self):
        self.job.rescue_for = "rescue_maze"
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "tower", "resolved": False})
        self.assertEqual(self.job.rescue_for, "rescue_maze")
        self.assertEqual([request["cmd"] for request in self.sent], [])

    def test_interrupted_script_survives_worker_abort(self):
        """Real path: worker aborts (script cleared) before the daemon
        reads the event, yet the interrupted script is still retained."""
        worker = session_worker.SessionWorker("alice")
        worker.bot = StubBot()
        worker.connected = True
        worker.phase = "running"
        worker.script = "fishing"
        worker.script_args = ["1"]
        worker.status = "running"

        def abort(bot, *args):
            raise SystemExit("random event unresolved at (0,3200,3200)")

        worker.run_thread(abort, "fishing", ["1"])
        session = worker.describe()
        self.assertIsNone(session["script"])
        self.assertEqual(session["phase"], "idle")
        self.daemon.apply_session(self.job, session)
        self.assertIsNone(self.job.script)
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "maze", "resolved": False})
        self.assertEqual(self.job.interrupted, ("fishing", ["1"]))
        self.assertEqual(self.job.rescue_for, "rescue_maze")


if __name__ == "__main__":
    unittest.main()
