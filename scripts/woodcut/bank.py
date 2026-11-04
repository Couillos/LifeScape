"""Woodcut bank helpers (split from run.py)."""

from scripts.woodcut.common import *
from scripts.tutorial import common as tutorial_common

from lifescape import banks as banks_mod
from scripts.woodcut.pack import *
from scripts.woodcut.walk import *
from scripts.woodcut.spots import *
from scripts.woodcut.gear import *
from scripts.woodcut.shop import *

def open_bank(bot, bank):
    booth = tutorial_common.find_loc(bot, "bankbooth")
    booth_x, booth_z, booth_level, booth_loc = booth
    bot.approach(booth_x, booth_z, open_doors=True)
    bot.op_loc(2, booth_x, booth_z, booth_loc)
    target = bot.world.ticks + 50
    while bot.world.ticks < target and bot.world.modal is None:
        bot.pump(5)
    return bot.world.modal is not None



def bank_all_but(bot, side_com, keep_ids):
    print("depositing the load", flush=True)
    # The genie lamp is unbankable: depositing it always fails, so keep it.
    # The next randomevents.check() rubs it for XP instead.
    lamp = knowledge.objs.by_name("macro_genilamp")
    if lamp:
        keep_ids = set(keep_ids) | {lamp[0].obj_id}
    return bot.bank_deposit_all(side_com, keep_ids)



def bank_trip(bot, bank, side_com, main_com, keep_ids, withdraw=0, withdraw_logs=0, log_ids=()):
    randomevents.greet(bot)
    print("bank trip to (%d,%d) from %s" % (bank.x, bank.z, bot.position), flush=True)
    bot.intend("walking to the bank at (%d,%d)" % (bank.x, bank.z))
    # The bank commute can cross hunters (spiders, bears, jail guards):
    # one failed walk is usually a wandering attacker, so walk, back off,
    # and retry.
    opened = False
    for _attempt in range(3):
        try:
            stand = banks_mod.stand_for(bank.x, bank.z, bank.level) or (bank.x, bank.z)
            reached = walk_checked(bot, stand[0], stand[1], timeout=15000)
        except (Unreachable, TimeoutError):
            # Booth tiles sit inside walls; standing next to one is close
            # enough since open_bank approaches the booth itself.
            reached = abs(bot.world.x - bank.x) + abs(bot.world.z - bank.z) <= 8
        if reached is False:
            # walk_checked returns False only when hurt mid-walk.
            if retreat_along_trail(bot):
                wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
        if not reached:
            try:
                bot.approach(bank.x, bank.z, radius=6, timeout=6000, open_doors=True)
                reached = True
            except (Unreachable, TimeoutError):
                reached = False
        if not reached:
            print("bank trip: could not reach (%d,%d), retrying" % (bank.x, bank.z), flush=True)
            wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
            continue
        try:
            opened = open_bank(bot, bank)
        except Unreachable:
            opened = False
        if opened:
            break
        print("bank trip: bank did not open near %s, retrying" % (bot.position,), flush=True)
        wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
    if not opened:
        print("bank trip: bank did not open near %s" % (bot.position,), flush=True)
        return False, 0
    ok = bank_all_but(bot, side_com, keep_ids)
    if withdraw != 0:
        coins_id = obj_id(bot, COINS)
        target = bot.world.ticks + 25
        bank_slot = None
        while bot.world.ticks < target:
            bot.pump(5)
            bank_slots = bot.world.inv.get(main_com, {})
            bank_slot = next((slot for slot, (held, _count) in bank_slots.items() if held == coins_id), None)
            if bank_slot is not None:
                break
        if bank_slot is None:
            print("no coins visible in bank", flush=True)
        elif withdraw < 0:
            bot.inv_button(4, coins_id, bank_slot, main_com)
            bot.pump(5)
            print("withdrew all coins, pack now %d" % pack_total(bot, coins_id), flush=True)
        else:
            slot = bank_slot
            while withdraw >= 5:
                bot.inv_button(2, coins_id, slot, main_com)
                bot.pump(3)
                withdraw -= 5
            while withdraw > 0:
                bot.inv_button(1, coins_id, slot, main_com)
                bot.pump(3)
                withdraw -= 1
            bot.pump(5)
            print("withdrew coins, pack now %d" % pack_total(bot, coins_id), flush=True)
    if withdraw_logs > 0 and log_ids:
        remaining = withdraw_logs
        for logs_id in log_ids:
            while remaining > 0:
                bank_slots = bot.world.inv.get(main_com, {})
                slot = next((slot for slot, (held, count) in sorted(bank_slots.items()) if held == logs_id and count > 0), None)
                if slot is None:
                    break
                take = 5 if remaining >= 5 else 1
                bot.inv_button(2 if take == 5 else 1, logs_id, slot, main_com)
                bot.pump(5)
                remaining -= take
            if remaining <= 0:
                break
        print("withdrew logs, pack now holds %d" % sum(pack_total(bot, oid) for oid in log_ids), flush=True)
    bank_coins = sum(count for (held, count) in bot.world.inv.get(main_com, {}).values() if held == obj_id(bot, COINS))
    bot.close_modal()
    bot.pump(5)
    return ok, bank_coins



