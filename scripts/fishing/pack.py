"""Fishing pack helpers (split from run.py)."""

from scripts.fishing.common import *

def pack_counts(bot):
    return dict(bot.world.inv.get(BACKPACK_COM, {}))



def pack_total(bot, obj_id):
    return sum(count for (held, count) in pack_counts(bot).values() if held == obj_id)



def pack_slot(bot, obj_id):
    for slot, (held, _count) in pack_counts(bot).items():
        if held == obj_id:
            return slot
    raise KeyError("no %d in backpack" % obj_id)



def obj_id(bot, name):
    return C.obj_id(bot, name)



def free_pack_slots(bot):
    return BACKPACK_SIZE - len(pack_counts(bot))



def wait_pack_count(bot, obj_id, minimum, timeout=50):
    """Settle-wait: server inventory updates arrive ticks after the click."""
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if pack_total(bot, obj_id) >= minimum:
            return True
    return False



def wait_com(bot, com, timeout=50):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if com in bot.world.inv:
            return True
    return False



def learn_level(bot, known):
    if FISH_STAT in bot.world.stats:
        return bot.world.stats[FISH_STAT][0]
    return known



def dismiss_modal(bot, timeout=25):
    texts = " ".join(text for _com, text in bot.chat_texts()).lower()
    if bot.world.modal is None:
        return True
    if "congratulations" not in texts:
        print("modal texts: %s" % ([text[:80] for _com, text in list(bot.chat_texts())[-4:]]), flush=True)
    for _n in range(4):
        bot.chat_continue()
        bot.pump(5)
        if bot.world.modal is None:
            print("ok dialog dismissed", flush=True)
            return True
    target = bot.world.ticks + timeout
    while bot.world.ticks < target and bot.world.modal is not None:
        bot.pump(5)
    if bot.world.modal is None:
        print("ok dialog dismissed", flush=True)
        return True
    return False
