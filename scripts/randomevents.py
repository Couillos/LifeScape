"""Shared random-event handling for all skill bots.

Call `check(bot)` once per work loop iteration (and `greet(bot)` from
inside long walks): it greets event hosts so they hand rewards instead
of punishing, and resolves teleporting events when it can. Returns True
when botting may continue, False when the bot must stop for a human
(see the printed reason).

Covered:
- macro_mage / macro_geni: talking gives the gift/lamp and prevents
  both the maze/mime/box events and the fail-teleport for ignoring.
- macro_dwarf: talking gives food and prevents him turning aggressive.
- maze: walked out via the centre statue when visible.
- fail-teleport towers: climbed down when reachable.

Not solved (clean abort instead): the mime performance, the strange
box dance, swarm/triffid aggression (the walk flee handles those).
shortcut: the maze statue and tower ladders are unreachable here
because our client gets no zone stream and wrong collision that far
out; a solver needs that fixed first.
"""

import json
import time


from lifescape.client import Unreachable  # noqa: F401  (re-exported for callers)
from scripts.tutorial import common as tutorial_common

EVENT_HOSTS = ("macro_mage", "macro_geni", "macro_dwarf")

MAZE_X = (2880, 2944)
MAZE_Z = (4544, 4608)
MAZE_CENTRE = (2911, 4575)
STATUE_NAMES = ("macro_maze_complete",)

# Mime stage mapzone 0_31_74.
MIME_X = (1984, 2048)
MIME_Z = (4736, 4800)


def in_maze(bot):
    return MAZE_X[0] <= bot.world.x < MAZE_X[1] and MAZE_Z[0] <= bot.world.z < MAZE_Z[1]


def in_mime(bot):
    return MIME_X[0] <= bot.world.x < MIME_X[1] and MIME_Z[0] <= bot.world.z < MIME_Z[1]


def greet(bot, timeout=60):
    """Talk to event hosts on sight. Their talk paths ask nothing."""
    for name in EVENT_HOSTS:
        try:
            slot = bot.find_npc(name)
        except Exception:
            continue
        if slot is None:
            continue
        npc = bot.world.npcs.get(slot)
        if npc is None:
            continue
        print("greeting event host %s" % name, flush=True)
        try:
            bot.approach(npc["x"], npc["z"], radius=2, timeout=60, open_doors=True)
            slot = bot.find_npc(name)
            if slot is None:
                continue
            bot.op_npc(1, slot)
        except (Unreachable, TimeoutError):
            continue
        except Exception:
            continue
        tutorial_common.finish_dialog(bot, (), timeout=timeout)


def nearby_named_locs(bot, needle, radius=60):
    found = []
    seen = {}
    seen.update(bot.world.locs)
    seen.update(bot.world.static_locs)
    for (x, z, _level), (loc_id, _shape, _angle) in seen.items():
        if abs(x - bot.world.x) + abs(z - bot.world.z) > radius:
            continue
        entry = bot.loctypes["loc"][loc_id] if loc_id < len(bot.loctypes["loc"]) else None
        name = (entry["name"] if entry else "") or ""
        if needle in name.lower():
            found.append((abs(x - bot.world.x) + abs(z - bot.world.z), x, z, loc_id, name))
    found.sort()
    return found


def solve_maze(bot):
    """Try to leave the maze. Returns True when outside it."""
    if not in_maze(bot):
        return True
    bot.ensure_nav()
    deadline = bot.world.ticks + 1500
    while in_maze(bot) and bot.world.ticks < deadline:
        for _dist, x, z, loc_id, name in nearby_named_locs(bot, "maze"):
            if name in STATUE_NAMES:
                print("statue at (%d,%d)" % (x, z), flush=True)
                try:
                    bot.approach(x, z, radius=2, timeout=120, open_doors=True)
                except (Unreachable, TimeoutError) as err:
                    print("approach failed: %s" % (err,), flush=True)
                    break
                bot.op_loc(1, x, z, loc_id)
                target = bot.world.ticks + 50
                while bot.world.ticks < target and in_maze(bot):
                    bot.pump(5)
                if not in_maze(bot):
                    print("ok out of maze at %s" % (bot.position,), flush=True)
                    return True
                break
        try:
            bot.walk_to(MAZE_CENTRE[0], MAZE_CENTRE[1], bot.world.level, timeout=60, open_doors=True)
        except (Unreachable, TimeoutError) as err:
            print("step toward centre failed at %s: %s" % (bot.position, err), flush=True)
            bot.pump(10)
    return not in_maze(bot)


def climb_out(bot):
    """Climb down ladders until back on level 0. Returns True there."""
    for _attempt in range(6):
        if bot.world.level == 0:
            print("ok on ground at %s" % (bot.position,), flush=True)
            return True
        try:
            x, z, _level, loc = tutorial_common.find_loc(bot, "ladder", "stair")
        except Unreachable:
            print("no ladder or stair near %s" % (bot.position,), flush=True)
            return False
        print("ladder at (%d,%d)" % (x, z), flush=True)
        reached = False
        for _round in range(3):
            try:
                bot.walk_to(x, z, bot.world.level, timeout=30, open_doors=True)
                reached = True
                break
            except (Unreachable, TimeoutError):
                pass
            try:
                if not bot.open_nearby_door(time.time() + 60):
                    pass
            except Exception:
                pass
            try:
                bot.approach(x, z, radius=3, open_doors=True)
                reached = True
                break
            except (Unreachable, TimeoutError):
                continue
        if not reached:
            print("ladder unreachable", flush=True)
            return False
        before = bot.world.level
        if bot.ride_transition_at(x, z, before):
            print("now %s" % (bot.position,), flush=True)
            continue
        bot.op_loc(1, x, z, loc)
        target = bot.world.ticks + 50
        while bot.world.ticks < target and bot.world.level == before:
            bot.pump(5)
        print("now %s" % (bot.position,), flush=True)
    return bot.world.level == 0


def report(bot, event, resolved, detail=None):
    """Tell the orchestrator (log event) what a random event needs."""
    payload = {"kind": "random_event", "event": event, "resolved": bool(resolved), "at": bot.position}
    if detail:
        payload["detail"] = detail
    print("@@EVENT " + json.dumps(payload), flush=True)


def check(bot):
    """Shared per-iteration event check. True means keep botting."""
    greet(bot)
    if in_mime(bot):
        print("in mime event at %s, needs a performer" % (bot.position,), flush=True)
        report(bot, "mime", False)
        return False
    if in_maze(bot):
        solved = solve_maze(bot)
        report(bot, "maze", solved)
        return solved
    if bot.world.level != 0:
        escaped = climb_out(bot)
        report(bot, "tower", escaped)
        return escaped
    return True
