"""Fisher: fishes the best spot for the player's level, banks full
loads, sells fish to fund tool and bait upgrades.

Adapts to any start position: bank, spots, general store and tool shop
are all picked by proximity from gamedata (best spot for the level,
safe for the combat level, nearest usable bank). Banks across the toll
gate are a last resort. Stays on one level."""

import os
import math
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from scripts import hazards, randomevents
from scripts.hazards import LIVE_MARGIN, hitpoints, static_threats, threats

from lifescape import collision as collision_flags
from lifescape import spots as spots_module
from lifescape.client import Bot, Unreachable
from lifescape.waits import wait_backpack, wait_hitpoints, wait_ticks

import gamedata as knowledge

from scripts.tutorial import common as C

BACKPACK_SIZE = 28
BACKPACK_COM = 3214
TREK_LIMIT = 400
SPOT_GONE_BAN_TICKS = 250
FLEE_BAN_TICKS = 500
ATTACK_BAN_RADIUS = 12
STALL_TICKS = 1000
MAX_FLEES_PER_LOAD = 2
FISH_STAT = 10
COINS = "coins"
BAIT_MIN = 20
BAIT_WANT = 200
LIVE_RADIUS = 40
