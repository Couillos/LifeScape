"""Woodcut pack helpers (split from run.py)."""

from scripts.woodcut.common import *
from scripts.tutorial import common as tutorial_common

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
    return tutorial_common.obj_id(bot, name)



def free_pack_slots(bot):
    return BACKPACK_SIZE - len(pack_counts(bot))



def wait_com(bot, com_id, timeout=25):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if bot.world.inv.get(com_id):
            return True
    return bool(bot.world.inv.get(com_id))



def learn_level(bot, known):
    if WC_STAT in bot.world.stats:
        return bot.world.stats[WC_STAT][0]
    return known



def dismiss_levelup(bot, timeout=25):
    texts = " ".join(text for _com, text in bot.chat_texts()).lower()
    if bot.world.modal is None or "congratulations" not in texts:
        return False
    bot.chat_continue()
    target = bot.world.ticks + timeout
    while bot.world.ticks < target and bot.world.modal is not None:
        bot.pump(5)
    if bot.world.modal is None:
        print("ok level-up dismissed", flush=True)
        return True
    return False
