"""Requirement check and completion: Dragon Slayer.

Guildmaster -> Oziach (maze key + map leads) -> Duke (shield) -> Ned
(hire) -> Melzar's maze (part 1) -> Oracle door + chest (part 3) ->
Wormbrain (part 2, 10000 coins) -> Klarense (buy ship, 2000 coins) ->
repair the hull (3 planks, 12 nails) -> join the map -> Ned (map) ->
sail to Crandor -> Elvarg with the anti-dragonbreath shield worn.

Needs from the bank (auto-withdrawn as quest requirements): 12000
coins, 3 planks, 12 nails, hammer, mind bomb, unfired bowl, lobster
pot, silk, anti-dragonbreath shield. Combat expected throughout
(Melzar, lesser demon, Elvarg); an admin-level account is assumed, so
no extra gearing or food runs are scripted.
"""



import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.client import Unreachable
from scripts.quests import solver
from scripts.quests.common import run as run_requirements
from scripts.tutorial import common as tutorial_common

import os

from lifescape import save as save_mod

QUEST = quest_requirements.by_key("dragon")

WORN_COM = knowledge.com_id("wornitems:wear")
DRAGONQUEST = knowledge.varp_id_by_name("dragonquest")
HIRED = knowledge.varp_id_by_name("dragon_ned_hired")
COMPLETE = 10  # ^dragon_complete

# Save-backed stages: quest varps are not transmitted, so gates consult
# the persisted stage too (same as BKF/shield). Item gates below cover
# the map pieces; varp gates cover talks and the ship.
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or ""
    if not account:
        return {}
    try:
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _varp(bot, name):
    varp = knowledge.varp_id_by_name(name)
    return max(bot.world.varps.get(varp, 0), _SAVED_VARPS.get(varp, 0))


def _count(bot, obj_id):
    return bot.inventory().get(obj_id, 0)


def _have(bot, name):
    return _count(bot, solver.obj_id(name)) > 0


def _wield(bot, item):
    """Wear an item through its Wield op; a missing item is a SolverError."""
    obj = solver.obj_id(item)
    worn = bot.world.inv.get(WORN_COM, {})
    if any(held == obj for held, _count in worn.values()):
        return
    try:
        com, slot = tutorial_common.inv_slot(bot, obj)
    except KeyError:
        raise solver.SolverError("no %s to wield at %s" % (item, bot.position))
    bot.op_held(2, obj, slot, com)
    target = bot.world.ticks + 30
    while bot.world.ticks < target:
        bot.pump(2)
        worn = bot.world.inv.get(WORN_COM, {})
        if any(held == obj for held, _count in worn.values()):
            return
    raise solver.SolverError("could not wield %s" % item)


def _try_op(bot, func, *args, **kwargs):
    """Best-effort loc op (ladders/gangplanks while routing the maze/ship)."""
    try:
        func(bot, *args, **kwargs)
        return True
    except (Unreachable, TimeoutError) as error:
        print("optional op %s failed: %s" % (args, error), flush=True)
        return False


def _maze_room(bot, npc, key, door):
    """Kill the room's monster, loot its key and unlock the next door."""
    if not _have(bot, key) and not _have(bot, "mappart1"):
        solver.attack(bot, (npc,), timeout=600)
        solver.take_near(bot, key, timeout=300)
    if _have(bot, key):
        solver.use_item_on_loc(bot, key, (door,))


def _combine_map(bot):
    if _have(bot, "dragonmap"):
        return
    for part in ("mappart1", "mappart2", "mappart3"):
        if not _have(bot, part):
            raise solver.SolverError("missing map piece %s at %s" % (part, bot.position))
    first = solver.obj_id("mappart1")
    second = solver.obj_id("mappart2")
    tutorial_common.use_held_on_held(bot, first, second)
    target = bot.world.ticks + 60
    while bot.world.ticks < target and not _have(bot, "dragonmap"):
        bot.pump(2)
    if not _have(bot, "dragonmap"):
        raise solver.SolverError("map pieces did not combine at %s" % (bot.position,))


def _repair_ship(bot):
    for _ in range(3):
        solver.use_item_on_loc(bot, "woodplank", ("shiphole",))


def _hired(bot):
    return max(bot.world.varps.get(HIRED, 0), _SAVED_VARPS.get(HIRED, 0)) >= 1


