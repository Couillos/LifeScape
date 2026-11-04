"""Two-sided item swap between partner bots (staff runs only).

Both accounts run this at the same time, standing next to each other,
with mirrored arguments: A gives what B wants and vice versa. Each side
drives its own window (offer, accept twice) and verifies its own result;
no chat channel is needed because both sides poll the live trade state.

One-sided handoffs pass "-" as the want item.

Usage: scripts/orchestrator.py run <account> trade_swap <partner> <give_item> <give_n> [want_item] [want_n]
"""

import gamedata as knowledge
from lifescape.client import Unreachable


def _obj(name):
    found = knowledge.objs.by_name(name)
    if not found:
        raise ValueError("unknown item %r" % name)
    return found[0].obj_id


def run(bot, *args):
    if len(args) < 3:
        print("usage: run <account> trade_swap <partner> <give_item> <give_n> [want_item] [want_n]", flush=True)
        return 2
    partner = args[0]
    give = _obj(args[1]) if args[1] != "-" else None
    give_n = int(args[2])
    want = _obj(args[3]) if len(args) > 3 and args[3] != "-" else None
    want_n = int(args[4]) if len(args) > 4 else 1
    bot.intend("trade with %s" % partner)
    if give is not None and bot.inventory().get(give, 0) < give_n:
        print("no %s x%d to give" % (args[1], give_n), flush=True)
        return 1
    deadline = bot.world.ticks + 6000
    while bot.world.ticks < deadline and bot.trade_modal() is None:
        if not bot.trade_request(partner, timeout=600):
            continue
    if bot.trade_modal() is None:
        print("no trade window with %s" % partner, flush=True)
        return 1
    print("trade window open with %s" % partner, flush=True)
    # Stage 1: offer, then accept whenever both offers look final. Any
    # offer change resets acceptions server-side, so keep re-accepting.
    while bot.world.ticks < deadline and bot.trade_modal() == "main":
        if give is not None and not bot.trade_offer(give, give_n, timeout=120):
            print("offer failed", flush=True)
            return 1
        partner_offer = bot.trade_partner_offer()
        if want is not None and partner_offer.get(want, 0) < want_n:
            bot.pump(10)
            continue
        bot.trade_accept(timeout=120)
        bot.pump(5)
    if bot.trade_modal() != "confirm":
        print("no confirm screen", flush=True)
        bot.trade_close()
        return 1
    print("confirm screen, accepting", flush=True)
    while bot.world.ticks < deadline and bot.trade_modal() is not None:
        bot.trade_accept(timeout=120)
        bot.pump(5)
    bot.pump(10)
    have = bot.inventory().get(want, 0) if want is not None else 0
    kept = bot.inventory().get(give, 0) if give is not None else 0
    print("after swap: want=%d kept-give=%d" % (have, kept), flush=True)
    if want is not None and have < want_n:
        print("swap failed: no %s" % args[3], flush=True)
        return 1
    return 0