def restore_from_bank(bot, bank, side_bank, main_bank):
    """Withdraw a banked axe (or its handle+head) and reattach it.
    True when holding a whole axe after. Cheaper than rebuying."""
    if best_axe(bot) is not None:
        return True
    try:
        handle_id = obj_id(bot, HANDLE_NAME)
    except KeyError:
        return False
    head_ids = set()
    for entry in knowledge.all_objs():
        if entry and entry.name and entry.name.endswith("_hatchethead"):
            try:
                head_ids.add(obj_id(bot, entry.name))
            except KeyError:
                continue
    if not head_ids:
        return False
    try:
        opened = open_bank(bot, bank)
    except Unreachable:
        opened = False
    if not opened:
        return False
    if not wait_com(bot, main_bank, timeout=50):
        bot.close_modal()
        bot.pump(5)
        return False
    bank_slots = bot.world.inv.get(main_bank, {})
    axe_slot = next((slot for slot, (held, _count) in bank_slots.items() if held in known_axe_ids(bot)), None)
    if axe_slot is not None:
        axe_id = bank_slots[axe_slot][0]
        bot.inv_button(1, axe_id, axe_slot, main_bank)
        bot.pump(10)
        bot.close_modal()
        bot.pump(5)
        if best_axe(bot) is not None:
            print("ok withdrew banked axe", flush=True)
            return True
        return False
    bank_slots = bot.world.inv.get(main_bank, {})
    handle_slot = next((slot for slot, (held, _count) in bank_slots.items() if held == handle_id), None)
    head_slot = next((slot for slot, (held, _count) in bank_slots.items() if held in head_ids), None)
    if handle_slot is None or head_slot is None:
        bot.close_modal()
        bot.pump(5)
        return False
    bot.inv_button(1, handle_id, handle_slot, main_bank)
    bot.pump(5)
    bot.inv_button(1, bank_slots[head_slot][0], head_slot, main_bank)
    bot.pump(10)
    bot.close_modal()
    bot.pump(5)
    if reattach(bot):
        print("ok restored axe from banked pieces", flush=True)
        return True
    return False



def recover_axe(bot, side_bank, main_bank, main_shop, coins_id):
    if best_axe(bot) is not None:
        return True
    if reattach(bot):
        return True
    if pack_total(bot, coins_id) >= 16 and buy_attempt(bot, main_shop, "bronze_axe"):
        print("ok replacement axe bought", flush=True)
        return True
    bank = nearest_free_bank(bot)
    if bank is None:
        return False
    keep = {coins_id}
    ok, bank_coins = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw=max(0, 16 - pack_total(bot, coins_id)))
    if not ok:
        return False
    if restore_from_bank(bot, bank, side_bank, main_bank):
        print("ok replacement axe restored", flush=True)
        return True
    if pack_total(bot, coins_id) >= 16 and buy_attempt(bot, main_shop, "bronze_axe"):
        print("ok replacement axe bought", flush=True)
        return True
    return False



def nearest_free_bank(bot):
    gate = nearest_toll_gate(bot)
    best = None
    fallback = None
    for bank in knowledge.all_banks():
        if bank.members or bank.level != bot.world.level:
            continue
        dist = abs(bank.x - bot.world.x) + abs(bank.z - bot.world.z)
        if gate is not None and (bank.x >= gate[0]) != (bot.world.x >= gate[0]):
            # Crossing the toll fence costs 10 coins each way: only a
            # last resort, never the default bank.
            if fallback is None or dist < fallback[0]:
                fallback = (dist, bank)
            continue
        if best is None or dist < best[0]:
            best = (dist, bank)
    if best is not None:
        return best[1]
    return fallback[1] if fallback is not None else None



def nearest_toll_gate(bot):
    try:
        gate = tutorial_common.find_loc(bot, "border_gate_toll_left", "border_gate_toll_right")
    except Unreachable:
        return None
    return gate
