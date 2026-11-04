import threading
import time
import types
import unittest
import unittest.mock

from lifescape.client import Bot
from lifescape.client.core import StaleConnection, StopRequested
from lifescape.connection import Connection, LoginError
from orchestration import session_worker


class StubWorld:
    def __init__(self):
        self.ticks = 0
        self.position = (0, 3200, 3200)


class StubBot:
    """Minimal session surface: thread flags, queued commands, pump hook."""

    def __init__(self):
        self.world = StubWorld()
        self._stopped = False
        self._paused = False
        self._commands = []
        self._route_target = None
        self.pumps = 0
        self.pump_error = None
        self.relogin_error = None
        self.invalidated = 0
        self.closed = False
        self.close_error = None

    def request_stop(self):
        self._stopped = True

    def clear_stop(self):
        self._stopped = False

    def note_run_thread(self):
        pass

    def stop_requested(self):
        return self._stopped

    def set_paused(self, paused):
        self._paused = bool(paused)

    def paused(self):
        return self._paused

    def drop_command(self, command):
        command["cancelled"] = True
        if command in self._commands:
            self._commands.remove(command)

    def invalidate_sends(self, thread=None):
        self.invalidated += 1

    def pump(self, count=1):
        self.pumps += 1
        if self.pump_error is not None:
            raise self.pump_error

    def intend(self, _intention):
        pass

    def relogin(self):
        if self.relogin_error is not None:
            raise self.relogin_error

    def close(self):
        self.closed = True
        if self.close_error is not None:
            raise self.close_error


def make_worker(**overrides):
    worker = session_worker.SessionWorker("alice")
    worker.bot = StubBot()
    worker.connected = True
    for key, value in overrides.items():
        setattr(worker, key, value)
    return worker


class RunThreadTest(unittest.TestCase):
    def run_outcome(self, run_fn):
        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        worker.run_thread(run_fn, "fishing", ["1"])
        return worker

    def test_return_value_is_finished(self):
        worker = self.run_outcome(lambda bot, *args: None)
        self.assertEqual(worker.status, "finished")
        self.assertIsNone(worker.script)
        self.assertEqual(worker.phase, "idle")

    def test_nonzero_return_is_failed(self):
        worker = self.run_outcome(lambda bot, *args: 1)
        self.assertEqual(worker.status, "failed")
        self.assertIn("returned 1", worker.last_error["message"])

    def test_exception_keeps_session_idle(self):
        def fail(bot, *args):
            raise ValueError("boom")

        worker = self.run_outcome(fail)
        self.assertEqual(worker.status, "idle")
        self.assertIsNone(worker.script)
        self.assertEqual(worker.last_error["type"], "ValueError")

    def test_random_event_unloads_for_rescue(self):
        def abort(bot, *args):
            raise SystemExit("random event unresolved at (0,3200,3200)")

        worker = self.run_outcome(abort)
        self.assertEqual(worker.status, "idle")
        self.assertIsNone(worker.script)
        self.assertFalse(worker.last_event["resolved"])

    def test_stop_request_ends_quietly(self):
        def stop(bot, *args):
            raise StopRequested("unload")

        worker = self.run_outcome(stop)
        self.assertEqual(worker.status, "idle")
        self.assertIsNone(worker.last_error)

    def test_connection_loss_reconnects_session(self):
        def drop(bot, *args):
            raise ConnectionError("socket closed")

        worker = self.run_outcome(drop)
        self.assertEqual(worker.status, "idle")
        self.assertIsNone(worker.script)

    def test_connection_loss_without_relogin_is_disconnected(self):
        def drop(bot, *args):
            raise ConnectionError("socket closed")

        worker = make_worker()
        worker.bot.relogin_error = LoginError("refused")
        worker.phase = "running"
        worker.script = "fishing"
        worker.run_thread(drop, "fishing", [])
        self.assertEqual(worker.status, "disconnected")
        self.assertIn("relogin", worker.last_error["where"])


