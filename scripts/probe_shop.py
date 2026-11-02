"""Live probe: sell 1 log to Lumbridge general store, buy 1 tinderbox,
then dump Bob's axe shop stock. Proves the shop buy/sell path.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_shop.py [username]
"""

import sys
import time


from lifescape.client import Bot, Unreachable

from scripts.tutorial import common as C

COM_SHOP_SIDE_INV = 3823
COM_SHOP_INV = 3900


def scan_npc(bot, npc_name, timeout=10):
    deadline = time.time() + timeout
    found = None
    while time.time() < deadline and found is None:
        bot.pump(5)
        found = bot.find_npc(npc_name)
    return found


def trade(bot, npc_name, near):
    if _scan_and_trade(bot, npc_name, near):
        return True
    print("trade relogging to resync NPCs", flush=True)
    try:
        bot.relogin()
        bot.close_modal()
        bot.pump(5)
    except Exception as error:
        print("relogin failed: %s" % error, flush=True)
        return False
    if _scan_and_trade(bot, npc_name, near):
        return True
    base = (bot.world.x, bot.world.z)
    for dx, dz in ((20, 0), (0, 0), (-20, 0)):
        if dx or dz:
            try:
                bot.walk_to(base[0] + dx, base[1] + dz, bot.world.level, open_doors=True, timeout=25)
            except Exception:
                pass
        if _scan_and_trade(bot, npc_name, None):
            return True
    print("trade giving up at %s" % (bot.position,), flush=True)
    return False


def _scan_and_trade(bot, npc_name, near):
    if near is not None and abs(bot.world.x - near[0]) + abs(bot.world.z - near[1]) > 4:
        try:
            bot.walk_to(near[0], near[1], bot.world.level, timeout=25, open_doors=True)
        except Exception as error:
            print("trade near walk failed: %s at %s" % (error, bot.position), flush=True)
    found = scan_npc(bot, npc_name)
    if found is None:
        print("trade scan: %d npcs at %s" % (len(bot.world.npcs), bot.position), flush=True)
        return False
    return _trade_from_best_tile(bot, found)


def _trade_from_best_tile(bot, slot, rounds=2):
    for _round in range(rounds):
        npc = bot.world.npcs.get(slot)
        if npc is None:
            bot.pump(5)
            continue
        tiles = sorted(
            [(npc["x"] + dx, npc["z"] + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
            key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
        )
        for tile_x, tile_z in tiles:
            try:
                bot.walk_to(tile_x, tile_z, bot.world.level, timeout=15, open_doors=True)
            except (Unreachable, TimeoutError):
                continue
            bot.op_npc(3, slot)
            end = time.time() + 8
            while not bot.world.modal and time.time() < end:
                bot.pump(5)
            if bot.world.modal is not None:
                return True
    return False


def dump_shop(bot):
    print("modal=%s" % (bot.world.modal,), flush=True)
    for com, slots in sorted(bot.world.inv.items()):
        print("  com=%s slots=%d" % (com, len(slots)), flush=True)


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t46"
    bot = Bot()
    try:
        bot.login(username, username)
        deadline = time.time() + 90
        while time.time() < deadline and not bot.inventory():
            bot.pump(5)
        logs_id = C.obj_id(bot, "logs")
        coins_id = C.obj_id(bot, "coins")
        tinder_id = C.obj_id(bot, "tinderbox")
        print("start inv logs=%d coins=%d" % (bot.inventory().get(logs_id, 0), bot.inventory().get(coins_id, 0)), flush=True)
        if not trade(bot, "generalshopkeeper", (3209, 3246, 0)):
            return 1
        dump_shop(bot)
        log_com, log_slot = C.inv_slot(bot, logs_id)
        bot.inv_button(2, logs_id, log_slot, COM_SHOP_SIDE_INV)
        bot.pump(10)
        print("after sell logs=%d coins=%d" % (bot.inventory().get(logs_id, 0), bot.inventory().get(coins_id, 0)), flush=True)
        shop_slots = bot.world.inv.get(COM_SHOP_INV, {})
        buy_slot = next((slot for slot, (obj_id, _count) in shop_slots.items() if obj_id == tinder_id), None)
        print("tinderbox shop slot=%s" % buy_slot, flush=True)
        if buy_slot is not None:
            bot.inv_button(2, tinder_id, buy_slot, COM_SHOP_INV)
            bot.pump(10)
            print("after buy coins=%d tinder=%d" % (bot.inventory().get(coins_id, 0), bot.inventory().get(tinder_id, 0)), flush=True)
        bot.close_modal()
        bot.pump(5)
        if not trade(bot, "bob", (3232, 3202, 0)):
            return 1
        dump_shop(bot)
        for slot, (obj_id, count) in sorted(bot.world.inv.get(COM_SHOP_INV, {}).items()):
            entry = bot._loctypes["obj"][obj_id] if obj_id < len(bot._loctypes["obj"]) else None
            print("  stock %s x%d" % ((entry["name"] if entry else obj_id), count), flush=True)
        bot.close_modal()
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
