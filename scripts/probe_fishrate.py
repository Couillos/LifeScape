"""Measure fishing catch rate: single op vs re-clicking every 12 ticks.

Usage: scripts/orchestrator.py run <account> probe_fishrate [once|reclick]
"""


from scripts import randomevents

LIVE_RADIUS = 40


def nearest_fish(bot):
    best = None
    for slot, npc in bot.world.npcs.items():
        entry = bot.loctypes["npc"][npc["type"]] if npc["type"] < len(bot.loctypes["npc"]) else None
        name = (entry["name"] if entry else "") or ""
        if "saltfish" not in name and "freshfish" not in name:
            continue
        dist = abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z)
        if dist > LIVE_RADIUS:
            continue
        if best is None or dist < best[0]:
            best = (dist, slot, npc["x"], npc["z"], name)
    return best


def fish_count(bot):
    total = 0
    for _slot, (held, count) in bot.world.inv.get(3214, {}).items():
        entry = bot.loctypes["obj"][held] if held < len(bot.loctypes["obj"]) else None
        name = (entry["name"] if entry else "") or ""
        if "raw_" in name or name in ("shrimp",):
            total += count
    return total


def run(bot, *args):
    mode = args[0] if len(args) > 0 else "once"
    bot.ensure_nav()
    for _i in range(10):
        bot.pump(5)
    found = nearest_fish(bot)
    if found is None:
        print("no live fish npc visible, waiting", flush=True)
        for _i in range(30):
            for _j in range(10):
                bot.pump(5)
            found = nearest_fish(bot)
            if found is not None:
                break
    if found is None:
        print("still nothing live", flush=True)
        return 1
    _dist, slot, fx, fz, name = found
    print("op on %s at (%d,%d) mode=%s" % (name, fx, fz, mode), flush=True)
    start = fish_count(bot)
    tick0 = bot.world.ticks
    bot.op_npc(1, slot)
    last_click = bot.world.ticks
    last_greet = bot.world.ticks
    last_pos = bot.position
    end = bot.world.ticks + 9000
    while bot.world.ticks < end:
        bot.pump(2)
        if bot.world.ticks - last_greet > 30:
            last_greet = bot.world.ticks
            randomevents.greet(bot)
        if bot.position != last_pos:
            print("moved to %s at tick %d" % (bot.position, bot.world.ticks - tick0), flush=True)
            last_pos = bot.position
        if mode == "reclick" and bot.world.ticks - last_click >= 12:
            last_click = bot.world.ticks
            found = nearest_fish(bot)
            if found is not None:
                bot.op_npc(1, found[1])
                print("re-click at tick %d" % (bot.world.ticks - tick0), flush=True)
    print("fish caught: %d in %d ticks at %s" % (fish_count(bot) - start, bot.world.ticks - tick0, bot.position), flush=True)
    return 0
