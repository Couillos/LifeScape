"""Full item loop on the static knife spawn: wait, take, drop, take back.

Uses the knife (946) at (3224, 3202) which respawns ~58 s after taking.
No cheats: pure ground-spawn protocol.

Usage: scripts/orchestrator.py run <account> probe_take
"""


KNIFE_SPOT = (3224, 3202, 0)
KNIFE_ID = 946


def knife_visible(bot):
    spot_items = bot.world.ground_items(*KNIFE_SPOT, radius=0, obj_id=KNIFE_ID)
    return bool(spot_items)


def run(bot, *args):
    bot.walk_to(*KNIFE_SPOT, timeout=6000)
    deadline = bot.world.ticks + 9000
    while bot.world.ticks < deadline and not knife_visible(bot):
        bot.pump(3)
    print("knife visible=%s inv=%s" % (knife_visible(bot), bot.inventory()), flush=True)
    if not knife_visible(bot):
        return 1
    if not bot.take(KNIFE_ID, timeout=3000):
        print("take failed", flush=True)
        return 1
    print("after take: %s" % (bot.inventory(),), flush=True)
    if not bot.drop(KNIFE_ID, timeout=1500):
        print("drop failed", flush=True)
        return 1
    print("after drop: %s" % (bot.inventory(),), flush=True)
    if not bot.take(KNIFE_ID, timeout=3000):
        print("re-take failed", flush=True)
        return 1
    print("after re-take: %s" % (bot.inventory(),), flush=True)
    return 0
