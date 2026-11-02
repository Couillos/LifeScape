"""Fishing walk helpers (split from run.py)."""

from scripts.fishing.common import *

def tile_walkable(bot, check_x, check_z):
    for known_map in (bot.opened_map, bot.collision):
        if known_map is None:
            continue
        try:
            flag = known_map.get(check_x, check_z, bot.world.level)
        except Exception:
            continue
        return (flag & collision_flags.WALK_BLOCKED) == 0
    return True



def walk_via_mid(bot, dest_x, dest_z, used):
    src_x, src_z = bot.world.x, bot.world.z
    dx, dz = dest_x - src_x, dest_z - src_z
    for off in (20, -20, 40, -40):
        if abs(dx) >= abs(dz):
            mid = (src_x + dx // 2, src_z + off)
        else:
            mid = (src_x + off, src_z + dz // 2)
        if mid in used:
            continue
        try:
            path = bot.router.find_path(bot.opened_map, bot.world.level, src_x, src_z, mid[0], mid[1])
            legs = [step for step in path if step[0] == bot.world.level]
        except Exception:
            continue
        if not legs:
            continue
        used.add(mid)
        try:
            bot.walk_to(mid[0], mid[1], bot.world.level, timeout=120, open_doors=True)
            return True
        except (Unreachable, TimeoutError):
            continue
    return False



def walk_checked(bot, dest_x, dest_z, timeout=300):
    deadline = time.time() + timeout
    hitpoints_before = hitpoints(bot)
    used_mids = set()
    trail = getattr(bot, "_trail", None)
    if trail is None:
        trail = []
        bot.trail = trail
    while (bot.world.x, bot.world.z) != (dest_x, dest_z):
        if bot.world.ticks - getattr(bot, "_last_greet", -50) > 50:
            bot.last_greet = bot.world.ticks
            randomevents.greet(bot)
        if time.time() > deadline:
            raise TimeoutError("walk to %d,%d timed out at %s" % (dest_x, dest_z, bot.position))
        step_x = bot.world.x + max(-40, min(40, dest_x - bot.world.x))
        step_z = bot.world.z + max(-40, min(40, dest_z - bot.world.z))
        try:
            bot.walk_to(step_x, step_z, bot.world.level, timeout=min(120, max(10, deadline - time.time())), open_doors=True)
        except (Unreachable, TimeoutError) as walk_error:
            if max(abs(bot.world.x - dest_x), abs(bot.world.z - dest_z)) <= 2:
                return True
            if time.time() <= deadline and walk_via_mid(bot, dest_x, dest_z, used_mids):
                continue
            print("no path from %s to (%d,%d): %s" % (bot.position, dest_x, dest_z, walk_error), flush=True)
            raise Unreachable("no path to %d,%d" % (dest_x, dest_z))
        if not trail or abs(bot.world.x - trail[-1][0]) + abs(bot.world.z - trail[-1][1]) >= 8:
            trail.append((bot.world.x, bot.world.z))
            del trail[:-100]
        if hitpoints(bot) < hitpoints_before:
            print("hurt walking, fleeing", flush=True)
            return False
        hitpoints_before = hitpoints(bot)
    return True



def retreat_along_trail(bot, max_back=2):
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



def compass_escape(bot):
    bot.ensure_nav()
    for deg in (0, 45, 90, 135, 180, 225, 270, 315):
        dest_x = bot.world.x + round(25 * math.cos(math.radians(deg)))
        dest_z = bot.world.z + round(25 * math.sin(math.radians(deg)))
        path = bot.router.find_path(bot.opened_map, bot.world.level, bot.world.x, bot.world.z, dest_x, dest_z)
        if not path:
            continue
        try:
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=90, open_doors=True)
            print("escaped to %s" % (bot.position,), flush=True)
            return True
        except (Unreachable, TimeoutError):
            continue
    return False



def ensure_not_mazed(bot):
    if not randomevents.check(bot):
        raise SystemExit("random event unresolved at %s" % (bot.position,))
