"""Item loop probe: door, survival dialog, tab click, drop with full trace.

Usage: scripts/orchestrator.py run <account> probe_items
"""


def drive(bot, answers, timeout=6000):
    deadline = bot.world.ticks + timeout
    while bot.world.ticks < deadline:
        bot.pump(5)
        texts = dict(bot.chat_texts())
        options = [t for t in texts.values() if t in answers or "thank you" in t.lower() or "Nothing" in t]
        if options:
            bot.chat_choose(next((a for a in answers if a in options), options[0]))
        elif bot.world.modal:
            bot.chat_continue()
        else:
            return True
    return False


def run(bot, *args):
    bot.close_modal()
    bot.pump(5)
    print("start %s inv=%s" % (bot.position, bot.inventory()), flush=True)
    bot.walk_to(3100, 3107, open_doors=True, timeout=6000)
    print("through door: %s" % (bot.position,), flush=True)
    nid = bot.find_npc("survival")
    print("instructor:", nid, flush=True)
    if nid is not None:
        npc = bot.world.npcs[nid]
        bot.approach(npc["x"], npc["z"], open_doors=True)
        bot.talk_to(nid)
        drive(bot, ["No, thank you.", "Nothing, thanks."])
        print("after dialog inv:", bot.inventory(), flush=True)
        bot.tut_clickside(3)
        deadline = bot.world.ticks + 1000
        while not bot.inventory() and bot.world.ticks < deadline:
            bot.pump(5)
        print("after tab: %s" % (bot.inventory(),), flush=True)
