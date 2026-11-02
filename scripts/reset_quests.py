"""Dev tool: reset every quest on a staff account (testing only).

Runs the server's ~resetquests debugproc, which zeroes all quest stage
varps. Never used by a bot script.

Usage: scripts/orchestrator.py run <account> reset_quests
"""


def run(bot, *args):
    bot.intend("resetting quests")
    bot.cheat("~resetquests")
    target = bot.world.ticks + 1000
    while bot.world.ticks < target:
        bot.pump(2)
    print("reset sent, at %s" % (bot.position,), flush=True)
    return 0
