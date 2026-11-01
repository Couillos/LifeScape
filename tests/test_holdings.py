import os
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from orchestration import holdings


class HoldingsScheduleTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.saved = (holdings.DB_PATH, holdings.LEGACY_JSON, holdings.SAVE_DIR)
        holdings.DB_PATH = os.path.join(self.directory, "holdings.sqlite")
        holdings.LEGACY_JSON = os.path.join(self.directory, "holdings.json")
        holdings.SAVE_DIR = os.path.join(self.directory, "saves")
        os.makedirs(holdings.SAVE_DIR)

    def tearDown(self):
        holdings.DB_PATH, holdings.LEGACY_JSON, holdings.SAVE_DIR = self.saved

    def _count(self):
        connection = sqlite3.connect(holdings.DB_PATH)
        try:
            return connection.execute("SELECT COUNT(*) FROM samples").fetchone()[0]
        finally:
            connection.close()

    def test_samples_at_start_then_only_after_interval(self):
        with mock.patch.object(holdings, "current_holdings", return_value={995: 7}):
            self.assertTrue(holdings.maybe_sample())  # no history yet
            self.assertFalse(holdings.maybe_sample())  # just sampled
            self.assertEqual(self._count(), 1)

            connection = sqlite3.connect(holdings.DB_PATH)
            connection.execute("UPDATE samples SET taken_at = taken_at - ?", (31 * 60,))
            connection.commit()
            connection.close()

            self.assertTrue(holdings.maybe_sample())  # 31 minutes later
            self.assertFalse(holdings.maybe_sample())
            self.assertEqual(self._count(), 2)

    def test_staff_and_private_accounts_are_excluded(self):
        from lifescape import save as save_mod

        saved_default = save_mod.default_save_dir
        save_mod.default_save_dir = lambda: holdings.SAVE_DIR
        try:
            for account, coins in (("admin", 1000), ("bite", 500), ("t01", 3)):
                with open(os.path.join(holdings.SAVE_DIR, account + ".sav"), "wb") as handle:
                    handle.write(b"")
            with mock.patch.object(holdings, "parse_save", side_effect=lambda path: {
                "inv": {995: 500 if "bite" in path else (1000 if "admin" in path else 3)},
                "worn": {},
                "bank": {},
            }):
                with mock.patch.object(holdings, "_live_holdings", return_value={}):
                    totals = holdings.current_holdings()
            self.assertEqual(totals, {995: 3})
        finally:
            save_mod.default_save_dir = saved_default

    def test_no_holdings_means_no_point(self):
        with mock.patch.object(holdings, "current_holdings", return_value={}):
            self.assertFalse(holdings.maybe_sample())
            self.assertEqual(self._count(), 0)

    def test_snapshot_series_aligns_with_samples(self):
        with mock.patch.object(holdings, "current_holdings", return_value={995: 7}):
            self.assertTrue(holdings.maybe_sample())
            connection = sqlite3.connect(holdings.DB_PATH)
            connection.execute("UPDATE samples SET taken_at = taken_at - ?", (31 * 60,))
            connection.commit()
            connection.close()
            self.assertTrue(holdings.maybe_sample())

        with mock.patch.object(holdings, "current_holdings", return_value={995: 5}):
            snapshot = holdings.snapshot()
        self.assertEqual(len(snapshot["samples"]), 2)
        self.assertEqual(len(snapshot["total"]), 2)
        coins = next(item for item in snapshot["items"] if item["id"] == 995)
        self.assertEqual(coins["series"], [7, 7])
        self.assertEqual(coins["count"], 5)


if __name__ == "__main__":
    unittest.main()
