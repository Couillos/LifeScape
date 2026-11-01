import os
import random
import tempfile
import unittest

from lifescape.world import to_base37

from orchestration import save_reader
from scripts import make_accounts

VALID_CHARS = set("abcdefghijklmnopqrstuvwxyz0123456789_")


class MakeAccountsTest(unittest.TestCase):
    def test_build_username_stays_short_and_clean(self):
        self.assertEqual(make_accounts.build_username("christopher", "holmes"), "christopher")
        self.assertEqual(make_accounts.build_username("amy", "fox"), "amy_fox")
        for first in make_accounts.FIRST_NAMES:
            for last in ("smith", "holmes"):
                candidate = make_accounts.build_username(first, last)
                self.assertTrue(len(candidate) <= 12, candidate)
                self.assertFalse(candidate.endswith("_"), candidate)

    def test_random_usernames_are_valid_and_unique(self):
        taken_names = {"bite", "t01"}
        taken_identities = set(to_base37(name) for name in taken_names)
        original_identities = set(taken_identities)
        rng = random.Random(1234)
        fresh = []
        for _draw in range(300):
            username = make_accounts.random_username(taken_names, taken_identities, rng)
            taken_names.add(username)
            taken_identities.add(to_base37(username))
            fresh.append(username)
        for username in fresh:
            self.assertTrue(3 <= len(username) <= 12, username)
            self.assertTrue(set(username) <= VALID_CHARS, username)
            self.assertFalse(username.startswith("_"), username)
            self.assertFalse(username.endswith("_"), username)
            taken_names.add(username)
        self.assertEqual(len(set(fresh)), len(fresh))
        identities = [to_base37(username) for username in fresh]
        self.assertEqual(len(set(identities)), len(identities))
        self.assertFalse(set(identities) & original_identities)

    def test_look_tables_match_server_idk_dump(self):
        # Enabled ids per slot from the IdkType dump (demon kits excluded).
        expected_male = [{0, 1, 2, 3, 4, 5, 6, 7, 8}, set(range(10, 18)), set(range(18, 26)),
                         set(range(26, 32)), {33, 34}, set(range(36, 41)), {42, 43}]
        expected_female = [set(range(45, 55)), {-1}, set(range(56, 61)), set(range(61, 66)),
                           {67, 68}, set(range(70, 78)), {79, 80}]
        self.assertEqual([set(slot) for slot in make_accounts.MALE_KITS], expected_male)
        self.assertEqual([set(slot) for slot in make_accounts.FEMALE_KITS], expected_female)

    def test_random_looks_use_legal_kits(self):
        rng = random.Random(99)
        seen_genders = set()
        for _draw in range(200):
            gender, kits, colors = make_accounts.random_look(rng)
            seen_genders.add(gender)
            table = make_accounts.MALE_KITS if gender == 0 else make_accounts.FEMALE_KITS
            self.assertEqual(len(kits), 7)
            self.assertEqual(len(colors), 5)
            for kit, slot in zip(kits, table):
                self.assertIn(kit, list(slot))
            for color, count in zip(colors, make_accounts.COLOR_COUNTS):
                self.assertTrue(0 <= color < count)
        self.assertEqual(seen_genders, {0, 1})
        males = sum(1 for _draw in range(2000) if make_accounts.random_look(rng)[0] == 0)
        self.assertTrue(1400 <= males <= 1600, males)

    def test_read_saved_look_roundtrip(self):
        body = bytes([3, 14, 20, 27, 33, 38, 255])
        payload = (
            (0x2004).to_bytes(2, "big") + (7).to_bytes(2, "big")
            + (3095).to_bytes(2, "big") + (3107).to_bytes(2, "big") + bytes([0])
            + body + bytes([5, 9, 2, 1, 7, 1]) + bytes(30)
        )
        handle = tempfile.NamedTemporaryFile(delete=False)
        try:
            handle.write(payload)
            handle.close()
            self.assertEqual(
                save_reader.look_path(handle.name),
                (1, [3, 14, 20, 27, 33, 38, -1], [5, 9, 2, 1, 7]),
            )
        finally:
            os.unlink(handle.name)

    def test_read_saved_look_rejects_garbage(self):
        self.assertIsNone(save_reader.look_path("/no/such/file.sav"))
        handle = tempfile.NamedTemporaryFile(delete=False)
        try:
            handle.write(b"\x20\x04\x00")
            handle.close()
            self.assertIsNone(save_reader.look_path(handle.name))
        finally:
            os.unlink(handle.name)


if __name__ == "__main__":
    unittest.main()
