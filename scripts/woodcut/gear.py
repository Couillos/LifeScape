"""Woodcut gear helpers (split from run.py)."""

from scripts.woodcut.common import *
from scripts.woodcut.pack import *

def axe_id(bot, name):
    if name not in AXE_IDS:
        AXE_IDS[name] = obj_id(bot, name)
    return AXE_IDS[name]



def best_axe(bot):
    held = set(held for (held, _count) in pack_counts(bot).values())
    for name in AXE_ORDER:
        try:
            if axe_id(bot, name) in held:
                return name
        except KeyError:
            continue
    return None



def known_axe_ids(bot):
    ids = set()
    for name in AXE_ORDER:
        try:
            ids.add(axe_id(bot, name))
        except KeyError:
            continue
    return ids



def reattach(bot):
    try:
        handle_id = obj_id(bot, HANDLE_NAME)
    except KeyError:
        return False
    if pack_total(bot, handle_id) < 1:
        return False
    heads = [entry for entry in knowledge.all_objs() if entry and entry.name and entry.name.endswith("_hatchethead")]
    for head in heads:
        try:
            head_id = obj_id(bot, head.name)
        except KeyError:
            continue
        if pack_total(bot, head_id) > 0:
            return use_handle_on_head(bot, handle_id, head_id)
    for head in heads:
        try:
            head_id = obj_id(bot, head.name)
        except KeyError:
            continue
        spots = bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, obj_id=head_id)
        if spots and bot.take(head_id, timeout=1000):
            return use_handle_on_head(bot, handle_id, head_id)
    return False



def use_handle_on_head(bot, handle_id, head_id):
    try:
        handle_slot = pack_slot(bot, handle_id)
        head_slot = pack_slot(bot, head_id)
    except KeyError:
        return False
    bot.op_held_u(handle_id, handle_slot, BACKPACK_COM, head_id, head_slot, BACKPACK_COM)
    target = bot.world.ticks + 50
    while bot.world.ticks < target:
        bot.pump(5)
        if best_axe(bot) is not None:
            print("ok axe reattached", flush=True)
            return True
    return False
