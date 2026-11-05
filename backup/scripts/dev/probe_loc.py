"""Dev tool: find static locs by debug name (staff testing).

Same source as find_loc (the bot's static tables with content debug
names), but global instead of radius-capped, listing every match. The
packed map only carries display names ("Door"), useless for quest locs.
Never used by bots.

Usage: scripts/orchestrator.py run <account> probe_loc <name> [cx] [cz] [radius] [level]
"""


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> probe_loc <name> [cx] [cz] [radius] [level]", flush=True)
        return 2
    names = [arg.lower() for arg in [args[0]]]
    cx = int(args[1]) if len(args) > 1 else bot.world.x
    cz = int(args[2]) if len(args) > 2 else bot.world.z
    radius = int(args[3]) if len(args) > 3 else 500
    level = int(args[4]) if len(args) > 4 else bot.world.level
    bot.ensure_nav()
    seen = dict(bot.world.static_locs)
    seen.update(getattr(bot.world, "static_actions", None) or {})
    found = 0
    for (x, z, lvl), loc in sorted(seen.items()):
        if loc is None or lvl != level:
            continue
        if abs(x - cx) + abs(z - cz) > radius:
            continue
        label = bot.loctypes["loc"][loc[0]]["name"] if loc[0] < len(bot.loctypes["loc"]) else ""
        if not any(name in (label or "").lower() for name in names):
            continue
        print("loc %s id=%d at %d,%d,%d shape=%d angle=%d" % (
            label, loc[0], x, z, lvl, loc[1], loc[2]), flush=True)
        found += 1
    print("%d match(es)" % found, flush=True)
    return 0
