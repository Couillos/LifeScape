"""Fallback rescue for random events with no dedicated script.

Relogins (bounded attempts) then loops the shared event check for a
bounded number of ticks. Returns 0 when the bot is back on level 0
outside any event zone, 1 when the bound is exhausted: the session then
goes failed and the other sessions keep running.
"""

from scripts.randomevents import check

CHECK_TICKS = 1500


def run(bot, *args):
    bot.intend("rescue: fallback")
    try:
        bot.relogin()
    except (TimeoutError, ConnectionError, OSError) as error:
        print("fallback relogin failed: %s" % (error,), flush=True)
        return 1
    target = bot.world.ticks + CHECK_TICKS
    while bot.world.ticks < target:
        try:
            if check(bot):
                print("fallback clear at %s" % (bot.position,), flush=True)
                return 0
        except Exception as error:
            print("fallback check failed: %s: %s" % (type(error).__name__, error), flush=True)
        bot.pump(5)
    print("fallback exhausted at %s" % (bot.position,), flush=True)
    return 1
