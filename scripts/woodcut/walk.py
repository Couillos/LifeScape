"""Woodcut walk helpers (split from run.py)."""

from scripts.woodcut.common import *
from scripts.tutorial import common as tutorial_common

from lifescape import collision as collision_mod


def dest_blocked(bot, check_x, check_z):
    """Cible non marchable (mur, table, booth) : s'arreter a cote."""
    try:
        return not collision_mod.tile_walkable(bot.collision.get(check_x, check_z, bot.world.level))
    except (KeyError, IndexError):
        return False


def walk_checked(bot, dest_x, dest_z, timeout=15000):
    # Failure backstop on purpose in wall seconds (walk_to takes seconds):
    # pacing waits above are all tick-based, this only bounds a stuck walk.
    deadline = bot.world.ticks + timeout
    hitpoints_before = hitpoints(bot)
    trail = getattr(bot, "_trail", None)
    if trail is None:
        trail = []
        bot.trail = trail
    while (bot.world.x, bot.world.z) != (dest_x, dest_z):
        if dest_blocked(bot, dest_x, dest_z) and max(abs(bot.world.x - dest_x), abs(bot.world.z - dest_z)) <= 2:
            return True
        if bot.world.ticks - getattr(bot, "_last_greet", -50) > 50:
            bot.last_greet = bot.world.ticks
            randomevents.greet(bot)
        if bot.world.ticks > deadline:
            raise TimeoutError("walk to %d,%d timed out at %s" % (dest_x, dest_z, bot.position))
        step_x = bot.world.x + max(-40, min(40, dest_x - bot.world.x))
        step_z = bot.world.z + max(-40, min(40, dest_z - bot.world.z))
        try:
            bot.walk_to(step_x, step_z, bot.world.level, timeout=min(6000, max(500, deadline - bot.world.ticks)), open_doors=True)
        except Unreachable:
            if max(abs(bot.world.x - dest_x), abs(bot.world.z - dest_z)) <= 2:
                return True
            print("no path from %s to (%d,%d)" % (bot.position, dest_x, dest_z), flush=True)
            raise
        if not trail or abs(bot.world.x - trail[-1][0]) + abs(bot.world.z - trail[-1][1]) >= 8:
            trail.append((bot.world.x, bot.world.z))
            del trail[:-100]
        if hitpoints(bot) < hitpoints_before:
            print("hurt walking, fleeing", flush=True)
            return False
        hitpoints_before = hitpoints(bot)
    return True



def retreat_along_trail(bot, max_back=5):
    trail = getattr(bot, "_trail", [])
    steps = 0
    while trail and steps < max_back:
        dest_x, dest_z = trail.pop()
        if abs(dest_x - bot.world.x) + abs(dest_z - bot.world.z) < 4:
            continue
        try:
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=4500, open_doors=True)
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
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=4500, open_doors=True)
            print("escaped to %s" % (bot.position,), flush=True)
            return True
        except (Unreachable, TimeoutError):
            continue
    return False



def cross_toll_gate(bot):
    """Cross the Lumbridge/Al-Kharid toll fence westward via the guard dialog (10 coins)."""
    gate = nearest_toll_gate(bot)
    if gate is None:
        print("no toll gate found", flush=True)
        return False
    gate_x, gate_z, _level, gate_loc = gate
    if bot.world.x < gate_x:
        return True
    # The server only talks to the guard within 1 tile: stand adjacent.
    bot.approach(gate_x, gate_z, radius=1, open_doors=True)
    if bot.world.x < gate_x:
        print("walked through the toll gate at %s" % (bot.position,), flush=True)
        return True
    for _attempt in range(3):
        bot.op_loc(1, gate_x, gate_z, gate_loc)
        target = bot.world.ticks + 25
        while bot.world.ticks < target and bot.world.modal is None:
            bot.pump(5)
        if bot.world.modal is not None:
            break
        bot.approach(gate_x, gate_z, radius=1, open_doors=True)
    else:
        print("toll gate guard never talked", flush=True)
        return False
    tutorial_common.finish_dialog(bot, ("Yes, ok.",), timeout=60)
    if bot.world.x < gate_x:
        print("through the toll gate at %s" % (bot.position,), flush=True)
        return True
    if not tutorial_common.wait_moved(bot, timeout=30):
        print("toll gate did not move us", flush=True)
        return False
    print("through the toll gate at %s" % (bot.position,), flush=True)
    return bot.world.x < gate_x



def ensure_not_mazed(bot):
    if not randomevents.check(bot):
        raise SystemExit("random event unresolved at %s" % (bot.position,))
