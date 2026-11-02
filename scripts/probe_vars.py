"""Dev tool: dump live quest varps after login (staff testing only).

Usage: scripts/orchestrator.py run <account> probe_vars
"""

import gamedata as knowledge

WANTED = ("cookquest", "qp", "runemysteries", "demonstart", "sheep", "prieststart")


def run(bot, *args):
    bot.pump(200)
    for name in WANTED:
        print("%s id=%s live=%s" % (name, knowledge.varp_id_by_name(name), bot.world.varps.get(knowledge.varp_id_by_name(name))), flush=True)
    print("live-nonzero=%s" % ({k: v for k, v in bot.world.varps.items() if v}), flush=True)
    return 0
