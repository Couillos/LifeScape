import threading
import time
import unittest

from lifescape.client import Bot
from orchestration import session_worker
from orchestration.session_worker import (
    aggregate,
    chat_options,
    find_backpack,
    method_lists,
    parse_call_args,
    read_from_snapshot,
)


def snapshot(**overrides):
    base = {
        "inv": {3200: {0: (995, 100)}, 3201: {0: (526, 1)}},
        "stats": {0: (40, 1000), 3: (10, 100)},
        "texts": {100: "Select an Option", 101: "Climb up", 102: "Climb down"},
        "position": (0, 3200, 3200),
        "modal": None,
        "com_ids": {"bank_main:inv": 3201, "inventory:inv": 3200},
        "inv_cap": {3200: 28},
    }
    base.update(overrides)
    return base


class ParseCallArgsTest(unittest.TestCase):
    def test_json_values_parse(self):
        self.assertEqual(parse_call_args(["3200", "true", '"texte"', "[1,2]"]), [3200, True, "texte", [1, 2]])

    def test_plain_strings_stay_strings(self):
        self.assertEqual(parse_call_args(["hello", "3200x"]), ["hello", "3200x"])


class ReadFromSnapshotTest(unittest.TestCase):
    def test_inventory_aggregates_slots(self):
        self.assertEqual(read_from_snapshot(snapshot(), "inventory"), {995: 100, 526: 1})

    def test_bank_items_uses_bank_com(self):
        self.assertEqual(read_from_snapshot(snapshot(), "bank_items"), {526: 1})

    def test_backpack_finds_28_cap(self):
        pack = read_from_snapshot(snapshot(), "backpack")
        self.assertEqual(pack["com"], 3200)
        self.assertEqual(pack["items"], [{"slot": 0, "id": 995, "count": 100}])

    def test_chat_texts_sorted(self):
        texts = read_from_snapshot(snapshot(), "chat_texts")
        self.assertEqual(texts[0], (100, "Select an Option"))

    def test_chat_options_after_header(self):
        self.assertEqual(chat_options(snapshot()["texts"]), [[101, "Climb up"], [102, "Climb down"]])

    def test_position_is_a_list(self):
        self.assertEqual(read_from_snapshot(snapshot(), "position"), [0, 3200, 3200])

    def test_unknown_method_raises(self):
        with self.assertRaises(KeyError):
            read_from_snapshot(snapshot(), "walk_to")

    def test_empty_snapshot_raises(self):
        with self.assertRaises(KeyError):
            read_from_snapshot({}, "inventory")


class QueueDrainTest(unittest.TestCase):
    def setUp(self):
        self.bot = Bot()
        self.bot.echo = lambda *args: ("echo", list(args))

    def test_queued_call_runs_at_safe_point(self):
        command = self.bot.queue_command("echo", [1, "x"])
        self.assertFalse(command["done"].is_set())
        self.bot._drain_command()
        self.assertTrue(command["done"].is_set())
        self.assertEqual(command["result"], ("echo", [1, "x"]))
        self.assertIsNone(command["error"])

    def test_unknown_method_errors_without_raising(self):
        command = self.bot.queue_command("no_such_method", [])
        self.bot._drain_command()
        self.assertIn("unknown method", command["error"])

    def test_dunder_method_is_not_callable(self):
        command = self.bot.queue_command("__class__", [])
        self.bot._drain_command()
        self.assertIn("unknown method", command["error"])

    def test_failing_call_reports_type(self):
        def fail():
            raise ValueError("boom")

        self.bot.fail = fail
        command = self.bot.queue_command("fail", [])
        self.bot._drain_command()
        self.assertIn("ValueError: boom", command["error"])

    def test_cancelled_command_is_skipped(self):
        command = self.bot.queue_command("echo", [])
        self.bot.drop_command(command)
        self.bot._drain_command()
        self.assertTrue(command["done"].is_set())
        self.assertIsNone(command["result"])

    def test_one_command_per_drain(self):
        first = self.bot.queue_command("echo", [1])
        second = self.bot.queue_command("echo", [2])
        self.bot._drain_command()
        self.assertTrue(first["done"].is_set())
        self.assertFalse(second["done"].is_set())


class WaitCommandTest(unittest.TestCase):
    def setUp(self):
        self.worker = session_worker.SessionWorker("alice")
        self.worker.bot = Bot()

    def test_done_command_returns_fast(self):
        command = self.worker.bot.queue_command("echo", [])
        command["done"].set()
        self.assertTrue(self.worker.wait_command(command))

    def test_tick_bound_returns_busy(self):
        command = self.worker.bot.queue_command("echo", [])
        stop = threading.Event()

        def advance():
            while not stop.is_set() and self.worker.bot.world.ticks < 600:
                time.sleep(0.01)
                self.worker.bot.world.ticks += 10

        thread = threading.Thread(target=advance, daemon=True)
        thread.start()
        try:
            self.assertFalse(self.worker.wait_command(command))
        finally:
            stop.set()
            thread.join()
        self.assertGreaterEqual(self.worker.bot.world.ticks, 500)

    def test_stop_returns_busy(self):
        command = self.worker.bot.queue_command("echo", [])
        self.worker.bot.request_stop()
        try:
            self.assertFalse(self.worker.wait_command(command))
        finally:
            self.worker.bot.clear_stop()


class MethodListsTest(unittest.TestCase):
    def test_non_admin_has_no_cheat_login_close(self):
        reads, writes = method_lists(admin=False)
        for denied in ("login", "relogin", "close", "cheat", "snapshot", "queue_command"):
            self.assertNotIn(denied, writes)
        self.assertIn("walk_to", writes)
        self.assertIn("inventory", reads)

    def test_admin_keeps_cheat_only(self):
        _reads, writes = method_lists(admin=True)
        self.assertIn("cheat", writes)
        for denied in ("login", "relogin", "close"):
            self.assertNotIn(denied, writes)


if __name__ == "__main__":
    unittest.main()
