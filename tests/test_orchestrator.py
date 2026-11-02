import json
import os
import tempfile
import unittest

from orchestration import paths
from orchestration.daemon import AttachedProcess, Orchestrator
from orchestration.jobs import Job
import orchestration.daemon as daemon_mod


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
        self.saved = (paths.USERS_DIR, paths.LOGS_DIR, daemon_mod.USERS_DIR)
        paths.USERS_DIR = os.path.join(self.directory, "users")
        paths.LOGS_DIR = os.path.join(self.directory, "logs")
        daemon_mod.USERS_DIR = paths.USERS_DIR
        paths.ensure_dirs()
        self.daemon = Orchestrator()
        self.sent = []
        self.daemon.send_worker = lambda account, request, timeout=40: self.fake_worker(account, request)
        self.job = Job("alice")
        self.job.process = FakeProcess()
        self.daemon.jobs["alice"] = self.job

    def tearDown(self):
        paths.USERS_DIR, paths.LOGS_DIR, daemon_mod.USERS_DIR = self.saved
        self.daemon.running = False

    def fake_worker(self, account, request):
        self.sent.append((account, request))
        return {"ok": True}

    def test_worker_death_marks_disconnected_without_relaunch(self):
        self.job.process = FakeProcess(1)
        self.daemon.supervise_once(self.job)
        self.assertEqual(self.job.status, "disconnected")
        self.assertFalse(self.job.supervise)
        self.assertIsNone(self.job.process)
        self.assertIn("close + open", self.job.last_error["message"])
        self.assertEqual([cmd for _acc, cmd in self.sent], [])

    def test_resolved_event_does_not_rescue(self):
        self.job.phase = "running"
        self.job.script = "fishing"
        self.daemon.handle_event(self.job, {"kind": "random_event", "event": "maze", "resolved": True})
        self.assertIsNone(self.job.rescue_for)
        self.assertIsNone(self.job.interrupted)

    def test_unknown_command_is_an_error(self):
        reply = self.daemon.dispatch({"cmd": "start", "account": "alice"})
        self.assertFalse(reply["ok"])
        self.assertIn("unknown command", reply["error"])
        for dead in ("stop", "restart"):
            reply = self.daemon.dispatch({"cmd": dead, "account": "alice"})
            self.assertFalse(reply["ok"])

    def test_run_without_session_is_no_session(self):
        reply = self.daemon.dispatch({"cmd": "run", "account": "ghost", "script": "fishing"})
        self.assertFalse(reply["ok"])
        self.assertIn("no session", reply["error"])

    def test_call_without_session_is_no_session(self):
        reply = self.daemon.dispatch({"cmd": "call", "account": "ghost", "method": "inventory"})
        self.assertFalse(reply["ok"])
        self.assertIn("no session, open ghost first", reply["error"])

    def test_call_forwards_method_and_staff_flag(self):
        reply = self.daemon.dispatch({"cmd": "call", "account": "alice", "method": "inventory", "args": []})
        self.assertTrue(reply["ok"])
        account, request = self.sent[-1]
        self.assertEqual((account, request["method"], request["admin"]), ("alice", "inventory", False))

    def test_describe_merges_bot_heartbeat(self):
        _merged, partial, _log = paths.user_paths("alice")
        with open(partial, "w") as handle:
            json.dump({"intention": "fishing shrimp", "position": [0, 3086, 3227], "hitpoints": 9}, handle)
        state = self.job.describe()
        self.assertEqual(state["intention"], "fishing shrimp")
        self.assertEqual(state["position"], [0, 3086, 3227])
        self.assertEqual(state["hitpoints"], 9)
        self.assertIsNone(state["script"])
        self.assertIsNone(state["last_error"])

    def test_read_events_parses_log_markers(self):
        _merged, _partial, log_path = paths.user_paths("alice")
        with open(log_path, "w") as handle:
            handle.write("noise\n@@EVENT {\"kind\": \"random_event\", \"event\": \"tower\", \"resolved\": false}\n")
        self.job.phase = "running"
        self.job.script = "woodcut"
        self.daemon.read_events(self.job)
        self.assertEqual(self.job.last_event["event"], "tower")
        self.assertFalse(self.job.last_event["resolved"])

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
        self.assertEqual(self.job.last_requirements["quest"], "cook")
        self.assertEqual(self.job.describe()["requirements"]["quest"], "cook")

    def test_registry_scripts_all_expose_run(self):
        from orchestration import registry

        for name in sorted(registry.SCRIPTS):
            if name == "tests":
                continue
            run, _path = registry.load_run(name)
            self.assertTrue(callable(run), name)

    def test_registry_has_no_removed_helpers(self):
        from orchestration import registry

        for name in ("clean_session",):
            self.assertNotIn(name, registry.SCRIPTS)
        self.assertNotIn("REPAIR_SCRIPTS", dir(registry))

    def test_restore_adopts_sessions_idle_without_script(self):
        _merged, partial, log_path = paths.user_paths("bob")
        with open(partial, "w") as handle:
            json.dump({"intention": "idle"}, handle)
        with open(log_path, "w") as handle:
            handle.write("")
        with open(os.path.join(paths.USERS_DIR, "bob.json"), "w") as handle:
            json.dump({"account": "bob", "script": "fishing", "status": "running"}, handle)
        seen = []

        state = {"running": True}

        def fake_status(account, request, timeout=40):
            seen.append(request)
            if request["cmd"] == "unload":
                state["running"] = False
                return {"ok": True}
            running = state["running"]
            return {"ok": True, "session": {
                "account": "bob", "phase": "running" if running else "idle",
                "script": "fishing" if running else None,
                "script_args": ["1"] if running else [], "status": "running" if running else "idle",
                "session_pid": os.getpid(),
                "last_error": None, "last_event": None, "last_code": None, "ticks": 10,
            }}

        self.daemon.send_worker = fake_status

        import orchestration.daemon as daemon_mod

        original = daemon_mod.session_paths
        original_supervisor = self.daemon.start_supervisor
        daemon_mod.session_paths = lambda account: (os.path.join(self.directory, "sock-%s" % account), "")
        self.daemon.start_supervisor = lambda job: None
        try:
            with open(os.path.join(self.directory, "sock-bob"), "w") as handle:
                handle.write("")
            self.daemon.jobs.pop("bob", None)
            self.daemon.restore_jobs()
        finally:
            daemon_mod.session_paths = original
            self.daemon.start_supervisor = original_supervisor
        job = self.daemon.jobs.get("bob")
        self.assertIsNotNone(job)
        self.assertIsNone(job.script)
        self.assertEqual(job.phase, "idle")
        self.assertIsInstance(job.process, AttachedProcess)
        self.assertEqual([request["cmd"] for request in seen], ["status", "unload", "status"])

    def test_open_failure_leaves_no_process(self):
        import orchestration.daemon as daemon_mod

        class DeadProcess:
            pid = 1234

            def poll(self):
                return 1

            def terminate(self):
                pass

            def kill(self):
                pass

        original = daemon_mod.subprocess.Popen
        daemon_mod.subprocess.Popen = lambda *args, **kwargs: DeadProcess()
        try:
            job = Job("carol")
            result = self.daemon.spawn(job, no_login=True)
        finally:
            daemon_mod.subprocess.Popen = original
        self.assertFalse(result.get("ok"))
        self.assertIsNone(job.process)
        self.assertNotIn("carol", self.daemon.jobs)

    def test_supervisor_leaves_healthy_idle_session_alone(self):
        self.job.phase = "idle"
        self.job.script = None
        self.job.status = "idle"
        before = self.job.describe()

        def fake_status(account, request, timeout=40):
            self.sent.append((account, request))
            return {"ok": True, "session": {
                "account": "alice", "phase": "idle", "script": None,
                "script_args": [], "status": "idle", "session_pid": os.getpid(),
                "last_error": None, "last_event": None, "last_code": None, "ticks": 7,
            }}

        self.daemon.send_worker = fake_status
        self.daemon.supervise_once(self.job)
        self.assertEqual(self.job.describe(), before)
        self.assertEqual([request["cmd"] for _account, request in self.sent], ["status"])


if __name__ == "__main__":
    unittest.main()