class UnloadTest(unittest.TestCase):
    def test_no_thread_is_already_idle(self):
        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        worker.status = "running"
        reply = worker.cmd_unload()
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["session"]["status"], "idle")
        self.assertEqual(worker.bot.pumps, 0)

    def test_stop_flag_ends_cooperative_thread(self):
        arrived = threading.Event()
        release = threading.Event()

        def slow(bot, *args):
            arrived.set()
            while not bot.stop_requested():
                time.sleep(0.01)

        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        thread = threading.Thread(target=worker.run_thread, args=(slow, "fishing", []))
        worker.script_thread = thread
        thread.start()
        self.assertTrue(arrived.wait(5))
        reply = worker.cmd_unload()
        thread.join(5)
        self.assertTrue(reply["ok"])
        self.assertFalse(thread.is_alive())
        self.assertIsNone(worker.script)
        self.assertEqual(worker.phase, "idle")
        self.assertIn(worker.status, ("idle", "finished"))

    def test_orphan_thread_gets_muted(self):
        old_ticks, old_backstop = session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP
        session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP = 5, 0.3
        try:
            worker = make_worker()
            worker.phase = "running"
            worker.script = "fishing"
            worker.status = "running"
            thread = threading.Thread(target=time.sleep, args=(30,), daemon=True)
            worker.script_thread = thread
            thread.start()
            reply = worker.cmd_unload()
        finally:
            session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP = old_ticks, old_backstop
        self.assertTrue(reply["ok"])
        self.assertEqual(worker.bot.invalidated, 1)
        # The orphan keeps the stop flag and its registration: the idle
        # pump stays out and no verify pump runs on the shared socket.
        self.assertEqual(worker.status, "busy")
        self.assertEqual(worker.script, "fishing")
        self.assertTrue(thread.is_alive())
        self.assertIs(worker.script_thread, thread)
        self.assertTrue(worker.bot.stop_requested())
        self.assertEqual(worker.bot.pumps, 0)

    def test_orphan_death_reports_idle_and_releases_stop(self):
        old_ticks, old_backstop = session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP
        session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP = 5, 0.3
        release = threading.Event()
        from lifescape.client.core import StopRequested

        def slow(bot, *args):
            release.wait(10)
            raise StopRequested("late stop")

        try:
            worker = make_worker()
            worker.phase = "running"
            worker.script = "fishing"
            worker.status = "running"
            thread = threading.Thread(target=worker.run_thread, args=(slow, "fishing", []))
            worker.script_thread = thread
            thread.start()
            reply = worker.cmd_unload()
            self.assertTrue(reply["ok"])
            self.assertEqual(worker.status, "busy")
            release.set()
            thread.join(5)
        finally:
            session_worker.UNLOAD_TICKS, session_worker.WALL_BACKSTOP = old_ticks, old_backstop
        self.assertFalse(thread.is_alive())
        self.assertIsNone(worker.script)
        self.assertEqual(worker.phase, "idle")
        self.assertEqual(worker.status, "idle")
        self.assertFalse(worker.bot.stop_requested())

    def test_stale_connection_escapes_script_relogin_handlers(self):
        # A muted orphan must die, not relogin and keep playing: fishing
        # and friends catch ConnectionError to relogin, so StaleConnection
        # must not be one (nor a plain Exception).
        self.assertNotIsInstance(StaleConnection("stale"), Exception)
        relogged = []

        def dab(bot, *args):
            try:
                raise StaleConnection("stale send from an unloaded script thread")
            except ConnectionError:
                relogged.append(True)
                bot.relogin()

        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        worker.run_thread(dab, "fishing", [])
        self.assertEqual(relogged, [])
        self.assertIsNone(worker.script)
        self.assertEqual(worker.status, "idle")

    def test_unload_reports_busy_while_stopping(self):
        arrived = threading.Event()
        release = threading.Event()

        def slow(bot, *args):
            arrived.set()
            release.wait(5)

        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        worker.status = "running"
        thread = threading.Thread(target=worker.run_thread, args=(slow, "fishing", []))
        worker.script_thread = thread
        thread.start()
        self.assertTrue(arrived.wait(5))
        seen = []

        original_describe = worker.describe

        def spy_describe():
            seen.append(worker.status)
            return original_describe()

        worker.describe = spy_describe
        unloading = threading.Thread(target=worker.cmd_unload)
        unloading.start()
        time.sleep(0.3)
        with worker._lock:
            during = worker.status
        release.set()
        unloading.join(5)
        thread.join(5)
        self.assertEqual(during, "busy")
        self.assertIn(worker.status, ("idle", "finished"))

    def test_failed_verify_pump_reconnects(self):
        arrived = threading.Event()

        def slow(bot, *args):
            arrived.set()
            while not bot.stop_requested():
                time.sleep(0.01)

        worker = make_worker()
        worker.phase = "running"
        worker.script = "fishing"
        worker.status = "running"
        worker.bot.pump_error = ConnectionError("gone")
        thread = threading.Thread(target=worker.run_thread, args=(slow, "fishing", []))
        worker.script_thread = thread
        thread.start()
        self.assertTrue(arrived.wait(5))
        reply = worker.cmd_unload()
        thread.join(5)
        self.assertTrue(reply["ok"])
        self.assertEqual(worker.status, "idle")


class OrphanMuteTest(unittest.TestCase):
    def test_send_raises_after_invalidate(self):
        bot = Bot()
        calls = []

        def fake_send(conn_self, op_id, payload=b""):
            calls.append(op_id)

        with unittest.mock.patch.object(Connection, "send", fake_send):
            bot.connection = types.SimpleNamespace(sock=None)
            bot._track_connection()
            bot.connection.send(1)
            bot.invalidate_sends(threading.current_thread())
            with self.assertRaises(StaleConnection):
                bot.connection.send(1)
            self.assertEqual(calls, [1])
            errors = []

            def other_thread():
                try:
                    bot.connection.send(1)
                except StaleConnection:
                    errors.append(1)

            thread = threading.Thread(target=other_thread)
            thread.start()
            thread.join(5)
            self.assertEqual(errors, [])
            self.assertEqual(calls, [1, 1])
            bot.note_run_thread()
            bot.connection.send(1)
            self.assertEqual(calls, [1, 1, 1])


class CloseTest(unittest.TestCase):
    def test_close_unloads_and_stops(self):
        worker = make_worker()
        worker.phase = "idle"
        reply = worker.cmd_close()
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["session"]["status"], "stopped")
        self.assertTrue(worker.bot.closed)
        worker.closing = False

    def test_failed_logout_still_closes(self):
        worker = make_worker()
        worker.bot.close_error = OSError("rst")
        reply = worker.cmd_close()
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["session"]["status"], "stopped")
        self.assertIsNotNone(worker.last_error)
        worker.closing = False


if __name__ == "__main__":
    unittest.main()
