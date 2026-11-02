"""Full early game on one account: guide, door, survival, items, take.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_chain.py [username]
"""

import sys
import time


from lifescape.client import Bot, Unreachable

GUIDE_CHOICES = ("Yes please.", "No, thank you.")
RECAP_EXITS = ("Nothing, thanks.", "No, thank you.")


def finish_dialog(bot, answers=(), timeout=180):
    deadline = time.time() + timeout
    answered = set()
    quiet = 0
    while time.time() < deadline:
        had_modal = bot.world.modal is not None
        before = len(bot.world.texts)
        bot.pump(5)
        texts = dict(bot.chat_texts())
        options = [text for text in texts.values() if text in answers or text in GUIDE_CHOICES]
        pending = [o for o in options if o not in answered]
        if pending:
            choice = next((a for a in answers if a in pending), pending[0])
            bot.chat_choose(choice)
            answered.add(choice)
            quiet = 0
        elif bot.world.modal:
            bot.chat_continue()
            quiet = 0
        elif len(bot.world.texts) != before or had_modal:
            quiet = 0
        else:
            quiet += 1
            if quiet >= 4:
                return True
    return False


def talk_and_finish(bot, name, answers=("No, thank you.", "Nothing, thanks."), timeout=120, near=None):
    if near is not None:
        try:
            bot.walk_to(near[0], near[1], open_doors=True)
        except (Unreachable, TimeoutError) as e:
            print("near walk failed: %s at %s" % (e, bot.position), flush=True)
            return False
    nid = None
    deadline = time.time() + 60
    while time.time() < deadline and nid is None:
        bot.pump(5)
        nid = bot.find_npc(name)
    if nid is None and near is not None:
        for sx, sz in ((near[0] - 2, near[1] - 2), (near[0] + 2, near[1] + 2)):
            try:
                bot.walk_to(sx, sz, open_doors=True, timeout=60)
            except (Unreachable, TimeoutError):
                continue
            deadline = time.time() + 20
            while time.time() < deadline and nid is None:
                bot.pump(5)
                nid = bot.find_npc(name)
            if nid is not None:
                break
    if nid is None:
        print("no %s nearby (at %s)" % (name, bot.position), flush=True)
        return False
    npc = bot.world.npcs[nid]
    bot.approach(npc["x"], npc["z"], open_doors=True)
    bot.talk_to(nid)
    deadline = time.time() + 20
    while not bot.world.modal and time.time() < deadline:
        bot.pump(5)
    finish_dialog(bot, answers)
    return True


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t01"
    log = open("/tmp/opencode/chain_%s.jsonl" % username, "w")
    trace = open("/tmp/opencode/trace_%s.txt" % username, "w")
    bot = Bot()
    real_pump = bot.pump

    def logged_pump(count=1):
        frames = real_pump(count)
        for op, name, body in frames:
            log.write('{"op": %d, "name": "%s", "hex": "%s"}\n' % (op, name, body.hex()))
        log.flush()
        return frames

    bot.pump = logged_pump
    try:
        bot.login(username)
        bot.connection.trace = trace
        print("seeds %s" % (bot.connection.seeds,), flush=True)
        bot.close_modal()
        bot.pump(5)
        print("start %s" % (bot.position,), flush=True)

        talk_and_finish(bot, "basics_instructor")
        print("guide talked", flush=True)
        bot.walk_to(3100, 3107, open_doors=True)
        print("through the door: %s" % (bot.position,), flush=True)

        ok = talk_and_finish(bot, "survival", near=(3103, 3095))
        print("survival talked: %s" % ok, flush=True)
        if not ok:
            return
        bot.tut_clickside(3)
        deadline = time.time() + 30
        while not bot.inventory() and time.time() < deadline:
            bot.pump(5)
        print("inventory: %s" % (bot.inventory(),), flush=True)
        if not bot.inventory():
            return
        dropped = sorted(bot.inventory())[0]
        bot.drop(dropped)
        print("after drop: %s" % (bot.inventory(),), flush=True)
        bot.take(dropped)
        print("after take: %s" % (bot.inventory(),), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
