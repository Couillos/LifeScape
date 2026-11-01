"""Mining walk helpers (split from run.py)."""

from scripts.mining.common import *

def tile_walkable(bot, check_x, check_z):
    for known_map in (bot._opened_map, bot._collision):
        if known_map is None:
            continue
        try:
            flag = known_map.get(check_x, check_z, bot.world.level)
        except Exception:
            continue
        return (flag & collision_flags.WALK_BLOCKED) == 0
    return True



def step_open(bot, dest_x, dest_z):
    """Next hop at most 12 tiles toward dest, on a walkable tile.

    Blind chunks land inside buildings or crop patches, where the
    router has nothing to approach. Nudge to the nearest walkable tile.
    """
    step_x = bot.world.x + max(-12, min(12, dest_x - bot.world.x))
    step_z = bot.world.z + max(-12, min(12, dest_z - bot.world.z))
    if tile_walkable(bot, step_x, step_z):
        return step_x, step_z
    cands = []
    for radius in range(1, 5):
        for off_x in range(-radius, radius + 1):
            for off_z in range(-radius, radius + 1):
                if max(abs(off_x), abs(off_z)) != radius:
                    continue
                cand_x, cand_z = step_x + off_x, step_z + off_z
                if tile_walkable(bot, cand_x, cand_z):
                    cands.append((abs(cand_x - dest_x) + abs(cand_z - dest_z), cand_x, cand_z))
        if cands:
            cands.sort()
            return cands[0][1], cands[0][2]
    return step_x, step_z



def walk_checked(bot, dest_x, dest_z, timeout=300):
    # Failure backstop on purpose in wall-clock seconds (walk_to takes
    # seconds): pacing waits above are all tick-based, this only bounds
    # a stuck walk.
    deadline = time.time() + timeout
    hitpoints_before = hitpoints(bot)
    max_hitpoints = hitpoints_before
    visited_detours = []
    trail = getattr(bot, "_trail", None)
    if trail is None:
        trail = []
        bot._trail = trail
    while (bot.world.x, bot.world.z) != (dest_x, dest_z):
        if bot.world.ticks - getattr(bot, "_last_greet", -50) > 50:
            bot._last_greet = bot.world.ticks
            randomevents.greet(bot)
        if time.time() > deadline:
            raise TimeoutError("walk to %d,%d timed out at %s" % (dest_x, dest_z, bot.position))
        # Short legs: the router gives up on long cuts through dense
        # fields (crop patches), but weaves around them on short hops.
        step_x, step_z = step_open(bot, dest_x, dest_z)
        try:
            bot.walk_to(step_x, step_z, bot.world.level, timeout=min(120, max(10, deadline - time.time())), open_doors=True)
        except Unreachable:
            if max(abs(bot.world.x - dest_x), abs(bot.world.z - dest_z)) <= 2:
                return True
            if max(abs(bot.world.x - dest_x), abs(bot.world.z - dest_z)) <= 12 and settle_step(bot, step_x, step_z, deadline):
                pass
            elif walk_detour(bot, dest_x, dest_z, step_x, step_z, deadline, visited_detours):
                pass
            elif cross_nearby_door(bot, dest_x, dest_z):
                pass
            else:
                print("no path from %s to (%d,%d)" % (bot.position, dest_x, dest_z), flush=True)
                raise
        if not trail or abs(bot.world.x - trail[-1][0]) + abs(bot.world.z - trail[-1][1]) >= 8:
            trail.append((bot.world.x, bot.world.z))
            del trail[:-100]
        hitpoints_now = hitpoints(bot)
        max_hitpoints = max(max_hitpoints, hitpoints_now, hitpoints_before)
        if hitpoints_now * 2 <= max_hitpoints and max_hitpoints > 2:
            print("heavily hurt walking, fleeing", flush=True)
            return False
        hitpoints_before = hitpoints_now
    return True



