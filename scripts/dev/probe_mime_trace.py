"""Jetable: tick-stamped trace of mime anim/modal to understand the cycle."""

from scripts import randomevents


def run(bot, *args):
    bot.pump(10)
    last_anim = None
    last_modal = "init"
    end = bot.world.ticks + 120
    while bot.world.ticks < end and randomevents.in_mime(bot):
        bot.pump(1)
        try:
            slot = bot.find_npc("macro_mime")
        except Exception:
            slot = None
        anim = (bot.world.npcs.get(slot) or {}).get("anim") if slot is not None else None
        modal = bot.world.modal
        if anim != last_anim or modal != last_modal:
            print("t=%d anim=%s modal=%r" % (bot.world.ticks, anim, modal), flush=True)
            last_anim = anim
            last_modal = modal
    return 0
