"""Walk the Varrock / Lumbridge / Falador triangle with the SDK pathfinder.

Usage: scripts/orchestrator.py run <account> walktour
"""

STOPS = (
    ("Lumbridge", 3222, 3218),
    ("Falador", 2965, 3378),
    ("Varrock", 3205, 3429),
)


def run(bot, *args):
    print("start %s" % (bot.position,), flush=True)
    for name, x, z in STOPS:
        print("leg %s -> %s (%d,%d)" % (bot.position, name, x, z), flush=True)
        bot.walk_to(x, z)
        position = bot.position
        print("arrived %s at %s" % (name, position), flush=True)
        if (position[1], position[2]) != (x, z):
            print("MISS %s: %s instead of (%d,%d)" % (name, position, x, z), flush=True)
            return 1
    print("tour complete", flush=True)
    return 0
