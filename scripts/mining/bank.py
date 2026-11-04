"""Mining bank helpers (split from run.py)."""

from scripts.mining.common import *
from scripts.tutorial import common as tutorial_common

from lifescape import banks as banks_mod
from scripts.mining.pack import *
from scripts.mining.walk import *
from scripts.mining.spots import *
from scripts.mining.gear import *
from scripts.mining.shop import *

def stage_at_bank(bot, bank):
    """Stand on the safest walkable tile near the booth before banking,
    so a long deposit loop doesn't loiter inside a hunter's range."""
    cands = []
    for off_x in range(-6, 7):
        for off_z in range(-6, 7):
            cand_x, cand_z = bank.x + off_x, bank.z + off_z
            if not tile_walkable(bot, cand_x, cand_z):
                continue
            cands.append((not spot_is_safe(bot, cand_x, cand_z), abs(cand_x - bot.world.x) + abs(cand_z - bot.world.z), cand_x, cand_z))
    if not cands:
        return True
    cands.sort()
    _safe, _dist, best_x, best_z = cands[0]
    if (bot.world.x, bot.world.z) == (best_x, best_z):
        return True
    try:
        return walk_checked(bot, best_x, best_z, timeout=6000)
    except (Unreachable, TimeoutError):
        return False



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



def bank_trip(bot, bank, side_com, main_com, keep_ids, withdraw=0):
    randomevents.greet(bot)
    print("bank trip to (%d,%d) from %s" % (bank.x, bank.z, bot.position), flush=True)
    bot.intend("walking to the bank at (%d,%d)" % (bank.x, bank.z))
    for _attempt in range(3):
        try:
            stand = banks_mod.stand_for(bank.x, bank.z, bank.level) or (bank.x, bank.z)
            reached = walk_checked(bot, stand[0], stand[1], timeout=15000)
        except (Unreachable, TimeoutError):
            reached = False
        if reached:
            break
        if not retreat_along_trail(bot) and not compass_escape(bot, bank.x, bank.z):
            print("bank trip: stuck at %s" % (bot.position,), flush=True)
            return False, 0
        wait_hitpoints(bot, hitpoints(bot) + 1, timeout=200)
    if not reached:
        print("bank trip: could not reach (%d,%d)" % (bank.x, bank.z), flush=True)
        return False, 0
    if not stage_at_bank(bot, bank):
        print("bank trip: no safe staging at %s" % (bot.position,), flush=True)
        return False, 0
    if not open_bank(bot, bank):
        print("bank trip: bank did not open at %s" % (bot.position,), flush=True)
        return False, 0
    ok = bank_all_but(bot, side_com, keep_ids)
    if withdraw > 0:
        coins_id = obj_id(bot, COINS)
        target = bot.world.ticks + 25
        slot = None
        while bot.world.ticks < target:
            bot.pump(5)
            bank_slots = bot.world.inv.get(main_com, {})
            slot = next((slot for slot, (held, _count) in bank_slots.items() if held == coins_id), None)
            if slot is not None:
                break
        if slot is None:
            print("no coins visible in bank", flush=True)
        else:
            while withdraw >= 5:
                bot.inv_button(2, coins_id, slot, main_com)
                bot.pump(3)
                withdraw -= 5
            while withdraw > 0:
                bot.inv_button(1, coins_id, slot, main_com)
                bot.pump(3)
                withdraw -= 1
            bot.pump(5)
    bank_coins = sum(count for (held, count) in bot.world.inv.get(main_com, {}).values() if held == obj_id(bot, COINS))
    bot.close_modal()
    bot.pump(5)
    return ok, bank_coins



