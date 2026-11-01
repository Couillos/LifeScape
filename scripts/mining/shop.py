"""Mining shop helpers (split from run.py)."""

from scripts.mining.common import *
from scripts.mining.pack import *
from scripts.mining.spots import *

def sell_batches(amount):
    batches = []
    while amount >= 10:
        batches.append((4, 10))
        amount -= 10
    while amount >= 5:
        batches.append((3, 5))
        amount -= 5
    while amount > 0:
        batches.append((2, 1))
        amount -= 1
    return batches



def nearest_keeper(bot, keepers):
    best = None
    for keeper in keepers:
        for spawn in knowledge.all_npc_spawns(keeper.npc_id):
            if spawn.level != bot.world.level:
                continue
            if abs(spawn.x - bot.world.x) + abs(spawn.z - bot.world.z) > TREK_LIMIT:
                continue
            dist = abs(spawn.x - bot.world.x) + abs(spawn.z - bot.world.z)
            if best is None or dist < best[0]:
                best = (dist, keeper.npc, (spawn.x, spawn.z, spawn.level))
    if best is None:
        return None, None
    return best[1], best[2]



def general_store(bot):
    keepers = []
    for shop in knowledge.all_shops():
        if "general" not in shop.name.lower():
            continue
        keepers.extend(shop.keepers)
    return nearest_keeper(bot, keepers)



def pickaxe_shop_stock(bot, obj_name):
    for shop in knowledge.all_shops():
        for stock in shop.stocks:
            if stock.obj == obj_name and stock.count > 0:
                name, spot = nearest_keeper(bot, shop.keepers)
                if name is not None:
                    return name, spot, stock.count
    return None, None, 0



def buy_pickaxe(bot, main_shop, pick_name):
    keeper, spot, _count = pickaxe_shop_stock(bot, pick_name)
    if keeper is None:
        return False
    if not bot.trade_to(keeper, spot):
        return False
    wanted_id = obj_id(bot, pick_name)
    shop_slots = bot.world.inv.get(main_shop, {})
    slot = next((slot for slot, (held, _count) in shop_slots.items() if held == wanted_id), None)
    if slot is None:
        bot.close_modal()
        bot.pump(5)
        return False
    bot.inv_button(2, wanted_id, slot, main_shop)
    bot.pump(10)
    ok = pack_total(bot, wanted_id) > 0
    bot.close_modal()
    bot.pump(5)
    if not ok:
        print("could not afford %s" % pick_name, flush=True)
    return ok



def cheapest_upgrade(bot, mining_level):
    # shortcut: only surface-reachable shops are considered, so only a
    # bronze pickaxe (Bob, Lumbridge) is ever in stock range; iron and up
    # sit with Nurmof in the dwarven mine (stairs down, walk_to cannot
    # change areas). Upgrade when the bot learns stairs/ladder traversal.
    current = best_pickaxe(bot)
    current_rank = PICK_ORDER.index(current) if current in PICK_ORDER else len(PICK_ORDER)
    candidates = [
        name
        for name in PICK_ORDER[:current_rank]
        if mining_level >= PICK_REQS.get(name, 99) and pickaxe_shop_stock(bot, name)[0] is not None
    ]
    if not candidates:
        return None, 0
    return candidates, min(obj_price(bot, name) for name in candidates)



def obj_price(bot, name):
    found = knowledge.obj_by_name(name)
    return found[0].price if found else 0



def sell_for_target(bot, shop_side_com, coins_id, target_coins):
    # Sell only the shortfall vs the bank: never stockpile sale coins.
    if target_coins <= 0:
        return 0
    keeper, spot = general_store(bot)
    if keeper is None:
        print("no general store found", flush=True)
        return 0
    if not bot.trade_to(keeper, spot):
        print("general store trade failed", flush=True)
        return 0
    coins_before = pack_total(bot, coins_id)
    ore_id = None
    for tier in usable_tiers():
        try:
            candidate = obj_id(bot, tier.product)
        except KeyError:
            continue
        if pack_total(bot, candidate) > 0:
            ore_id = candidate
            break
    if ore_id is None:
        bot.close_modal()
        bot.pump(5)
        return 0
    try:
        slot = pack_slot(bot, ore_id)
    except KeyError:
        bot.close_modal()
        bot.pump(5)
        return 0
    bot.inv_button(2, ore_id, slot, shop_side_com)
    bot.pump(10)
    unit = pack_total(bot, coins_id) - coins_before
    if unit <= 0:
        bot.close_modal()
        bot.pump(5)
        return 0
    while pack_total(bot, coins_id) < target_coins:
        ore_id = None
        for tier in usable_tiers():
            try:
                candidate = obj_id(bot, tier.product)
            except KeyError:
                continue
            if pack_total(bot, candidate) > 0:
                ore_id = candidate
                break
        if ore_id is None:
            break
        try:
            slot = pack_slot(bot, ore_id)
        except KeyError:
            break
        count = pack_total(bot, ore_id)
        need = target_coins - pack_total(bot, coins_id)
        amount = min(count, max(1, (need + unit - 1) // unit))
        ops = sell_batches(amount)
        if not ops:
            break
        op, _count = ops[0]
        bot.inv_button(op, ore_id, slot, shop_side_com)
        bot.pump(5)
    gained = pack_total(bot, coins_id) - coins_before
    print("ok sold ore for %d coins" % gained, flush=True)
    bot.close_modal()
    bot.pump(5)
    return gained
