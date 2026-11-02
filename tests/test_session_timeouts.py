import inspect
import os
import time
import unittest
import unittest.mock

from lifescape import waits
from lifescape.client import Bot
from lifescape.client import core as core_mod
from lifescape.client import movement as movement_mod
from lifescape.connection import LoginError


def default_of(method, name):
    return inspect.signature(method).parameters[name].default


class TickDefaultsTest(unittest.TestCase):
    def test_movement_bounds_are_ticks(self):
        self.assertEqual(default_of(Bot.walk_to, "timeout"), 9000)
        self.assertEqual(default_of(Bot.approach, "timeout"), 3000)
        self.assertEqual(default_of(Bot.open_door, "timeout"), 1250)
        self.assertEqual(default_of(Bot.open_doors_near, "timeout"), 3000)
        self.assertEqual(default_of(Bot.stand_on_door_side, "timeout"), 500)

    def test_action_bounds_are_ticks(self):
        self.assertEqual(default_of(Bot.take, "timeout"), 1500)
        self.assertEqual(default_of(Bot.drop, "timeout"), 750)

    def test_constants(self):
        self.assertEqual(movement_mod.DOOR_WAIT, 35)
        self.assertEqual(core_mod.SAFE_POINT_TICKS, 500)
        self.assertEqual(core_mod.UNLOAD_TICKS, 250)
        self.assertEqual(waits.TICK_MS, 20)


class LoginAttemptsTest(unittest.TestCase):
    def test_busy_session_retries_then_succeeds(self):
        bot = Bot()
        calls = []

        class FakeConnection:
            host = "127.0.0.1"
            port = 43594

            def login(self, *args):
                calls.append(args)
                if len(calls) < 3:
                    raise LoginError("login refused: 5 (already logged in)")
                return {"staff": 0}

            def close(self):
                pass

        bot.connection = FakeConnection()
        bot._new_connection = lambda host, port: None  # no socket in unit test
        with unittest.mock.patch("time.sleep") as slept:
            bot._login_with_retry({"pack_dir": "", "server_key": ""}, attempts=15, spacing=2)
        self.assertEqual(len(calls), 3)
        self.assertEqual(slept.call_count, 2)

    def test_other_refusals_raise_at_once(self):
        bot = Bot()

        class FakeConnection:
            def login(self, *args):
                raise LoginError("login refused: 1 (invalid credentials)")

            def close(self):
                pass

        bot.connection = FakeConnection()
        with self.assertRaises(LoginError):
            bot._login_with_retry({"pack_dir": "", "server_key": ""})

    def test_relogin_is_nine_attempts(self):
        bot = Bot()
        calls = []

        def fail(*args, **kwargs):
            calls.append(1)
            raise ConnectionError("down")

        bot._reset_session = lambda: None
        bot.login = fail
        with unittest.mock.patch("time.sleep"):
            with self.assertRaises(ConnectionError):
                bot.relogin()
        self.assertEqual(len(calls), 9)

    def test_keepalive_due_covers_frozen_ticks(self):
        bot = Bot()
        bot.world.ticks = 100
        bot._last_send_tick = 100  # nothing sent for 0 ticks...

        class FakeConnection:
            sock = True
            outgoing = True
            last_send = time.time() - 2.0  # ...but 2 s of wall clock

        bot.connection = FakeConnection()
        self.assertTrue(bot._keepalive_due())
        bot.connection.last_send = time.time()
        self.assertFalse(bot._keepalive_due())

    def test_reset_session_clears_send_baseline(self):
        """A relogin restarts world ticks at 0: the keepalive baseline must
        follow, or no keepalive is ever sent again and the session drops."""
        bot = Bot()
        bot._last_send_tick = 600
        closed = []
        created = []

        class FakeConnection:
            host = "127.0.0.1"
            port = 43594

            def close(self):
                closed.append(1)

        bot.connection = FakeConnection()
        bot._new_connection = lambda host, port: created.append((host, port))
        bot._reset_session()
        self.assertEqual(bot._last_send_tick, 0)
        self.assertEqual(bot.world.ticks, 0)
        self.assertEqual(closed, [1])


class DerogationsTest(unittest.TestCase):
    def test_open_transition_already_ticks(self):
        source = inspect.getsource(Bot.open_transition)
        self.assertIn("self.world.ticks + 20", source)
        self.assertNotIn("time.time", source)

    def test_await_teleport_stays_wall_clock(self):
        self.assertIn("seconds", inspect.signature(Bot.await_teleport).parameters)
        self.assertIn("time.time()", inspect.getsource(Bot.await_teleport))

    def test_await_teleport_arrives_on_frozen_ticks(self):
        """A teleport arrival must end the wait even when ticks stall."""
        bot = _FrozenBot(land_after=3)
        self.assertTrue(Bot.await_teleport(bot, {"name": "stairs"}, (0, 10, 0), seconds=2.0, quiet=True))
        self.assertGreater(bot.pumps, 0)

    def test_await_teleport_times_out_on_wall_clock(self):
        """No arrival bounds the wait in seconds, not in frozen ticks."""
        bot = _FrozenBot(land_after=None)
        started = time.time()
        self.assertFalse(Bot.await_teleport(bot, {"name": "stairs"}, (0, 10, 0), seconds=0.2, quiet=True))
        self.assertLess(time.time() - started, 2.0)


