"""Live probe: chop one normal tree near Lumbridge, prove logs + XP arrive.

Usage: scripts/orchestrator.py run <account> probe_chop
"""


from scripts.tutorial import common as tutorial_common

TREE_NAMES = ("tree",)


def run(bot, *args):
    deadline = bot.world.ticks + 4500
    while bot.world.ticks < deadline and not bot.inventory():
        bot.pump(5)
    print("inv=%s" % bot.inventory(), flush=True)
    bot.ensure_nav()
    for obj_id, count in sorted(bot.inventory().items()):
        entry = bot.loctypes["obj"][obj_id] if obj_id < len(bot.loctypes["obj"]) else None
        print("  hold %s x%d" % ((entry["name"] if entry else obj_id), count), flush=True)
    found = tutorial_common.find_loc(bot, *TREE_NAMES)
    print("tree at %s" % (found,), flush=True)
    logs_id = tutorial_common.obj_id(bot, "logs")
    had = bot.inventory().get(logs_id, 0)
    deadline = bot.world.ticks + 6000
    chopped = tutorial_common.op_loc_near(bot, TREE_NAMES, 1, lambda probe: probe.inventory().get(logs_id, 0) > had, timeout=120)
    print("chopped=%s inv=%s wc=%s" % (chopped, bot.inventory(), bot.world.stats.get(8)), flush=True)
    return 0 if chopped else 1
