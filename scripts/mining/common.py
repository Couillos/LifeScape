"""Miner: mines the best F2P rock for the player's level, banks full
loads, sells ore to fund pickaxe upgrades, reattaches lost pickaxe
heads instead of rebuying.

Adapts to any start position: bank, rocks, general store and pickaxe
shop are all picked by proximity from gamedata (best rock for the
level, safe for the combat level, nearest usable bank). Banks across
the toll gate are a last resort (10 coins per crossing). Stays on one
level.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/mining/run.py <username> [loads]
  loads = full bank trips before stopping, 0 means forever (default 0).
"""

import math
import time


from scripts import hazards, randomevents
from scripts.hazards import LIVE_MARGIN, hitpoints, static_threats, threats

from lifescape import collision as collision_flags
from lifescape import doors as doors_module
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
MINING_STAT = 14
COINS = "coins"

PICK_ORDER = ("rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe")
# Wield/mining level reqs from content/scripts/skill_mining/configs/pickaxes.obj
# (param=levelrequire; obj dump carries no params, so the six values live here).
PICK_REQS = {"bronze_pickaxe": 0, "iron_pickaxe": 0, "steel_pickaxe": 6, "mithril_pickaxe": 21, "adamant_pickaxe": 31, "rune_pickaxe": 41}
PICK_IDS = {}
HANDLE_NAME = "macro_pickaxehandle"