class _FrozenWorld:
    ticks = 0
    level, x, z = 0, 0, 0

    @property
    def position(self):
        return (self.level, self.x, self.z)


class _FrozenBot:
    """Ticks never advance (stalled teleport round-trip); pumps do."""

    def __init__(self, land_after):
        self.world = _FrozenWorld()
        self.pumps = 0
        self.land_after = land_after

    def pump(self, count=1):
        self.pumps += 1
        if self.land_after is not None and self.pumps >= self.land_after:
            self.world.x = 10


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Every wall-clock use left in game code, with its documented reason.
ALLOWED_TIME_USES = [
    # socket/teardown guards (connection half-closed, not game pacing)
    ("lifescape/connection.py", "self.last_send = time.time()"),
    ("lifescape/connection.py", "deadline = time.time() + self.timeout"),
    ("lifescape/connection.py", "if time.time() > deadline:"),
    ("lifescape/client/core.py", "time.sleep(spacing)"),
    ("lifescape/client/core.py", "self._state_written = time.time()"),
    ("lifescape/client/core.py", '"updated": time.time(),'),
    ("lifescape/client/core.py", "deadline = time.time() + timeout"),
    ("lifescape/client/core.py", "while time.time() < deadline:"),
    ("lifescape/client/core.py", "time.time() - connection.last_send"),
    # round-trip derogation: ticks can stall mid-teleport
    ("lifescape/client/movement.py", "deadline = time.time() + seconds"),
    ("lifescape/client/movement.py", "while time.time() < deadline:"),
    # human-readable durations, never bounds
    ("scripts/check_doors.py", "started = time.time()"),
    ("scripts/check_doors.py", '"seconds": round(time.time() - started, 1),'),
    ("scripts/check_transitions.py", "started = time.time()"),
    ("scripts/check_transitions.py", '"seconds": round(time.time() - started, 1),'),
    ("scripts/fishing/run.py", "walked_at = time.time()"),
    ("scripts/fishing/run.py", '"adjacent in %.1fs" % (time.time() - walked_at)'),
    ("scripts/mining/run.py", "walked_at = time.time()"),
    ("scripts/mining/run.py", '"adjacent in %.1fs" % (time.time() - walked_at)'),
    ("scripts/woodcut/run.py", "walked_at = time.time()"),
    ("scripts/woodcut/run.py", '"adjacent in %.1fs" % (time.time() - walked_at)'),
    ("scripts/probe_death.py", "wall_start = time.time()"),
    ("scripts/probe_death.py", "time.time() - wall_start"),
    ("scripts/probe_respawn.py", "stump_at = time.time()"),
    ("scripts/probe_respawn.py", "time.time() - stump_at"),
    ("scripts/probe_respawn.py", "start_mark[0] = time.time()"),
    ("scripts/tutorial/common.py", '"when": time.time()'),
    # process-side guards outside any connection
    ("scripts/make_accounts.py", "timeout=30 + 10 * len(passwords)"),
    ("scripts/make_accounts.py", "sqlite3.connect(DB_PATH, timeout=30)"),
    ("scripts/make_accounts.py", "time.sleep(2)"),
    ("scripts/make_accounts.py", "deadline = time.time() + 120"),
    ("scripts/make_accounts.py", "while time.time() < deadline:"),
    ("scripts/probe_login.py", "timeout=10"),
]


class TimeRatissageTest(unittest.TestCase):
    def test_every_wall_clock_use_is_classified(self):
        markers = {}
        for path, marker in ALLOWED_TIME_USES:
            markers.setdefault(path, []).append(marker)
        offenders = []
        for directory in ("lifescape", "scripts"):
            for base, _dirs, files in os.walk(os.path.join(ROOT, directory)):
                for name in sorted(files):
                    if not name.endswith(".py"):
                        continue
                    path = os.path.join(base, name)
                    rel = os.path.relpath(path, ROOT)
                    with open(path) as handle:
                        for lineno, line in enumerate(handle, 1):
                            if "time.time()" in line or "time.sleep(" in line:
                                if not any(marker in line for marker in markers.get(rel, [])):
                                    offenders.append("%s:%d: %s" % (rel, lineno, line.strip()))
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
