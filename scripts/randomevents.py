"""Shared random-event handling for all skill bots.

Call `check(bot)` once per work loop iteration (and `greet(bot)` from
inside long walks): it greets event hosts so they hand rewards instead
of punishing, and resolves teleporting events when it can. Returns True
when botting may continue, False when the bot must stop for a scripted
rescue (the worker unloads the script and the daemon runs the mapped
rescue or the fallback; see the printed event).

Covered:
- macro_mage / macro_geni: talking gives the gift/lamp and prevents
  both the maze/mime/box events and the fail-teleport for ignoring.
- macro_dwarf: talking gives food and prevents him turning aggressive.
- maze: ratcheted out via short legs to the centre statue on sight
  (walllow doors open one-sided, wallhigh never; refusals dismissed).
- fail-teleport towers: climbed down when reachable.

Not solved (clean abort instead): the mime performance, the strange
box dance, swarm/triffid aggression (the walk flee handles those).
"""

import json


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


def greet(bot, timeout=3000):
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
            bot.approach(npc["x"], npc["z"], radius=2, timeout=3000, open_doors=True)
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
    for (x, z, _level), loc in seen.items():
        # Dynamic locs vanish as None when the server removes them.
        if loc is None:
            continue
        loc_id = loc[0]
        if abs(x - bot.world.x) + abs(z - bot.world.z) > radius:
            continue
        entry = bot.loctypes["loc"][loc_id] if loc_id < len(bot.loctypes["loc"]) else None
        name = (entry["name"] if entry else "") or ""
        if needle in name.lower():
            found.append((abs(x - bot.world.x) + abs(z - bot.world.z), x, z, loc_id, name))
    found.sort()
    return found


def _touch_statue(bot, x, z, loc_id):
    """Walk to the exit statue and touch it. True when out of the maze."""
    try:
        bot.approach(x, z, radius=2, timeout=6000, open_doors=True)
    except (Unreachable, TimeoutError) as err:
        print("statue approach failed: %s" % (err,), flush=True)
        return False
    bot.op_loc(1, x, z, loc_id)
    target = bot.world.ticks + 200
    while bot.world.ticks < target and in_maze(bot):
        bot.pump(5)
    if not in_maze(bot):
        print("ok out of maze at %s" % (bot.position,), flush=True)
        return True
    return False


def solve_maze(bot):
    """Ratchet to the maze centre, touching the exit statue on sight.

    Walllow segments open from one side only (the planner clicks them
    and blocklists refusals); wallhigh segments never open. One direct
    walk stalls on the first wrong-side door, so advance in short legs
    and rescan for the statue after each: it streams in around 15
    tiles out, well before the centre tile itself is reachable.
    Returns True when outside the maze.
    """
    if not in_maze(bot):
        return True
    bot.ensure_nav()
    # Maze wallhigh segments never open (the server says "I can't open
    # that"), but they carry an Open op so the planner treats them as
    # doors and burns legs clicking them. Take them out of planning so
    # the router only threads the walllow one-way doors. Temporary like
    # any other door block (2 min), routing-only, expires on its own.
    # Batched: one rebuild, not one per wall (each rebuild copies maps).
    fresh = bot.world.ticks + 6000
    added = False
    for spot in bot.wall_doors or ():
        if spot[2] != bot.world.level:
            continue
        if not (MAZE_X[0] <= spot[0] < MAZE_X[1] and MAZE_Z[0] <= spot[1] < MAZE_Z[1]):
            continue
        entry = bot.loctypes["loc"][spot[9]] if spot[9] < len(bot.loctypes["loc"]) else None
        if entry is not None and entry.get("name") == "macro_maze_wallhigh":
            if id(spot) not in bot._blocked_doors:
                bot._blocked_doors.add(id(spot))
                added = True
            bot._blocked_doors_exp[id(spot)] = fresh
    if added:
        bot._rebuild_opened()
    deadline = bot.world.ticks + 15000
    best = abs(bot.world.x - MAZE_CENTRE[0]) + abs(bot.world.z - MAZE_CENTRE[1])
    stall = 0
    last = (bot.world.x, bot.world.z)
    while in_maze(bot) and bot.world.ticks < deadline:
        if bot.world.modal is not None:
            # Wrong-side door refusals land in a mesbox: dismiss it so
            # the rest of the leg stops clicking behind a modal.
            bot.close_modal()
            bot.pump(5)
        for _dist, x, z, loc_id, name in nearby_named_locs(bot, "maze"):
            if name in STATUE_NAMES:
                print("statue at (%d,%d)" % (x, z), flush=True)
                if _touch_statue(bot, x, z, loc_id):
                    return True
                break
        else:
            try:
                # Short legs: one door interaction per pass, then
                # dismiss any refusal modal before the next click.
                bot.walk_to(MAZE_CENTRE[0], MAZE_CENTRE[1], bot.world.level, timeout=400, open_doors=True)
            except (Unreachable, TimeoutError) as err:
                print("leg toward centre failed at %s: %s" % (bot.position, err), flush=True)
            bot.pump(5)
        here = (bot.world.x, bot.world.z)
        dist = abs(here[0] - MAZE_CENTRE[0]) + abs(here[1] - MAZE_CENTRE[1])
        if dist < best:
            # Real progress: the maze is threadable, allow more time.
            best = dist
            deadline = bot.world.ticks + 15000
        stall = stall + 1 if here == last else 0
        last = here
        if stall >= 3:
            # Door ping-pong: sidestep off the desire line so the next
            # replan tries a different set of doors.
            stall = 0
            detour = (here[0] + 4, here[1] - 4)
            print("sidestep to %s" % (detour,), flush=True)
            try:
                bot.walk_to(detour[0], detour[1], bot.world.level, timeout=600, open_doors=True)
            except (Unreachable, TimeoutError):
                pass
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
                bot.walk_to(x, z, bot.world.level, timeout=1500, open_doors=True)
                reached = True
                break
            except (Unreachable, TimeoutError):
                pass
            try:
                if not bot.open_nearby_door(bot.world.ticks + 3000):
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
