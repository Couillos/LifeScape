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
- mime: solved by scripts/rescue_mime.py (copy the 4 emotes).
- fail-teleport towers: climbed down when reachable.

Not solved (clean abort instead): the strange box dance (handled by
rescue_cube), swarm/triffid aggression (the walk flee handles those).
Triffid plants are picked on sight by survive(), before they turn hostile.
"""

import heapq
import json
from collections import deque


import gamedata as knowledge
from lifescape import collision as collision_mod
from lifescape.client import Unreachable  # noqa: F401  (re-exported for callers)
from scripts.tutorial import common as tutorial_common

EVENT_HOSTS = ("macro_mage", "macro_geni", "macro_dwarf")
# Hostile random events: the swarm and the angry triffid follow and hit the
# player. They never despawn on their own (the server only drops them when
# their mode is back to wander), so a bot that just walks on bleeds out.
HOSTILE_EVENTS = ("macro_swarm", "macro_triffidseed_angry")

DOOR_COST = 80  # ticks a walllow crossing costs, against 1 per tile

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


def pick_triffid(bot):
    """Pick a triffid plant's fruit when one is in click range.

    A ready seed hands the fruit and clears the event; a growing one just
    says it isn't ready (harmless). One attempt per survive() call is
    enough: check() runs once per work loop, and a missed seed turns
    hostile and is killed below instead.
    """
    seeds = bot.npcs_named(["macro_triffidseed"])
    if not seeds:
        return
    nid, npc = min(seeds, key=lambda item: abs(item[1]["x"] - bot.world.x) + abs(item[1]["z"] - bot.world.z))
    if abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z) > 6:
        return
    try:
        bot.op_npc(1, nid)
    except (ConnectionError, OSError):
        raise
    except Exception:
        pass


def survive(bot, ticks=600):
    """Kill hostile event NPCs before they bleed the bot dry.

    Returns True when none is left (or none was there). Bounded in ticks:
    a bot that cannot kill it still returns control to its own script.
    """
    pick_triffid(bot)
    deadline = bot.world.ticks + ticks
    while bot.world.ticks < deadline:
        foes = bot.npcs_named(HOSTILE_EVENTS)
        if not foes:
            return True
        nid, npc = min(foes, key=lambda item: abs(item[1]["x"] - bot.world.x) + abs(item[1]["z"] - bot.world.z))
        if abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z) > 6:
            return True
        try:
            bot.op_npc(2, nid)
        except (ConnectionError, OSError):
            raise
        except Exception:
            pass
        bot.pump(5)
    return not bot.npcs_named(HOSTILE_EVENTS)


# Fail-teleport spots, decoded from
# content/scripts/macro events/configs/macro_events_teleports.enum
# (macro_event_fail_teleports_free). A bot standing on one of these after
# ignoring an event host was yanked there; anywhere else upstairs is a
# normal floor (a quest room, a bank upper storey) and none of our business.
FAIL_TELEPORTS = {
    (1, 3278, 3500), (1, 3097, 3268), (2, 2954, 3388), (3, 3221, 3474),
    (1, 3229, 3223), (0, 3097, 9567), (2, 3287, 3190), (1, 2964, 3213),
    (0, 2968, 3335), (0, 2935, 3515), (0, 3212, 3423), (0, 3076, 3412),
    (0, 2996, 3145), (1, 3025, 3251), (1, 2975, 3382), (1, 3214, 3215),
    (1, 3112, 3157), (0, 3246, 3451), (0, 3283, 3218), (0, 3045, 3491),
    (0, 3083, 3469), (2, 3104, 3163), (0, 3218, 9618), (0, 3263, 3424),
}


def in_fail_teleport(bot):
    return (bot.world.level, bot.world.x, bot.world.z) in FAIL_TELEPORTS


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
    """Tiles whose presence ends the event.

    The server closes the maze when the player stands in the centre rect
    (coordx 2909..2913, coordz 4573..4577 in [timer,macro_maze]), no statue
    click needed. The statue loc only locates the centre.
    """
    statue = maze_statue(bot)
    if statue is None:
        return set()
    x, z, _loc_id = statue
    return {
        (tx, tz)
        for tx in range(x - 2, x + 3)
        for tz in range(z - 2, z + 3)
    }


def maze_path(bot, goals, doors, blocked):
    """Cheapest path to any goal: tiles cost 1, a walllow costs DOOR_COST.

    Crossing a door is what eats the event timer (the SDK stands on a
    valid side, clicks, and steps through), so the route must prefer a
    longer corridor over one extra door. `blocked` holds the directed
    edges (from_tile, door_tile) the server already refused.
    """
    start = (bot.world.x, bot.world.z)
    previous = {start: None}
    cost = {start: 0}
    pending = [(0, start)]
    level = bot.world.level
    while pending:
        spent, here = heapq.heappop(pending)
        if spent > cost.get(here, 1 << 30):
            continue
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
            door = here in doors or nxt in doors
            if not traversable and not door:
                continue
            # A refused edge is never retried, door or plain step: the
            # server knows things the static map does not.
            if (here, nxt) in blocked:
                continue
            step = DOOR_COST if door else 1
            total = spent + step
            if total < cost.get(nxt, 1 << 30):
                cost[nxt] = total
                previous[nxt] = here
                heapq.heappush(pending, (total, nxt))
    return None


REFUSALS = ("right way", "can't reach", "can't open", "locked", "nothing interesting")


def _dialog_open(bot):
    """Whether a real dialogue is open. The overlay is not one."""
    modal = bot.world.modal
    return modal is not None and modal[0] not in ("overlay", "side")


def cross_door_step(bot, spot, beyond, attempts, here, nxt):
    """Click a walllow; the server teleports through, no step needed.

    The door script walks the player onto the tile then teleports past
    it, so any position change is progress: recompute from the new spot.
    Only an explicit refusal blocks the directed edge. Silence costs one
    retry (a slow script must not partition the maze); the static loc id
    is always clicked, never the transient open-state id.
    Returns True when it moved, else False.
    """
    x, z = spot[0], spot[1]
    # A modal makes the server ignore ops entirely (canAccess is false
    # while busy). The overlay is not a modal; real dialogues are.
    if _dialog_open(bot):
        bot.close_modal()
        bot.pump(2)
    # Let the previous leg settle: a click that lands on the same tick as a
    # walk step is dropped by the server's arrival delay.
    bot.pump(2)
    before = (bot.world.x, bot.world.z)
    modal_before = bot.world.modal
    texts_before = {text for _com, text in bot.chat_texts()}
    msgs_before = len(bot.world.messages)
    bot.op_loc(1, x, z, spot[9])
    target = bot.world.ticks + 20
    while bot.world.ticks < target:
        bot.pump(1)
        if (bot.world.x, bot.world.z) != before:
            return True
        fresh = [str(line) for line in bot.world.messages[msgs_before:]]
        modal = bot.world.modal
        if modal is not None and modal != modal_before and modal[0] not in ("overlay", "side"):
            fresh += [text for _com, text in bot.chat_texts() if text not in texts_before]
        if any(marker in " ".join(fresh).lower() for marker in REFUSALS):
            print("walllow (%d,%d): refused" % (x, z), flush=True)
            return False
    key = (here, nxt)
    attempts[key] = attempts.get(key, 0) + 1
    if attempts[key] >= 2:
        print("walllow (%d,%d): silent twice, learning edge (was %s now %s)" % (
            x, z, before, (bot.world.x, bot.world.z)), flush=True)
        return False
    print("walllow (%d,%d): silent, retrying once" % (x, z), flush=True)
    return cross_door_step(bot, spot, beyond, attempts, here, nxt)


def _try_edge(bot, doors, here, nxt, blocked, attempts):
    """Cross one door edge. Returns True when it moved, else False.

    The SDK's open_door already stands on a reach-valid side, clicks, and
    steps through (proven by probe_door on this very maze). Raw clicks from
    our own guess were silently dropped by the server's reach check, so the
    SDK is the primary path; one plain retry, then the edge is learned.
    """
    spot = doors.get(nxt, doors.get(here))
    if spot is None or here in doors:
        # Standing on the door tile already: step off it instead of clicking
        # it again (a second click closes it and we oscillate). A closed door
        # underfoot refuses routed steps, so fall back to a bare move packet.
        try:
            if bot.step_to(nxt[0], nxt[1], timeout_ticks=20):
                return True
        except (Unreachable, TimeoutError) as err:
            print("step to %s failed: %s" % (nxt, err), flush=True)
        # Directed raw step at the planned tile only: an undirected step-off
        # picks the nearest walkable neighbour (i.e. backwards) and the
        # solver bounces between the two tiles forever.
        if bot.try_step_on(nxt[0], nxt[1], bot.world.level, bot.world.ticks + 75):
            return (bot.world.x, bot.world.z) == nxt
        blocked.add((here, nxt))
        return False
    if _dialog_open(bot):
        bot.close_modal()
        bot.pump(2)
    for _round in range(2):
        try:
            if bot.open_door(spot[0], spot[1], timeout=25):
                return True
        except (Unreachable, TimeoutError) as err:
            print("door %d,%d failed: %s" % (spot[0], spot[1], err), flush=True)
    key = (here, nxt)
    attempts[key] = attempts.get(key, 0) + 1
    if attempts[key] >= 2:
        print("walllow (%d,%d): never opened, learning edge" % (spot[0], spot[1]), flush=True)
        blocked.add((here, nxt))
        return False
    print("walllow (%d,%d): not open yet, retrying" % (spot[0], spot[1]), flush=True)
    return True


def _maze_percent(bot):
    """Reward percentage shown by the maze timer overlay, or -1."""
    for _com, text in bot.chat_texts():
        value = str(text).strip().rstrip("%")
        if value.isdigit() and len(value) <= 3:
            return int(value)
    return -1


def _crosses_door(doors, here, nxt):
    return here in doors or nxt in doors


def _run_leg(bot, path, start, doors):
    """Longest stretch of plain corridor from path[start], and its end index.

    The event timer only allows ~500 ticks, so a 188-tile route has to be
    walked in legs (multi-step walk packets), not one step per tick.
    """
    end = start
    while end + 1 < len(path) and not _crosses_door(doors, path[end], path[end + 1]):
        end += 1
    return path[start:end + 1], end


def solve_maze(bot):
    """Walk to the maze centre, in corridor legs, clicking the walllows.

    The route comes from the collision map. Plain corridor is walked as one
    leg per walk packet (the event timer is short); door edges are clicked
    one at a time because the server decides which side crosses.
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
    started = bot.world.ticks
    stalled = 0
    if _dialog_open(bot):
        # The intro chat makes every op inert (busy => no access).
        bot.close_modal()
        bot.pump(3)
    blocked = set()
    attempts = {}
    forgiven = False
    last_greet = 0
    deadline = bot.world.ticks + 15000
    while in_maze(bot) and bot.world.ticks < deadline:
        if bot.world.ticks - last_greet > 100:
            # Greet event hosts regularly: an ignored host fail-teleports
            # the bot out of the maze mid-solve.
            last_greet = bot.world.ticks
            print("maze: %d ticks in, %d%% left at %s" % (
                bot.world.ticks - started, _maze_percent(bot), bot.position), flush=True)
            try:
                greet(bot)
                survive(bot, ticks=200)
            except Exception as error:
                print("maze greet failed: %s" % (error,), flush=True)
        before_iteration = (bot.world.x, bot.world.z)
        path = maze_path(bot, goals, doors, blocked)
        if path is None:
            if not forgiven and blocked:
                # Learned refusals may include a transient (slow script,
                # missed packet): forgive once and replan before giving up.
                print("maze: no route, forgiving %d learned edges once" % (len(blocked),), flush=True)
                blocked = set()
                attempts = {}
                forgiven = True
                continue
            print("maze: no route left from %s" % (bot.position,), flush=True)
            return False
        progressed = False
        index = 1
        while index < len(path):
            if not in_maze(bot):
                return True
            gone = (bot.world.x, bot.world.z)
            if abs(path[index][0] - gone[0]) + abs(path[index][1] - gone[1]) != 1:
                break
            if _crosses_door(doors, gone, path[index]):
                door_started = bot.world.ticks
                crossed = _try_edge(bot, doors, gone, path[index], blocked, attempts)
                moved = (bot.world.x, bot.world.z)
                if crossed and moved != gone:
                    print("maze door %s -> %s took %d ticks" % (
                        gone, moved, bot.world.ticks - door_started), flush=True)
                    progressed = True
                    index += 1
                    continue
                # The click "succeeded" but left us on the same tile: the
                # door only opens the other way. Learn it, do not bounce.
                print("maze door %s did not move us (%s)" % (gone, moved), flush=True)
                blocked.add((gone, path[index]))
                progressed = True
                break
            leg, end = _run_leg(bot, path, index, doors)
            try:
                bot.walk_steps(leg[1:], timeout_ticks=60)
            except (Unreachable, TimeoutError) as err:
                print("maze leg to %s failed: %s" % (leg[-1], err), flush=True)
            if (bot.world.x, bot.world.z) == gone:
                # The leg moved us nowhere: learn why and replan.
                blocked.add((gone, path[index]))
                progressed = True
                break
            progressed = True
            index = end + 1
        if not progressed:
            print("maze: stuck at %s" % (bot.position,), flush=True)
            return False
        if (bot.world.x, bot.world.z) == before_iteration:
            stalled += 1
            if stalled >= 6:
                print("maze: no progress at %s, giving up" % (bot.position,), flush=True)
                return False
        else:
            stalled = 0
    print("maze: %d ticks spent, %d to go" % (
        bot.world.ticks - started,
        abs(bot.world.x - sorted(goals)[0][0]) + abs(bot.world.z - sorted(goals)[0][1]) if goals else -1,
    ), flush=True)
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


def event_at(bot):
    """Random event that must be cleared before a script can work: maze,
    mime or cube. Zone checks that depend on walking live in check()."""
    if in_maze(bot):
        return "maze"
    if in_mime(bot):
        return "mime"
    if cube_in_pack(bot):
        return "cube"
    return None


def report(bot, event, resolved, detail=None):
    """Tell the orchestrator (log event) what a random event needs."""
    payload = {"kind": "random_event", "event": event, "resolved": bool(resolved), "at": bot.position}
    if detail:
        payload["detail"] = detail
    print("@@EVENT " + json.dumps(payload), flush=True)


def check(bot):
    """Shared per-iteration event check. True means keep botting."""
    greet(bot)
    survive(bot)
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
    if in_fail_teleport(bot):
        # Yanked by an ignored event host. Climb down; the spot itself is
        # proof the event is over, so nothing else is pending.
        escaped = bot.world.level == 0 or climb_out(bot)
        report(bot, "tower", escaped)
        return escaped
    return True
