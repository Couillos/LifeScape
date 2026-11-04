"""Dev tool: reset F2P quest stages on a staff account (testing only).

~resetquests needs level 4 + a non-production server, so this mirrors the
server's reset_all_quests proc with ::setvar (level 3, same as give/tele).
Verifies each varp took the value by reading the confirmation messages.

Usage: scripts/orchestrator.py run <account> quest_reset
"""

# Stage varps zeroed by the server's reset_all_quests proc (F2P subset),
# plus qp so quest-point gates are earned honestly by the test order.
VARPS = (
    "spy cookquest demonstart doricquest dragon_goblin dragon_ned_hired "
    "dragon_oracle dragon_shield dragon_wall dragonquest dragonquestvar "
    "ernestlever haunted_manor_fountain_poisoned haunted goblinquest squire "
    "imp hunt hunt_store_employed prince_keystatus princequest prieststart "
    "rjquest runemysteries sheep blackarmgang phoenixgang vampire hetty qp"
).split()


def run(bot, *args):
    bot.intend("resetting F2P quest stages")
    for name in VARPS:
        seen = len(bot.world.messages)
        bot.cheat("setvar %s 0" % name)
        target = bot.world.ticks + 200
        confirmed = False
        while bot.world.ticks < target:
            bot.pump(2)
            if any(("set %s:" % name) in message.lower() for message in bot.world.messages[seen:]):
                confirmed = True
                break
        print("%s %s" % (name, "ok" if confirmed else "NOT-CONFIRMED"), flush=True)
    return 0
