import json
import os
import tempfile
import unittest


from orchestration import paths
from orchestration.daemon import Orchestrator
from orchestration.jobs import Job


class FakeProcess:
    def __init__(self, code=None):
        self.code = code
        self.pid = 1234

    def poll(self):
        return self.code

    def terminate(self):
        self.code = 0

    def kill(self):
        self.code = -9


class OrchestratorTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.saved = (paths.USERS_DIR, paths.LOGS_DIR)
        paths.USERS_DIR = os.path.join(self.directory, "users")
        paths.LOGS_DIR = os.path.join(self.directory, "logs")
        paths.ensure_dirs()
        self.daemon = Orchestrator()
        self.spawned = []
        self.daemon.spawn = lambda job, script: self.spawned.append(script) or setattr(job, "process", FakeProcess())
        self.job = Job("alice", "fishing", ["1"])
        self.daemon.jobs["alice"] = self.job

    def tearDown(self):
        paths.USERS_DIR, paths.LOGS_DIR = self.saved

    def test_crash_cleans_then_restarts(self):
        self.job.process = FakeProcess(1)
        self.job.status = "running"
        self.daemon.handle_exit(self.job, 1)
        self.assertEqual(self.job.phase, "cleanup")
        self.assertEqual(self.spawned[-1], "clean_session")

        self.job.process = FakeProcess(0)
        self.job.phase = "cleanup"
        self.daemon.handle_exit(self.job, 0)
        self.assertEqual(self.job.status, "restarting")
        self.assertEqual(self.job.restarts, 1)

    def test_maze_event_triggers_rescue_after_cleanup(self):
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "maze", "resolved": False})
        self.assertEqual(self.job.rescue, "rescue_maze")

        self.job.phase = "cleanup"
        self.daemon.handle_exit(self.job, 0)
        self.assertEqual(self.job.phase, "repair")
        self.assertEqual(self.spawned[-1], "rescue_maze")

        self.job.phase = "repair"
        self.daemon.handle_exit(self.job, 0)
        self.assertEqual(self.job.phase, "cleanup")
        self.assertEqual(self.spawned[-1], "clean_session")

    def test_resolved_event_does_not_rescue(self):
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "maze", "resolved": True})
        self.assertIsNone(self.job.rescue)

    def test_mime_needs_human(self):
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "mime", "resolved": False})
        self.assertEqual(self.job.status, "needs_human")
        self.daemon.handle_exit(self.job, 1)
        self.assertEqual(self.job.status, "needs_human")

    def test_clean_exit_is_finished(self):
        self.job.process = FakeProcess(0)
        self.job.status = "running"
        self.daemon.handle_exit(self.job, 0)
        self.assertEqual(self.job.status, "finished")
        self.assertEqual(self.spawned, [])

    def test_describe_merges_bot_heartbeat(self):
        _merged, partial, _log = paths.user_paths("alice")
        with open(partial, "w") as handle:
            json.dump({"intention": "fishing shrimp", "position": [0, 3086, 3227], "hitpoints": 9}, handle)
        state = self.job.describe()
        self.assertEqual(state["intention"], "fishing shrimp")
        self.assertEqual(state["position"], [0, 3086, 3227])
        self.assertEqual(state["hitpoints"], 9)

    def test_read_events_parses_log_markers(self):
        _merged, _partial, log_path = paths.user_paths("alice")
        with open(log_path, "w") as handle:
            handle.write("noise\n@@EVENT {\"kind\": \"random_event\", \"event\": \"tower\", \"resolved\": false}\n")
        self.daemon.read_events(self.job)
        self.assertEqual(self.job.rescue, "rescue_down")
        self.daemon.read_events(self.job)
        self.assertEqual(self.job.rescue, "rescue_down")

    def test_quest_requirements_event_is_kept_on_the_job(self):
        _merged, _partial, log_path = paths.user_paths("alice")
        event = {
            "kind": "quest_requirements",
            "quest": "cook",
            "met": False,
            "reasons": [{"type": "item", "item": "egg", "required": 1, "owned": 0}],
        }
        with open(log_path, "w") as handle:
            handle.write("@@EVENT " + json.dumps(event) + "\n")
        self.daemon.read_events(self.job)
        self.assertEqual(self.job.last_requirements, event)
        self.assertEqual(self.job.describe()["requirements"], event)
        self.assertEqual(self.job.status, "starting")


if __name__ == "__main__":
    unittest.main()
