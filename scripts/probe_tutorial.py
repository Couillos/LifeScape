"""Full tutorial start: talk to the guide, answer, walk out through the door.

Usage: scripts/orchestrator.py run <account> probe_tutorial
"""


def run(bot, *args):
    print("start %s" % (bot.position,), flush=True)
    bot.close_modal()
    bot.pump(5)
    deadline = bot.world.ticks + 1500
    while not bot.world.npcs and bot.world.ticks < deadline:
        bot.pump(5)
    nid = min(bot.world.npcs, key=lambda i: abs(bot.world.npcs[i]["x"] - bot.world.x) + abs(bot.world.npcs[i]["z"] - bot.world.z))
    bot.walk_to(bot.world.npcs[nid]["x"], bot.world.npcs[nid]["z"])
    bot.talk_to(nid)
    deadline = bot.world.ticks + 1500
    while not bot.world.modal and bot.world.ticks < deadline:
        bot.pump(5)
    answered = False
    deadline = bot.world.ticks + 6000
    while bot.world.ticks < deadline:
        bot.pump(5)
        if not answered and any("thank you" in text.lower() for _, text in bot.chat_texts()):
            bot.chat_choose("No, thank you.")
            answered = True
            print("answered no", flush=True)
        elif bot.world.modal:
            bot.chat_continue()
        else:
            break
    print("dialog done at %s" % (bot.position,), flush=True)
    bot.walk_to(3100, 3107, open_doors=True, timeout=9000)
    print("through the door: %s" % (bot.position,), flush=True)
