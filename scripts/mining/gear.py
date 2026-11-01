"""Mining gear helpers (split from run.py)."""

from scripts.mining.common import *
from scripts.mining.pack import *

def pick_id(bot, name):
    if name not in PICK_IDS:
        PICK_IDS[name] = obj_id(bot, name)
    return PICK_IDS[name]



def best_pickaxe(bot):
    held = set(held for (held, _count) in pack_counts(bot).values())
    for name in PICK_ORDER:
        try:
            if pick_id(bot, name) in held:
                return name
        except KeyError:
            continue
    return None



def known_pick_ids(bot):
    ids = set()
    for name in PICK_ORDER:
        try:
            ids.add(pick_id(bot, name))
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
    heads = [entry for entry in knowledge.all_objs() if entry and entry.name and entry.name.endswith("_pickaxehead")]
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
        if spots and bot.take(head_id, timeout=20):
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
        if best_pickaxe(bot) is not None:
            print("ok pickaxe reattached", flush=True)
            return True
    return False