def restore_from_bank(bot, bank, side_bank, main_bank):
    """Withdraw a banked pickaxe (or its handle+head) and reattach it.
    True when holding a pickaxe after. Cheaper than rebuying."""
    if best_pickaxe(bot) is not None:
        return True
    try:
        handle_id = obj_id(bot, HANDLE_NAME)
    except KeyError:
        return False
    head_ids = set()
    for entry in knowledge.all_objs():
        if entry and entry.name and entry.name.endswith("_pickaxehead"):
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
    axe_slot = next((slot for slot, (held, _count) in bank_slots.items() if held in known_pick_ids(bot)), None)
    if axe_slot is not None:
        axe_id = bank_slots[axe_slot][0]
        bot.inv_button(1, axe_id, axe_slot, main_bank)
        bot.pump(10)
        bot.close_modal()
        bot.pump(5)
        if best_pickaxe(bot) is not None:
            print("ok withdrew banked pickaxe", flush=True)
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
        print("ok restored pickaxe from banked pieces", flush=True)
        return True
    return False



def recover_pickaxe(bot, side_bank, main_bank, main_shop, coins_id):
    if best_pickaxe(bot) is not None:
        return True
    if reattach(bot):
        return True
    # shortcut: rebuy a bronze pickaxe (obj cost 1) instead of trekking to
    # Nurmof in the dwarven mine for a repair; the upgrade loop rebuys
    # better pickaxes as soon as they are affordable.
    bronze_price = obj_price(bot, "bronze_pickaxe")
    if pack_total(bot, coins_id) >= bronze_price and buy_pickaxe(bot, main_shop, "bronze_pickaxe"):
        print("ok replacement pickaxe bought", flush=True)
        return True
    bank = nearest_free_bank(bot)
    if bank is None:
        return False
    keep = {coins_id}
    ok, bank_coins = bank_trip(bot, bank, side_bank, main_bank, keep, withdraw=max(0, bronze_price - pack_total(bot, coins_id)))
    if not ok:
        return False
    if restore_from_bank(bot, bank, side_bank, main_bank):
        print("ok replacement pickaxe restored", flush=True)
        return True
    if pack_total(bot, coins_id) >= bronze_price and buy_pickaxe(bot, main_shop, "bronze_pickaxe"):
        print("ok replacement pickaxe bought", flush=True)
        return True
    return False



def nearest_free_bank(bot):
    best = None
    for bank in knowledge.all_banks():
        if bank.members or bank.level != bot.world.level:
            continue
        dist = abs(bank.x - bot.world.x) + abs(bank.z - bot.world.z)
        if best is None or dist < best[0]:
            best = (dist, bank)
    return best[1] if best is not None else None



def nearest_toll_gate(bot):
    try:
        gate = tutorial_common.find_loc(bot, "border_gate_toll_left", "border_gate_toll_right")
    except Unreachable:
        return None
    return gate



def pick_home_bank(bot, tiers, mining_level):
    """Nearest F2P bank that actually has a safe mineable rock nearby.

    The closest bank (e.g. Lumbridge) may have no level-appropriate
    rocks; walking to a slightly further bank with ore beats failing.
    A booth inside a hunter's range (dark wizards camp some bank
    booths) loses to a safe booth further away. Banks across the toll gate
    lose to same-side banks (10 coins per crossing).
    """
    player_combat = bot.combat_level()
    gate = nearest_toll_gate(bot)
    candidates = [bank for bank in knowledge.all_banks() if not bank.members and bank.level == bot.world.level]
    ranked = []
    for bank in candidates:
        tier, spot, _use = pick_rock_spot(tiers, bank, mining_level, set(), player_combat)
        if tier is None or spot is None:
            continue
        booth_safe = spots_module.spot_safe(bank.x, bank.z, bank.level, threats(bot), player_combat)
        if gate is not None and (bank.x >= gate[0]) != (bot.world.x >= gate[0]):
            continue
        dist = abs(bank.x - bot.world.x) + abs(bank.z - bot.world.z)
        ranked.append((not booth_safe, dist, bank.x, bank.z, bank))
    if not ranked:
        return nearest_free_bank(bot)
    ranked.sort()
    return ranked[0][4]
