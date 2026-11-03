"""Dev tool: dump static walkability around a tile (staff testing).

Answers "is this tile walkable" from the packed collision map: no router,
no door machinery, no server round-trip. A tile the map calls blocked but
the server accepts (teleport landing, secret door) is dynamic state, not
a wall. Never used by bots.

Usage: scripts/orchestrator.py run <account> probe_tiles <x> <z> [level] [radius]
"""

from lifescape import collision as collision_mod


def run(bot, *args):
    if len(args) < 2:
        print("usage: run <account> probe_tiles <x> <z> [level] [radius]", flush=True)
        return 2
    x, z = int(args[0]), int(args[1])
    level = int(args[2]) if len(args) > 2 else bot.world.level
    radius = int(args[3]) if len(args) > 3 else 2
    bot.ensure_nav()
    for dz in range(radius, -radius - 1, -1):
        row = ""
        for dx in range(-radius, radius + 1):
            try:
                walkable = collision_mod.tile_walkable(bot.collision.get(x + dx, z + dz, level))
            except Exception:
                walkable = False
            row += "." if walkable else "#"
        print("%d %s" % (z + dz, row), flush=True)
    cx, cz = x, z
    for _ in range(6):
        try:
            opened = collision_mod.step_open(bot.collision, cx, cz, level, -1, 0)
        except Exception as error:
            print("step (%d,%d)->(%d,%d): error %s" % (cx, cz, cx - 1, cz, error), flush=True)
            break
        print("step (%d,%d)->(%d,%d): %s" % (cx, cz, cx - 1, cz, opened), flush=True)
        if not opened:
            break
        cx -= 1
    return 0
