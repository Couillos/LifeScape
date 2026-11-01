"""Fishing shop helpers (split from run.py)."""

from scripts.fishing.common import *
from scripts.fishing.pack import *
from scripts.fishing.walk import *
from scripts.fishing.spots import *

def op_batches(amount, ops):
    batches = []
    for op, count in ops:
        while amount >= count:
            batches.append((op, count))
            amount -= count
    return batches



def sell_batches(amount):
    return op_batches(amount, ((4, 10), (3, 5), (2, 1)))



def withdraw_batches(amount):
    return op_batches(amount, ((2, 5), (1, 1)))



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



def tool_shop_stock(bot, obj_name):
    for shop in knowledge.all_shops():
        for stock in shop.stocks:
            if stock.obj == obj_name and stock.count > 0:
                name, spot = nearest_keeper(bot, shop.keepers)
                if name is not None:
                    return name, spot, stock.count
    return None, None, 0



def buy_stock(bot, main_com, obj_name, count):
    keeper, spot, _count = tool_shop_stock(bot, obj_name)
    if keeper is None:
        return 0
    if not bot.trade_to(keeper, spot):
        return 0
    try:
        wanted = obj_id(bot, obj_name)
    except KeyError:
        bot.close_modal()
        return 0
    before = pack_total(bot, wanted)
    if main_com not in bot.world.inv and not wait_com(bot, main_com):
        bot.close_modal()
        bot.pump(5)
        return 0
    shop_slots = bot.world.inv.get(main_com, {})
    slot = next((slot for slot, (held, _count) in shop_slots.items() if held == wanted), None)
    if slot is None:
        bot.close_modal()
        bot.pump(5)
        return 0
    for op, _count in sell_batches(count):
        bot.inv_button(op, wanted, slot, main_com)
        bot.pump(5)
    wait_pack_count(bot, wanted, before + 1, timeout=60)
    gained = pack_total(bot, wanted) - before
    bot.close_modal()
    bot.pump(5)
    return gained



def buy_tool(bot, main_com, tool_name):
    return buy_stock(bot, main_com, tool_name, 1) > 0



def obj_price(bot, name):
    found = knowledge.obj_by_name(name)
    return found[0].price if found else 0



def cheapest_upgrade(bot, fishing_level):
    current = best_tool(bot)
    current_level = next((tier.level for tier in knowledge.all_fish_tiers() if tier.tool == current), -1)
    candidates = []
    for tier in knowledge.all_fish_tiers():
        if tier.level <= current_level or tier.level > fishing_level:
            continue
        if tool_shop_stock(bot, tier.tool)[0] is None:
            continue
        candidates.append(tier)
    if not candidates:
        return None, 0
    candidates.sort(key=lambda tier: -tier.level)
    return [tier.tool for tier in candidates], min(obj_price(bot, tier.tool) for tier in candidates)



def sell_for_target(bot, shop_side_com, coins_id, target_coins):
    # Sell only the shortfall vs the bank: never stockpile sale coins.
    if target_coins <= 0:
        return 0
    fish_ids = {}
    for tier in knowledge.all_fish_tiers():
        for product in tier.products:
            try:
                fish_ids[product] = obj_id(bot, product)
            except KeyError:
                continue
    if not any(pack_total(bot, fish_id) > 0 for fish_id in fish_ids.values()):
        return 0
    keeper, spot = general_store(bot)
    if keeper is None:
        print("no general store found", flush=True)
        return 0
    if not bot.trade_to(keeper, spot):
        print("general store trade failed at %s" % (bot.position,), flush=True)
        return 0
    coins_before = pack_total(bot, coins_id)
    side_com = shop_side_com
    if side_com is None:
        bot.close_modal()
        return 0
    first_id = next((fish_id for fish_id in fish_ids.values() if pack_total(bot, fish_id) > 0), None)
    if first_id is None:
        bot.close_modal()
        bot.pump(5)
        return 0
    try:
        slot = pack_slot(bot, first_id)
    except KeyError:
        bot.close_modal()
        bot.pump(5)
        return 0
    bot.inv_button(2, first_id, slot, side_com)
    if not wait_pack_count(bot, coins_id, coins_before + 1):
        bot.close_modal()
        bot.pump(5)
        return 0
    unit = pack_total(bot, coins_id) - coins_before
    if unit <= 0:
        bot.close_modal()
        bot.pump(5)
        return 0
    while pack_total(bot, coins_id) < coins_before + target_coins:
        first_id = next((fish_id for fish_id in fish_ids.values() if pack_total(bot, fish_id) > 0), None)
        if first_id is None:
            break
        try:
            slot = pack_slot(bot, first_id)
        except KeyError:
            break
        count = pack_total(bot, first_id)
        need = coins_before + target_coins - pack_total(bot, coins_id)
        amount = min(count, max(1, (need + unit - 1) // unit))
        ops = sell_batches(amount)
        if not ops:
            break
        op, _count = ops[0]
        bot.inv_button(op, first_id, slot, side_com)
        bot.pump(5)
    gained = pack_total(bot, coins_id) - coins_before
    print("ok sold fish for %d coins" % gained, flush=True)
    bot.close_modal()
    bot.pump(5)
    return gained



def sell_junk(bot, side_com, coins_id, keep_ids, target_coins):
    if target_coins <= 0:
        return 0
    junk_ids = [
        held
        for (held, _count) in pack_counts(bot).values()
        if held not in keep_ids and held != coins_id and pack_total(bot, held) > 0
    ]
    if not junk_ids:
        return 0
    keeper, spot = general_store(bot)
    if keeper is None:
        print("no general store found", flush=True)
        return 0
    if not bot.trade_to(keeper, spot):
        print("junk sale trade failed at %s" % (bot.position,), flush=True)
        return 0
    coins_before = pack_total(bot, coins_id)
    for junk_id in junk_ids:
        while pack_total(bot, coins_id) < target_coins and pack_total(bot, junk_id) > 0:
            try:
                slot = pack_slot(bot, junk_id)
            except KeyError:
                break
            bot.inv_button(2, junk_id, slot, side_com)
            bot.pump(5)
            if pack_total(bot, coins_id) >= target_coins:
                break
    gained = pack_total(bot, coins_id) - coins_before
    print("ok sold junk for %d coins" % gained, flush=True)
    bot.close_modal()
    bot.pump(5)
    return gained
