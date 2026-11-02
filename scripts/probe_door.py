"""Dev tool: probe one door instance (staff testing only).

Teleports next to the tile, stands on a server-accepted side, clicks it
once, then tries to step onto it. Prints every intermediate result so a
refused click or a refused step is unambiguous.

Usage: scripts/orchestrator.py run <account> probe_door <x> <z> [level] [noclick|clickonly]
"""

from lifescape import doors as doors_mod


def run(bot, *args):
    if len(args) < 2:
        print("usage: run <account> probe_door <x> <z> [level] [noclick|clickonly]", flush=True)
        return 2
    x, z = int(args[0]), int(args[1])
    level = int(args[2]) if len(args) > 2 else 0
    mode = args[3] if len(args) > 3 else ""
    bot.ensure_nav()
    spot = bot.door_spot(x, z, level)
    print("spot=%s sides=%s" % (
        spot,
        bot.door_standing_sides(spot) if spot is not None else None,
    ), flush=True)
    loc = bot.world.loc_at(x, z, level)
    print("loc=%s" % (loc,), flush=True)
    if spot is None:
        print("no known door at %d,%d,%d" % (x, z, level), flush=True)
        return 0
    op = doors_mod.open_op(bot.loctypes, spot[9])
    print("op=%s" % (op,), flush=True)
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    target = bot.world.ticks + 500
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            break
    print("at=%s" % (bot.position,), flush=True)
    if not bot.stand_on_door_side(spot, timeout=750):
        print("no reachable side", flush=True)
        return 0
    print("side=%s" % ((bot.world.x, bot.world.z),), flush=True)
    if mode not in ("noclick", "clickonly"):
        if bot.try_step_on(x, z, level, bot.world.ticks + 150):
            print("step-first: OPEN ALREADY", flush=True)
            return 0
    if mode == "clickonly":
        # Bare click test: stand on the first valid side and click three
        # times with pauses, no unseal, no step. Watch the server truth
        # (/api/player) for a type flip.
        if op is None:
            print("no Open op (already open?)", flush=True)
            return 0
        if not bot.stand_on_door_side(spot, timeout=750):
            print("no reachable side", flush=True)
            return 0
        print("side=%s clicking 3x" % ((bot.world.x, bot.world.z),), flush=True)
        for _ in range(3):
            bot.send_oploc(op, x, z, spot[9])
            bot.pump(20)
        print("clicked 3x from %s" % ((bot.world.x, bot.world.z),), flush=True)
        return 0
    before = bot.world.loc_at(x, z, level)
    if bot.open_door(x, z, timeout=500):
        print("PROBE OK: opened/stepped (was=%s now=%s)" % (before, bot.world.loc_at(x, z, level)), flush=True)
        return 0
    print("PROBE FAIL: still closed after click, loc=%s" % (bot.world.loc_at(x, z, level),), flush=True)
    return 0