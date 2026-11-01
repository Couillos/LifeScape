"""Live verification: ground spawn presence, take removal, respawn cooldown.

Slow on purpose (waits out a real ~60 s respawn). Run on demand, not in CI:
    ../../lostcity/.pypy/bin/pypy3 scripts/verify_live.py [username]

Fails fast with the server's own reason instead of timing out blindly.
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from lifescape.client import Bot

import gamedata as knowledge

SPAWN_X, SPAWN_Z, SPAWN_OBJ = 3224, 3202, 946


def exact(bot, obj_id):
    merged = dict(bot.world.static_objs)
    merged.update(bot.world.objs)
    found = {}
    for (key, oid), count in merged.items():
        if oid == obj_id and key == (SPAWN_X, SPAWN_Z, 0) and count:
            found[key] = count
    return found


def fail(reason):
    raise SystemExit("verify-live FAILED: %s" % reason)


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t48"
    known = [spawn for spawn in knowledge.all_obj_spawns(SPAWN_OBJ) if (spawn.x, spawn.z) == (SPAWN_X, SPAWN_Z)]
    assert known, "spawn missing from database"
    print("db: obj %d at (%d, %d) cooldown %d ticks" % (SPAWN_OBJ, SPAWN_X, SPAWN_Z, known[0].respawn_ticks), flush=True)

    bot = Bot()
    try:
        bot.login(username)
        bot.close_modal()
        bot.pump(3)
        # Clean slate far from the spawn: drop every test knife so the
        # backpack has room and old drops never pollute the tile.
        dropped_here = False
        for dest_x, dest_z in ((SPAWN_X + 35, SPAWN_Z), (SPAWN_X, SPAWN_Z + 35), (SPAWN_X - 35, SPAWN_Z)):
            try:
                bot.walk_to(dest_x, dest_z, 0, timeout=120, open_doors=True)
                dropped_here = True
                break
            except Exception:
                continue
        if not dropped_here:
            raise SystemExit("verify-live FAILED: no reachable drop zone")
        while bot.inventory().get(SPAWN_OBJ, 0) > 0:
            try:
                if not bot.drop(SPAWN_OBJ, timeout=15):
                    break
            except Exception:
                break
        bot.walk_to(SPAWN_X, SPAWN_Z, 0, timeout=180, open_doors=True)
        print("ok test knives dropped far away", flush=True)
        deadline = time.time() + 150
        while time.time() < deadline and not exact(bot, SPAWN_OBJ):
            bot.pump(3)
        if not exact(bot, SPAWN_OBJ):
            fail("spawn not visible in game")
        print("ok spawn visible in game", flush=True)
        had = bot.inventory().get(SPAWN_OBJ, 0)
        bot.take(obj_id=SPAWN_OBJ, timeout=60)
        if bot.inventory().get(SPAWN_OBJ, 0) <= had:
            fail("take did not add the item")
        print("ok taken", flush=True)
        deadline = time.time() + 60
        removed_at = None
        while time.time() < deadline and exact(bot, SPAWN_OBJ):
            bot.pump(2)
            for message in bot.world.messages[-3:]:
                lowered = message.lower()
                if "can't reach that" in lowered or "don't have enough" in lowered or "don't have room" in lowered:
                    fail("server refused the take: %s" % message.strip())
        if exact(bot, SPAWN_OBJ):
            fail("server did not remove the item (still visible 60 s after take)")
        removed_at = time.time()
        print("ok removal replicated", flush=True)
        deadline = time.time() + 150
        while time.time() < deadline and not exact(bot, SPAWN_OBJ):
            bot.pump(3)
        if not exact(bot, SPAWN_OBJ):
            fail("no respawn within 150 s")
        print("ok respawned after ~%ds (cooldown %d ticks holds live)" % (time.time() - removed_at, known[0].respawn_ticks), flush=True)
    finally:
        bot.close()
    print("verify-live OK", flush=True)


if __name__ == "__main__":
    main()
