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


def _touch_statue(bot, x, z, loc_id):
    """Walk to the exit statue and touch it. True when out of the maze."""
    if abs(bot.world.x - x) + abs(bot.world.z - z) > 2:
        try:
            bot.approach(x, z, radius=3, timeout=3000, open_doors=False)
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


def cross_maze_door(bot, spot, near, beyond):
    """Get onto a walllow, click it, then walk out through the opened face.

    Verified live: the click is accepted from the corridor side or from the
    door tile itself, and is refused from the along-wall side, so the door
    is entered first when the plain step fails. Returns True when the bot
    reached `beyond`.
    """
    x, z = spot[0], spot[1]
    door_tile = (x, z)
    here = (bot.world.x, bot.world.z)
    if here != door_tile:
        try:
            bot.walk_to(near[0], near[1], 0, timeout=300, open_doors=False)
        except (Unreachable, TimeoutError) as err:
            print("cannot reach %s before walllow %s: %s" % (near, door_tile, err), flush=True)
            return False
        if (bot.world.x, bot.world.z) != door_tile:
            try:
                bot.walk_to(door_tile[0], door_tile[1], 0, timeout=300, open_doors=False)
            except (Unreachable, TimeoutError):
                pass
    seen_messages = len(bot.world.messages)
    print("open walllow (%d,%d) at %s then step to %s" % (x, z, bot.position, beyond), flush=True)
    bot.op_loc(1, x, z, spot[9])
    ticks = bot.world.ticks + 60
    while bot.world.ticks < ticks:
        bot.pump(3)
        if (bot.world.x, bot.world.z) in (door_tile, beyond):
            break
    # The click either opens the face or answers with a box; the only
    # reliable proof is the server moving us, so judge on the tile reached.
    if bot.world.modal is not None:
        bot.close_modal()
        bot.pump(3)
    if (bot.world.x, bot.world.z) != door_tile:
        try:
            bot.step_to(door_tile[0], door_tile[1])
        except (Unreachable, TimeoutError) as err:
            print("walllow (%d,%d) not entered: %s" % (x, z, err), flush=True)
    if (bot.world.x, bot.world.z) == door_tile:
        try:
            bot.step_to(beyond[0], beyond[1])
        except (Unreachable, TimeoutError) as err:
            print("walllow (%d,%d) open but %s failed: %s" % (x, z, beyond, err), flush=True)
    if (bot.world.x, bot.world.z) != beyond:
        print("walllow (%d,%d) did not yield %s (at %s)%s" % (
            x, z, beyond, bot.position,
            ", server said: " + refusal_text(bot, seen_messages) if refusal_since(bot, seen_messages) else "",
        ), flush=True)
        return False
    return True


def refusal_text(bot, first):
    for line in (bot.world.messages or [])[first:]:
        text = str(getattr(line, "text", line))
        if "right way" in text.lower():
            return text
    return ""

def refusal_since(bot, first):
    """True when the server answered a door click with its wrong-side box."""
    return bool(refusal_text(bot, first))


def solve_maze(bot):
    """Walk to the maze centre through the doors the server lets us open.

    The maze map is static, so the geometry and the statue come from the
    collision map; only the walllow clicks are live. Each leg crosses one
    door, walked through right after the click while it stands open. A
    refused approach is remembered as a directed edge and never retried.
    Returns True when outside the maze.
    """
    if not in_maze(bot):
        return True
    bot.ensure_nav()
    statue = maze_statue(bot)
    if statue is None:
        print("maze exit statue missing from the static action table", flush=True)
        return False
    deadline = bot.world.ticks + 15000
    doors = maze_doors(bot)
    goals = maze_statue_targets(bot)
    print("maze doors: %d candidates, statue at (%d,%d)" % (len(doors), statue[0], statue[1]), flush=True)
    blocked = set()
    while in_maze(bot) and bot.world.ticks < deadline:
        if bot.world.modal is not None and bot.world.modal[0] == "mesbox":
            bot.close_modal()
            bot.pump(5)
        if abs(bot.world.x - statue[0]) + abs(bot.world.z - statue[1]) <= 2:
            print("statue in reach from %s" % (bot.position,), flush=True)
            if _touch_statue(bot, *statue):
                return True
            return False
        path = maze_path(bot, goals, doors, blocked)
        if path is None:
            print("no path through the open doors at %s" % (bot.position,), flush=True)
            return False
        index = next((i for i, tile in enumerate(path) if tile in doors), None)
        if index is None:
            goal = path[-1]
            try:
                bot.walk_to(goal[0], goal[1], 0, timeout=600, open_doors=False)
            except (Unreachable, TimeoutError) as err:
                print("cannot walk the last leg to %s: %s" % (goal, err), flush=True)
                return False
            if _touch_statue(bot, *statue):
                return True
            return False
        here = (bot.world.x, bot.world.z)
        near = path[index - 1] if index > 0 else here
        door_tile = path[index]
        beyond = path[index + 1] if index + 1 < len(path) else near
        if near != here:
            try:
                bot.walk_to(near[0], near[1], 0, timeout=600, open_doors=False)
            except (Unreachable, TimeoutError) as err:
                print("cannot reach %s before walllow %s: %s" % (near, door_tile, err), flush=True)
                blocked.add((near, door_tile))
                continue
        if not cross_maze_door(bot, doors[door_tile], near, beyond):
            blocked.add((near, door_tile))
    print("maze unsolved after %d ticks at %s" % (15000, bot.position), flush=True)
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
