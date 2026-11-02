"""Dev tool: probe which obj debugnames the server cheat accepts (staff only).

Gives each named item once and reports the inventory delta. Never used by
a bot script.

Usage: scripts/orchestrator.py run <account> probe_give <item[=count]>...
Gives count of each item (default 1) and reports the inventory delta.
Never used by a bot script.
"""

import gamedata as knowledge


def parse(spec):
    if "=" in spec:
        name, count = spec.split("=", 1)
        return name, max(1, int(count))
    return spec, 1


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> probe_give <item[=count]>...", flush=True)
        return 2
    for spec in args:
        name, count = parse(spec)
        found = knowledge.objs.by_name(name)
        if not found:
            print("%s: UNKNOWN-LOCALLY" % name, flush=True)
            continue
        obj_id = found[0].obj_id
        before = bot.inventory().get(obj_id, 0)
        bot.cheat("give %s %d" % (name, count))
        target = bot.world.ticks + 400
        while bot.world.ticks < target:
            bot.pump(2)
            if bot.inventory().get(obj_id, 0) >= before + count:
                break
        after = bot.inventory().get(obj_id, 0)
        print("%s: %d -> %d %s" % (name, before, after, "OK" if after >= before + count else "REFUSED"), flush=True)
    return 0
