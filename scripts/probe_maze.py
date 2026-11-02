"""Dev tool: dump dynamic locs around the bot (maze debugging).

Usage: scripts/orchestrator.py run <account> probe_maze [radius]
"""


def run(bot, *args):
    radius = int(args[0]) if args else 60
    bot.ensure_nav()
    bot.pump(10)
    rows = []
    for (x, z, level), loc in bot.world.locs.items():
        if loc is None:
            continue
        loc_id = loc[0]
        if level != bot.world.level:
            continue
        dist = abs(x - bot.world.x) + abs(z - bot.world.z)
        if dist > radius:
            continue
        entry = bot.loctypes["loc"][loc_id] if loc_id < len(bot.loctypes["loc"]) else None
        name = (entry["name"] if entry else "") or ""
        ops = [entry.get("op%d" % i) for i in range(1, 6)] if entry else []
        rows.append((dist, x, z, loc_id, name, [op for op in ops if op]))
    rows.sort()
    print("at %s dynamic locs within %d:" % (bot.position, radius), flush=True)
    for dist, x, z, loc_id, name, ops in rows[:60]:
        print("d=%d (%d,%d) id=%d %r ops=%s" % (dist, x, z, loc_id, name, ops), flush=True)
    # Planner internals: which doors does the router assume open?
    try:
        maze_doors = [s for s in (bot.wall_doors or [])
                      if 2880 <= s[0] <= 2943 and 4544 <= s[1] <= 4607 and s[2] == 0]
        print("wall_doors near maze: %d (total %d)" % (len(maze_doors), len(bot.wall_doors or [])), flush=True)
        print("blocked_doors: %d unsealed: %d" % (len(getattr(bot, "_blocked_doors", ())), len(getattr(bot, "unsealed_doors", ()))), flush=True)
        for s in maze_doors[:12]:
            print("door (%d,%d) shape=%d angle=%d id=%d blocked=%s" % (
                s[0], s[1], s[3], s[4], s[9], id(s) in (getattr(bot, "_blocked_doors", None) or ())), flush=True)
        bot._rebuild_planning()
        path = bot.router.find_path(bot.planning_map, bot.world.level, bot.world.x, bot.world.z, 2911, 4575)
        print("planning path to centre: %s" % (path if not path else (len(path), path[:4]),), flush=True)
    except Exception as err:
        print("planner dump failed: %r" % (err,), flush=True)
    return 0
