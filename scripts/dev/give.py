"""Dev tool: hand items to a staff account (testing only).

A quest requirement needs a bank item the test account does not have yet;
this puts it in the backpack through the staff cheat. Never used by a bot
script.

Usage: scripts/orchestrator.py run <account> give <item> [count]
"""

import gamedata as knowledge


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> give <item> [count]", flush=True)
        return 2
    item = args[0]
    count = int(args[1]) if len(args) > 1 else 1
    bot.intend("give %s x%d" % (item, count))
    obj_id = knowledge.objs.by_name(item)[0].obj_id
    before = bot.inventory().get(obj_id, 0)
    bot.cheat("give %s %d" % (item, count))
    target = bot.world.ticks + 120
    while bot.world.ticks < target:
        bot.pump(2)
        if bot.inventory().get(obj_id, 0) >= before + count:
            print("have %s x%d" % (item, bot.inventory().get(obj_id, 0)), flush=True)
            return 0
    print("give failed: inventory %s" % (bot.inventory(),), flush=True)
    return 0
