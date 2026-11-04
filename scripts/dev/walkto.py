"""Walk an account to an absolute tile with the SDK pathfinder.

Usage: scripts/orchestrator.py run <account> walkto <x> <z> [level]
"""


def run(bot, *args):
    if len(args) < 2:
        print("usage: walkto <x> <z> [level]", flush=True)
        return 2
    x, z = int(args[0]), int(args[1])
    level = int(args[2]) if len(args) > 2 else 0
    start = bot.position
    print("from %s to %d,%d,%d" % (start, x, z, level), flush=True)
    bot.walk_to(x, z, level=level)
    print("arrived %s (target %d,%d)" % (bot.position, x, z), flush=True)
