"""Dev tool: talk to an NPC and dump every modal text/option (staff testing).

Traces full dialogue flow for debugging quest branches. Never used by bots.

Usage: scripts/orchestrator.py run <account> probe_talk <npc> [answer ...]
Answers are substrings matched like solver keywords (first match wins,
else first option); use "CONTINUE" to force chat_continue rounds.
"""


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> probe_talk <npc> [answer ...]", flush=True)
        return 2
    name, answers = args[0], args[1:]
    nid = bot.find_npc(name)
    if nid is None:
        bot.pump(10)
        nid = bot.find_npc(name)
    if nid is None:
        print("no such npc in view: %r" % (name,), flush=True)
        return 1
    npc = bot.world.npcs[nid]
    try:
        bot.approach(npc["x"], npc["z"], radius=2, timeout=3000, open_doors=True)
    except Exception as error:
        print("approach failed: %s" % (error,), flush=True)
    nid = bot.find_npc(name) or nid
    bot.talk_to(nid)
    bot.pump(5)
    answered = set()
    for round_no in range(80):
        texts = [(com, text) for com, text in bot.chat_texts()]
        options = [(com, text) for com, text in bot.chat_options() if text not in answered]
        print("--- round %d modal=%s" % (round_no, bot.world.modal), flush=True)
        for com, text in texts[-6:]:
            print("  text %s: %r" % (com, text[:100]), flush=True)
        for com, text in options:
            print("  option %s: %r" % (com, text[:100]), flush=True)
        if bot.world.modal is None:
            print("modal closed, done", flush=True)
            print("live varps: %s" % (sorted((k, v) for k, v in bot.world.varps.items() if v)), flush=True)
            return 0
        if options:
            choice = None
            for keyword in answers:
                choice = next(((c, t) for c, t in options if keyword.lower() in t.lower()), None)
                if choice is not None:
                    break
            if choice is None:
                choice = options[0]
            print("click option %s %r" % (choice[0], choice[1][:60]), flush=True)
            bot.if_button(choice[0])
            answered.add(choice[1])
        else:
            bot.chat_continue()
        bot.pump(10)
    print("rounds exhausted", flush=True)
    return 1
