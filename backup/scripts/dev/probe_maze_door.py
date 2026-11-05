"""Dev tool: systematic walllow reach/step/click probe (staff testing).

Teleports next to a maze door and tries, side by side: raw step onto
the door, op_loc click, and (from the door tile) raw steps off it.
Dumps the server answer for every attempt. Never solves anything.

Usage: scripts/orchestrator.py run <account> probe_maze_door <x> <z> [level]
"""

from lifescape import collision as collision_mod


def _dump(bot, tag):
    print("--- %s pos=%s modal=%s" % (tag, bot.world.position, bot.world.modal), flush=True)
    for text in bot.world.messages[-3:]:
        print("  msg: %r" % (text[:120],), flush=True)
    for _com, text in list(bot.chat_texts())[-3:]:
        print("  text: %r" % (text[:100],), flush=True)


def _tele(bot, x, z, level):
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    target = bot.world.ticks + 60
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            break
    print("at %s (want %s)" % (bot.position, (level, x, z)), flush=True)


def run(bot, *args):
    if len(args) < 2:
        print("usage: run <account> probe_maze_door <x> <z> [level]", flush=True)
        return 2
    x, z = int(args[0]), int(args[1])
    level = int(args[2]) if len(args) > 2 else 0
    bot.ensure_nav()
    loc = bot.world.loc_at(x, z, level)
    static = bot.world.static_locs.get((x, z, level))
    print("door (%d,%d,%d) live=%s static=%s" % (x, z, level, loc, static), flush=True)
    entry = bot.loctypes["loc"][(loc[0] if loc else static[0])]
    print("type=%s ops=%s" % (entry["name"], entry.get("ops")), flush=True)
    loc_id = (loc[0] if loc else static[0])
    for name, dx, dz in (("EAST", 1, 0), ("WEST", -1, 0), ("NORTH", 0, 1), ("SOUTH", 0, -1)):
        _tele(bot, x + dx, z + dz, level)
        _dump(bot, "stand %s" % name)
        try:
            ok = bot.step_to(x, z, timeout_ticks=30)
            print("raw step %s onto door: %s at %s" % (name, ok, bot.position), flush=True)
        except Exception as error:
            print("raw step %s onto door: RAISED %s at %s" % (name, error, bot.position), flush=True)
        _tele(bot, x + dx, z + dz, level)
        seen = len(bot.world.messages)
        bot.op_loc(1, x, z, loc_id)
        target = bot.world.ticks + 15
        moved = modal = None
        while bot.world.ticks < target:
            bot.pump(1)
            if bot.world.modal is not None:
                modal = bot.world.modal
                break
            if (bot.world.x, bot.world.z, bot.world.level) != (level, x + dx, z + dz):
                moved = (bot.world.x, bot.world.z, bot.world.level)
                break
        print("click from %s: moved=%s modal=%s at %s" % (name, moved, modal, bot.position), flush=True)
        for text in bot.world.messages[seen:]:
            print("  new msg: %r" % (text[:120],), flush=True)
        bot.pump(5)
    _tele(bot, x, z, level)
    for name, dx, dz in (("EAST", 1, 0), ("WEST", -1, 0), ("NORTH", 0, 1), ("SOUTH", 0, -1)):
        try:
            ok = bot.step_to(x + dx, z + dz, timeout_ticks=30)
            print("step off door %s: %s at %s" % (name, ok, bot.position), flush=True)
        except Exception as error:
            print("step off door %s: RAISED %s at %s" % (name, error, bot.position), flush=True)
        _tele(bot, x, z, level)
    _dump(bot, "done")
    return 0