def solve(bot):
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    if _varp(bot, "dragonquest") == COMPLETE:
        return
    # One stage per run (logout persists the next): a drop mid-quest
    # would otherwise replay every talk from the guild. Item gates skip
    # finished loot; varp gates skip finished talks.
    # Champions' Guild -> Oziach: the guildmaster names him, Oziach hands
    # over the maze key and the three map leads (plus the shield lead).
    # Stage-gated: replays would drag the bot back across the map.
    if _varp(bot, "dragonquest") < 1:
        solver.talk_like(bot, "guildmaster", ["rune plate", "plate mail", "oziach"])
        return
    if _varp(bot, "dragonquest") < 2:
        # Stage north of the compound first, but only when far: nearby the
        # talk approach walks in directly, and staging would drag the bot
        # on a long detour for nothing.
        if abs(bot.world.x - 3069) + abs(bot.world.z - 3517) > 30:
            try:
                bot.walk_to(3069, 3522, 0, timeout=9000, open_doors=True)
            except (Unreachable, TimeoutError):
                pass
        solver.talk_like(bot, "oziach", ["rune plate", "sell", "guildmaster", "prove", "fun",
                                         "shield", "first", "second", "third", "dragon",
                                         "everything", "together", "fare"])
        return
    # Anti-dragonbreath shield from the Duke (needs Oziach's shield lead).
    if not _have(bot, "antidragonbreathshield"):
        solver.talk_like(bot, "duke_of_lumbridge", ["shield", "dragon", "breath"])
        return
    if not _have(bot, "antidragonbreathshield"):
        raise solver.SolverError("no anti-dragonbreath shield at %s" % (bot.position,))
    # Hire Ned in Draynor; he sails once the ship is repaired and mapped.
    if not _hired(bot):
        solver.talk_like(bot, "ned", ["sailor", "crandor", "take me", "map"])
        return
    # Melzar's maze: rat -> ghost -> skeleton, then down the ladder for
    # zombie -> Melzar -> lesser demon, then the chest holds part 1.
    # Stage at the maze door first (find_loc only sees 150 tiles).
    if not _have(bot, "mappart1"):
        try:
            bot.walk_to(2941, 3252, 0, timeout=9000, open_doors=True)
        except (Unreachable, TimeoutError):
            pass
        solver.use_item_on_loc(bot, "melzarkey", ("melzardoor",))
        _maze_room(bot, "dragonslayer_giantrat", "redkey", "reddoor")
        _maze_room(bot, "dragonslayer_ghost", "orangekey", "orangedoor")
        _maze_room(bot, "dragonslayer_skeleton", "yellowkey", "yellowdoor")
        _try_op(bot, solver.operate_loc, ("funladdertop",), 1, timeout=6000)
        _maze_room(bot, "dragonslayer_zombie", "bluekey", "bluedoor")
        _maze_room(bot, "melzar_the_mad", "magentakey", "magentadoor")
        _maze_room(bot, "dragonslayer_demon", "greenkey", "greendoor")
        solver.operate_loc(bot, ("funchestshut",), 1, timeout=6000)
        solver.operate_loc(bot, ("funchestopen",), 1, timeout=6000)
        return
    if not _have(bot, "mappart1"):
        raise solver.SolverError("no Melzar map piece at %s" % (bot.position,))
    # Oracle rhyme, then through the magic door (consumes the four items)
    # to the dwarven-mine chest holding part 3.
    if not _have(bot, "mappart3"):
        solver.talk_like(bot, "oracle", ["map", "crandor", "piece"])
        solver.operate_loc(bot, ("dragon_slayer_magic_door",), 1, timeout=6000)
        solver.operate_loc(bot, ("oraclechestshut",), 1, timeout=6000)
        solver.operate_loc(bot, ("oraclechestopen",), 1, timeout=6000)
        return
    if not _have(bot, "mappart3"):
        raise solver.SolverError("no oracle map piece at %s" % (bot.position,))
    # Wormbrain in the Port Sarim jail sells part 2 for 10000 coins.
    if not _have(bot, "mappart2"):
        solver.talk_like(bot, "wormbrain", ["piece of map", "pay", "alright", "10,000"])
        return
    if not _have(bot, "mappart2"):
        raise solver.SolverError("no Wormbrain map piece at %s" % (bot.position,))
    # Buy the Lady Lumbridge from Klarense, patch her three holes, then
    # join the map and hand it to Ned in Draynor.
    if _varp(bot, "dragonquest") < 3:
        solver.talk_like(bot, "klarense", ["buy", "yep", "sounds good"])
        return
    _try_op(bot, solver.operate_loc, ("dragonshipgangplank_on",), 1, timeout=6000)
    _try_op(bot, solver.operate_loc, ("dragonshipladdertop",), 1, timeout=6000)
    if _varp(bot, "dragonquestvar") < 3:
        _repair_ship(bot)
    _try_op(bot, solver.operate_loc, ("dragonshipladder",), 1, timeout=6000)
    _try_op(bot, solver.operate_loc, ("dragonshipgangplank_off",), 1, timeout=6000)
    _combine_map(bot)
    solver.talk_like(bot, "ned", ["crandor island", "take me", "map", "sailor"])
    # Board in Port Sarim, climb to Ned's cabin and sail to Crandor.
    solver.operate_loc(bot, ("dragonshipgangplank_on",), 1, timeout=6000)
    solver.operate_loc(bot, ("dragonshipladdertop",), 1, timeout=6000)
    solver.talk_like(bot, "dragonslayer_ned", ["yep", "ready", "go"])
    # Into the lair: rock opening, then the gates, then Elvarg himself.
    # The shield must be worn or the dragon fire (max 70) is unsurvivable.
    _wield(bot, "antidragonbreathshield")
    solver.operate_loc(bot, ("crandor_rock_opening",), 1, timeout=6000)
    _try_op(bot, solver.operate_loc, ("elvarg_gate_right", "elvarg_gate_left"), 1, timeout=6000)
    solver.attack(bot, ("elvarg",), timeout=1200)
    solver.wait_complete(bot)


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
