"""Fishing bank helpers (split from run.py)."""

from scripts.fishing.common import *
from scripts.tutorial import common as tutorial_common

from lifescape import banks as banks_mod
from scripts.fishing.pack import *
from scripts.fishing.walk import *
from scripts.fishing.spots import *
from scripts.fishing.gear import *
from scripts.fishing.shop import *

def open_bank(bot, bank):
    booth_x, booth_z, booth_level, booth_loc = tutorial_common.find_loc(bot, "bankbooth")
    bot.approach(booth_x, booth_z, open_doors=True)
    bot.op_loc(2, booth_x, booth_z, booth_loc)
    target = bot.world.ticks + 50
    while bot.world.ticks < target and bot.world.modal is None:
        bot.pump(5)
    return bot.world.modal is not None



def bank_all_but(bot, side_com, keep_ids):
    print("depositing the load", flush=True)
    return bot.bank_deposit_all(side_com, keep_ids)



def bank_trip(bot, bank, side_com, main_com, keep_ids, withdraw=0, withdraw_tool=None):
    randomevents.greet(bot)
    print("bank trip to (%d,%d) from %s" % (bank.x, bank.z, bot.position), flush=True)
    bot.intend("walking to the bank at (%d,%d)" % (bank.x, bank.z))
    trip_hitpoints = hitpoints(bot)
    for _attempt in range(2):
        try:
            stand = banks_mod.stand_for(bank.x, bank.z, bank.level) or (bank.x, bank.z)
            reached = walk_checked(bot, stand[0], stand[1], timeout=15000)
        except Unreachable:
            reached = False
        if reached:
            break
        if not retreat_along_trail(bot, max_back=5) and not compass_escape(bot):
            print("bank trip: stuck at %s" % (bot.position,), flush=True)
            return False, 0
        wait_hitpoints(bot, trip_hitpoints, timeout=200)
    if not reached:
        print("bank trip: could not reach (%d,%d)" % (bank.x, bank.z), flush=True)
        return False, 0
    if not open_bank(bot, bank):
        print("bank trip: bank did not open at %s" % (bot.position,), flush=True)
        return False, 0
    if not wait_com(bot, main_com):
        print("bank trip: no bank snapshot at %s" % (bot.position,), flush=True)
        bot.close_modal()
        return False, 0
    ok = bank_all_but(bot, side_com, keep_ids)
    if withdraw_tool is not None:
        bank_slots = bot.world.inv.get(main_com, {})
        slot = next((slot for slot, (held, _count) in bank_slots.items() if held == withdraw_tool), None)
        if slot is not None:
            before = pack_total(bot, withdraw_tool)
            bot.inv_button(1, withdraw_tool, slot, main_com)
            wait_pack_count(bot, withdraw_tool, before + 1)
    if withdraw > 0:
        coins_id = obj_id(bot, COINS)
        before = pack_total(bot, coins_id)
        bank_slots = bot.world.inv.get(main_com, {})
        slot = next((slot for slot, (held, _count) in bank_slots.items() if held == coins_id), None)
        if slot is not None:
            for op, _count in withdraw_batches(withdraw):
                bot.inv_button(op, coins_id, slot, main_com)
                bot.pump(3)
            wait_pack_count(bot, coins_id, before + 1)
            bot.pump(5)
    bank_coins = sum(count for (held, count) in bot.world.inv.get(main_com, {}).values() if held == obj_id(bot, COINS))
    bot.close_modal()
    bot.pump(5)
    return ok, bank_coins



def recover_tool(bot, side_bank, main_bank, main_shop, tool_name, coins_id, bank):
    try:
        tool_id = obj_id(bot, tool_name)
    except KeyError:
        return False
    if pack_total(bot, tool_id) > 0:
        return True
    # The big-fish event knocks the tool onto the ground: pick it back up.
    try:
        if bot.take(tool_id, timeout=1500):
            print("ok tool recovered from ground", flush=True)
            return True
    except Unreachable:
        pass
    keep = {coins_id}
    keep.update(held for (held, _count) in pack_counts(bot).values() if held in known_tool_ids(bot))
    ok, _bank_coins = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw_tool=tool_id)
    if not ok:
        return False
    if pack_total(bot, tool_id) > 0:
        print("ok tool withdrawn from bank", flush=True)
        return True
    return pack_total(bot, coins_id) >= obj_price(bot, tool_name) and buy_tool(bot, main_shop, tool_name)



def nearest_free_bank(bot):
    return nearest_free_bank_at(bot.world.x, bot.world.z, bot.world.level, bot)



def ensure_gear(bot, tier, side_shop, main_shop, coins_id, side_bank, main_bank, bank):
    try:
        tool_id = obj_id(bot, tier.tool)
    except KeyError:
        return False
    if pack_total(bot, tool_id) < 1 and not recover_tool(bot, side_bank, main_bank, main_shop, tier.tool, coins_id, bank):
        if pack_total(bot, coins_id) < obj_price(bot, tier.tool):
            sell_for_target(bot, side_shop, coins_id, obj_price(bot, tier.tool) - pack_total(bot, coins_id))
        if not buy_tool(bot, main_shop, tier.tool):
            keep = {coins_id}
            keep.update(held for (held, _count) in pack_counts(bot).values() if held in known_tool_ids(bot))
            ok, _bank_coins = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw=obj_price(bot, tier.tool))
            if not ok or not buy_tool(bot, main_shop, tier.tool):
                return False
    if tier.bait:
        try:
            bait_id = obj_id(bot, tier.bait)
        except KeyError:
            return False
        if pack_total(bot, bait_id) < BAIT_MIN and not restock_bait(bot, tier, side_shop, main_shop, coins_id, side_bank, main_bank, bank):
            return "nobait"
    return pack_total(bot, tool_id) > 0



def restock_bait(bot, tier, side_shop, main_shop, coins_id, side_bank, main_bank, bank):
    if not tier.bait:
        return True
    try:
        bait_id = obj_id(bot, tier.bait)
    except KeyError:
        return False
    need = BAIT_WANT - pack_total(bot, bait_id)
    if need <= 0:
        return True
    price = obj_price(bot, tier.bait)
    shortfall = need * price - pack_total(bot, coins_id)
    if shortfall > 0:
        sell_for_target(bot, side_shop, coins_id, shortfall)
    if pack_total(bot, coins_id) < price:
        keep = {held for (held, _count) in pack_counts(bot).values() if held in known_tool_ids(bot)}
        ok, bank_coins = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw=min(need * price, 5000))
        if not ok:
            return False
        shortfall = need * price - pack_total(bot, coins_id)
        if shortfall > 0 and bank_coins <= 0:
            sell_for_target(bot, side_shop, coins_id, shortfall)
    bought = buy_stock(bot, main_shop, tier.bait, min(need, pack_total(bot, coins_id) // price if price else need))
    return pack_total(bot, bait_id) >= BAIT_MIN or bought > 0
