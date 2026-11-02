"""Woodcutter: chops the best affordable trees for the player's level,
banks full loads, sells logs to fund axe upgrades, reattaches
lost axe heads instead of rebuying.

Adapts to any start position: bank, trees, general store and axe shop
are all picked by proximity from gamedata (best spot for the level,
safe for the combat level, nearest usable bank). Banks across the
toll gate are a last resort (10 coins per crossing). Stays on one level.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/woodcut/run.py <username> [loads]
  loads = full bank trips before stopping, 0 means forever (default 0).
"""

import math
import time


from scripts import hazards, randomevents
from scripts.hazards import LIVE_MARGIN, hitpoints, static_threats, threats

from lifescape import spots as spots_module
from lifescape.client import Bot, Unreachable
from lifescape.waits import ban_area, wait_backpack, wait_hitpoints, wait_ticks

import gamedata as knowledge

from scripts.tutorial import common as C

BACKPACK_SIZE = 28
BACKPACK_COM = 3214
NEAR_BANK_TILES = 150
TREK_LIMIT = 400
ROUND_IDLE_TICKS = 12
SILENT_ROUNDS_TOLERATED = 3
DEPLETE_BAN_TICKS = 250
FLEE_BAN_TICKS = 500
ATTACK_BAN_RADIUS = 12
STALL_TICKS = 1000
MAX_FLEES_PER_LOAD = 2
WC_STAT = 8
COINS = "coins"

AXE_ORDER = ("rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe")
AXE_IDS = {}
HANDLE_NAME = "macro_hatchethandle"
