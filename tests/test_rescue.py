import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from scripts.randomevents import in_maze, in_mime


def bot_at(x, z):
    return SimpleNamespace(world=SimpleNamespace(x=x, z=z))


class RescueTest(unittest.TestCase):
    def test_maze_region(self):
        self.assertTrue(in_maze(bot_at(2892, 4586)))
        self.assertTrue(in_maze(bot_at(2911, 4575)))
        self.assertFalse(in_maze(bot_at(3222, 3222)))
        self.assertFalse(in_maze(bot_at(3091, 3243)))
        self.assertFalse(in_maze(bot_at(2880 - 1, 4575)))

    def test_mime_region(self):
        self.assertTrue(in_mime(bot_at(2008, 4764)))
        self.assertFalse(in_mime(bot_at(3222, 3222)))
        self.assertFalse(in_mime(bot_at(2892, 4586)))
