"""Live probe: walk to Draynor bank, open a booth (op2), deposit one item
(op4 on bank_side:inv), withdraw it back (op1 on bank_main:inv).

Usage: scripts/orchestrator.py run <account> probe_draynor_bank
"""


from lifescape.client import Unreachable

from gamedata import comps as comps_module
from scripts.tutorial import common as tutorial_common

BACKPACK_COM = 3214


def pack_counts(bot):
    return {slot: (obj_id, count) for slot, (obj_id, count) in bot.world.inv.get(BACKPACK_COM, {}).items()}


def pack_total(bot, obj_id):
    return sum(count for (held, count) in pack_counts(bot).values() if held == obj_id)


def wait_pack(bot, obj_id, count, timeout=1500):
    deadline = bot.world.ticks + timeout
    while bot.world.ticks < deadline:
        bot.pump(5)
        if pack_total(bot, obj_id) >= count:
            return True
    return pack_total(bot, obj_id) >= count


def wait_pack_exact(bot, obj_id, count, timeout=1500):
    deadline = bot.world.ticks + timeout
    while bot.world.ticks < deadline:
        bot.pump(5)
        if pack_total(bot, obj_id) == count:
            return True
    return pack_total(bot, obj_id) == count


def run(bot, *args):
    deadline = bot.world.ticks + 4500
    while bot.world.ticks < deadline and not bot.world.inv.get(BACKPACK_COM):
        bot.pump(5)
    knife_id = tutorial_common.obj_id(bot, "knife")
    coins_id = tutorial_common.obj_id(bot, "coins")
    side_com = comps_module.com_id("bank_side:inv")
    main_com = comps_module.com_id("bank_main:inv")
    print("bank coms side=%s main=%s" % (side_com, main_com), flush=True)
    if pack_total(bot, knife_id) < 1:
        print("pack empty, will withdraw first", flush=True)
    bot.approach(3091, 3243, open_doors=True)
    print("at bank %s" % (bot.position,), flush=True)
    booth = tutorial_common.find_loc(bot, "bankbooth")
    print("booth %s" % (booth,), flush=True)
    booth_x, booth_z, booth_level, booth_loc = booth
    bot.approach(booth_x, booth_z, open_doors=True)
    bot.op_loc(2, booth_x, booth_z, booth_loc)
    end = bot.world.ticks + 1500
    while bot.world.ticks < end and bot.world.modal is None:
        bot.pump(5)
    print("bank modal=%s" % (bot.world.modal,), flush=True)
    if bot.world.modal is None:
        raise SystemExit("bank did not open")
    if pack_total(bot, knife_id) >= 1:
        slot = next(slot for slot, (held, _count) in pack_counts(bot).items() if held == knife_id)
        before = pack_total(bot, knife_id)
        bot.inv_button(4, knife_id, slot, side_com)
        end = bot.world.ticks + 1500
        while bot.world.ticks < end and pack_total(bot, knife_id) >= before:
            bot.pump(5)
        if pack_total(bot, knife_id) >= before:
            raise SystemExit("deposit failed")
        print("deposited, pack knives=%d" % pack_total(bot, knife_id), flush=True)
    bank_slots = bot.world.inv.get(main_com, {})
    bank_slot = next((slot for slot, (held, _count) in bank_slots.items() if held == knife_id), None)
    print("knife bank slot=%s" % (bank_slot,), flush=True)
    if bank_slot is None:
        raise SystemExit("knife not visible in bank")
    had = pack_total(bot, knife_id)
    bot.inv_button(1, knife_id, bank_slot, main_com)
    if not wait_pack(bot, knife_id, had + 1, timeout=30):
        raise SystemExit("withdraw failed")
    print("withdrew, pack knives=%d" % pack_total(bot, knife_id), flush=True)
    slot = next(slot for slot, (held, _count) in pack_counts(bot).items() if held == knife_id)
    before = pack_total(bot, knife_id)
    bot.inv_button(4, knife_id, slot, side_com)
    end = bot.world.ticks + 1500
    while bot.world.ticks < end and pack_total(bot, knife_id) >= before:
        bot.pump(5)
    if pack_total(bot, knife_id) >= before:
        raise SystemExit("re-deposit failed")
    print("re-deposited, pack knives=%d" % pack_total(bot, knife_id), flush=True)
    bot.close_modal()
    bot.pump(5)
    return 0
