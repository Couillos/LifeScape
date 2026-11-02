"""Dev tool: put an account on a known tile (staff teleport).

Only for staff accounts while testing: a bot stranded in scenery would
otherwise need a long walk the SDK may legitimately refuse. Never used by a bot
script.

Usage: scripts/orchestrator.py run <account> goto <x> <z> [level]
"""


def run(bot, *args):
    if len(args) < 2:
        print("usage: run <account> goto <x> <z> [level]", flush=True)
        return 2
    x, z = int(args[0]), int(args[1])
    level = int(args[2]) if len(args) > 2 else 0
    bot.intend("teleport to %d,%d,%d" % (x, z, level))
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    target = bot.world.ticks + 100
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            print("at %s" % (bot.position,), flush=True)
            return 0
    print("teleport did not land: %s" % (bot.position,), flush=True)
    return 1
