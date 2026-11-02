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
from collections import deque


import gamedata as knowledge
from lifescape import collision as collision_mod
from lifescape.client import Unreachable  # noqa: F401  (re-exported for callers)
from scripts.tutorial import common as tutorial_common

EVENT_HOSTS = ("macro_mage", "macro_geni", "macro_dwarf")

MAZE_X = (2880, 2944)
MAZE_Z = (4544, 4608)
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


def in_rect(x, z):
    return MAZE_X[0] <= x < MAZE_X[1] and MAZE_Z[0] <= z < MAZE_Z[1]


def loc_name(bot, loc_id):
    entry = bot.loctypes["loc"][loc_id] if loc_id < len(bot.loctypes["loc"]) else None
    return (entry["name"] if entry else "") or ""


def maze_statue(bot):
    """(x, z, loc_id) of the maze exit statue, from the static action table."""
    for (x, z, level), (loc_id, _shape, _angle) in (bot.world.static_actions or {}).items():
        if level == 0 and in_rect(x, z) and loc_name(bot, loc_id) in STATUE_NAMES:
            return x, z, loc_id
    return None


def maze_doors(bot):
    """{(x, z): placement} for every maze door the server may open.

    Server truth (macro_event_maze.rs2): the plain walllow and the safe1/
    safe4 variants answer the click; safe2/safe3 answer "I don't think
    that's the right way". Which variant sits where is map data, not a
    rule we can trust blind, so every door is a candidate and a refusal
    removes it at runtime.
    """
    found = {}
    for spot in bot.wall_doors or ():
        if spot[2] != 0 or not in_rect(spot[0], spot[1]):
            continue
        name = loc_name(bot, spot[9])
        if name.startswith(("macro_maze_walllow", "macor_maze_walllow")):
            found[(spot[0], spot[1])] = spot
    return found


def maze_statue_targets(bot):
    """Tiles from which the 3x3 exit statue can be touched."""
    statue = maze_statue(bot)
    if statue is None:
        return set()
    x, z, _loc_id = statue
    return {
        (x + dx, z + dz)
        for dx in range(-1, 4)
        for dz in range(-1, 4)
        if dx in (-1, 3) or dz in (-1, 3)
    }


def maze_path(bot, goals, doors, blocked):
    """Shortest tile path to any goal, crossing candidate doors.

    `blocked` holds the directed edges (from_tile, door_tile) the server
    already refused, so a refused approach is never retried from the same
    side.
    """
    start = (bot.world.x, bot.world.z)
    previous = {start: None}
    queue = deque([start])
    level = bot.world.level
    while queue:
        here = queue.popleft()
        if here in goals:
            path = []
            tile = here
            while tile is not None:
                path.append(tile)
                tile = previous[tile]
            path.reverse()
            return path
        x, z = here
        for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nxt = (x + dx, z + dz)
            if nxt in previous or not in_rect(nxt[0], nxt[1]):
                continue
            traversable = collision_mod.step_open(bot.collision, x, z, level, dx, dz)
            if not traversable and (here in doors or nxt in doors):
                # A door is walked like an open tile; the server decides
                # which approach side it accepts, learned from refusals.
                traversable = (here, nxt) not in blocked
            if traversable:
                previous[nxt] = here
                queue.append(nxt)
    return None


def cross_door_step(bot, spot, beyond):
    """Click a walllow, then take the single step it opens."""
    x, z = spot[0], spot[1]
    bot.op_loc(1, x, z, spot[9])
    bot.pump(3)
    if bot.world.modal is not None:
        bot.close_modal()
        bot.pump(3)
    try:
        return bot.step_to(beyond[0], beyond[1], timeout_ticks=120)
    except (Unreachable, TimeoutError) as err:
        print("walllow (%d,%d): step to %s failed: %s" % (x, z, beyond, err), flush=True)
        return False


def solve_maze(bot):
    """Walk to the maze centre one tile at a time, opening walllows.

    The maze is a static map of single-tile corridors behind walllow doors,
    so the route comes from the collision map and the walk is a sequence of
    single-tile steps: no routing, no door hunting, nothing that can wander
    off. A step the server refuses teaches the path to avoid that edge.
    Reaching the centre ends the event server-side. Returns True when out.
    """
    if not in_maze(bot):
        return True
    bot.ensure_nav()
    doors = maze_doors(bot)
    goals = maze_statue_targets(bot)
    if not goals:
        print("maze centre missing from the map table", flush=True)
        return False
    print("maze: %d doors, centre target %s" % (len(doors), sorted(goals)[:1]), flush=True)
    blocked = set()
    deadline = bot.world.ticks + 15000
    while in_maze(bot) and bot.world.ticks < deadline:
        if bot.world.modal is not None and bot.world.modal[0] == "mesbox":
            bot.close_modal()
            bot.pump(3)
        path = maze_path(bot, goals, doors, blocked)
        if path is None:
            print("maze: no route left from %s" % (bot.position,), flush=True)
            return False
        progressed = False
        for nxt in path[1:]:
            if not in_maze(bot):
                return True
            here = (bot.world.x, bot.world.z)
            if abs(nxt[0] - here[0]) + abs(nxt[1] - here[1]) != 1:
                break
            try:
                if bot.step_to(nxt[0], nxt[1], timeout_ticks=60):
                    progressed = True
                    continue
            except (Unreachable, TimeoutError):
                pass
            if nxt in doors and cross_door_step(bot, doors[nxt], nxt):
                progressed = True
                continue
            blocked.add((here, nxt))
            progressed = True
            break
        if not progressed:
            print("maze: stuck at %s" % (bot.position,), flush=True)
            return False
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


def cube_in_pack(bot):
    """True when a strange box is waiting to be solved in the inventory."""
    cube = knowledge.objs.by_name("macro_cube")
    return bool(cube) and bot.inventory().get(cube[0].obj_id, 0) > 0


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
    if cube_in_pack(bot):
        # The strange box is carried, not a zone: hand it to the rescue so
        # the botting script is not blocked behind a question interface.
        print("strange box in inventory, handing it to the cube rescue", flush=True)
        report(bot, "cube", False)
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