def cross_nearby_door(bot, dest_x, dest_z):
    """Click open a nearby closed door/gate (city gates are locs, not
    wall doors, so the router alone never gets through them) and step
    through before it swings shut again."""
    bot._ensure_nav()
    seen = list(bot.world.locs.items()) + [(key, val) for key, val in bot.world.static_locs.items() if key not in bot.world.locs]
    cands = []
    for (door_x, door_z, door_level), loc in seen:
        if loc is None or door_level != bot.world.level:
            continue
        if abs(door_x - bot.world.x) + abs(door_z - bot.world.z) > 12:
            continue
        if doors_module.open_op(bot._loctypes, loc[0]) is None:
            continue
        cands.append((abs(door_x - bot.world.x) + abs(door_z - bot.world.z), door_x, door_z, loc[0]))
    for _dist, door_x, door_z, door_loc in sorted(cands):
        beyond = (door_x + (1 if dest_x > door_x else -1 if dest_x < door_x else 0), door_z + (1 if dest_z > door_z else -1 if dest_z < door_z else 0))
        try:
            if C.cross_door(bot, door_x, door_z, door_loc, beyond=beyond, what="door"):
                print("door crossed at (%d,%d)" % (door_x, door_z), flush=True)
                return True
        except (Unreachable, TimeoutError):
            continue
    return False



def settle_step(bot, step_x, step_z, deadline):
    """Near the goal, blockage is often a passing NPC: wait a moment and
    retry the direct step a couple of times before detouring away."""
    for _retry in range(2):
        bot.pump(10)
        try:
            bot.walk_to(step_x, step_z, bot.world.level, timeout=min(30, max(10, deadline - time.time())), open_doors=True)
            return True
        except (Unreachable, TimeoutError):
            continue
    return False



def walk_detour(bot, dest_x, dest_z, step_x, step_z, deadline, visited_detours):
    """Sidestep around a blocked chunk: try perpendicular offsets,
    threat-safe tiles first so detours don't wander into hunters."""
    delta_x, delta_z = dest_x - bot.world.x, dest_z - bot.world.z
    close = abs(delta_x) + abs(delta_z) <= 30
    laterals = (8, -8, 16, -16) if close else (8, -8, 16, -16, 32, -32)
    cands = []
    for lateral in laterals:
        if abs(delta_x) >= abs(delta_z):
            cand_x, cand_z = step_x, step_z + lateral
        else:
            cand_x, cand_z = step_x + lateral, step_z
        if not tile_walkable(bot, cand_x, cand_z):
            continue
        if abs(cand_x - bot.world.x) + abs(cand_z - bot.world.z) <= 2:
            continue
        if (cand_x, cand_z) in visited_detours:
            continue
        # Never walk back onto ground just covered: detours that only
        # succeed because they undo each other ping-pong forever.
        trail = getattr(bot, "_trail", [])[-12:]
        if any(abs(cand_x - seen_x) + abs(cand_z - seen_z) <= 6 for seen_x, seen_z in trail):
            continue
        cands.append((cand_x, cand_z))
    cands.sort(key=lambda cand: (not spot_is_safe(bot, cand[0], cand[1]), abs(cand[0] - dest_x) + abs(cand[1] - dest_z)))
    for cand_x, cand_z in cands:
        try:
            bot.walk_to(cand_x, cand_z, bot.world.level, timeout=min(60, max(10, deadline - time.time())), open_doors=True)
            print("detour to (%d,%d)" % (cand_x, cand_z), flush=True)
            visited_detours.append((cand_x, cand_z))
            del visited_detours[:-8]
            return True
        except (Unreachable, TimeoutError):
            continue
    return False



def retreat_along_trail(bot, max_back=12):
    trail = getattr(bot, "_trail", [])
    steps = 0
    while trail and steps < max_back:
        dest_x, dest_z = trail.pop()
        if abs(dest_x - bot.world.x) + abs(dest_z - bot.world.z) < 4:
            continue
        try:
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=90, open_doors=True)
            steps += 1
        except (Unreachable, TimeoutError):
            continue
    return steps > 0



def compass_escape(bot, dest_x=None, dest_z=None):
    bot._ensure_nav()
    degs = (0, 45, 90, 135, 180, 225, 270, 315)
    if dest_x is not None and dest_z is not None:
        # Escape toward the goal, not blindly: a blind jump can fling
        # the bot to the wrong side of the obstacle it just avoided.
        degs = sorted(degs, key=lambda deg: abs(bot.world.x + round(25 * math.cos(math.radians(deg))) - dest_x) + abs(bot.world.z + round(25 * math.sin(math.radians(deg))) - dest_z))
    for deg in degs:
        dest_x = bot.world.x + round(25 * math.cos(math.radians(deg)))
        dest_z = bot.world.z + round(25 * math.sin(math.radians(deg)))
        path = bot._router.find_path(bot._opened_map, bot.world.level, bot.world.x, bot.world.z, dest_x, dest_z)
        if not path:
            continue
        try:
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=90, open_doors=True)
            print("escaped to %s" % (bot.position,), flush=True)
            return True
        except (Unreachable, TimeoutError):
            continue
    return False
