"""Walk the bot and prove arrival by its own position updates.

Usage: scripts/orchestrator.py run <account> probe_walk [dx] [dz]
"""


def run(bot, *args):
    dx = int(args[0]) if len(args) > 0 else 10
    dz = int(args[1]) if len(args) > 1 else 0
    start = bot.position
    print("start %s" % (start,))
    target = (start[1] + dx, start[2] + dz)
    bot.walk_to(*target)
    print("arrived %s (target %s)" % (bot.position, target))
