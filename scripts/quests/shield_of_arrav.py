"""Requirement check and completion: Shield of Arrav.

Gang choice: Phoenix. A solo bot can join the Phoenix Gang alone (Reldo ->
book -> Baraek -> Straven -> kill Jonny the Beard -> hand over the
intelligence report), while the Black Arm route needs a Phoenix insider to
steal two crossbows from the weapon depot first. The opposite half still
needs a partner: each gang's loot store only yields its own half, and the
other gang's HQ door stays shut, so half2 here comes from a Black Arm
partner (drop/trade) or the bank, which the requirement check already
proves before solve() runs.

Needs from the bank (besides the two halves the check already proves):
20 coins for Baraek's bribe, topped up here when the bank has them.
"""



import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.client import Unreachable
from lifescape.waits import wait_ticks
from scripts.quests import common as quest_common
from scripts.quests import solver
from scripts.quests.common import run as run_requirements
from scripts.tutorial import common as tutorial_common

import os

from lifescape import save as save_mod

QUEST = quest_requirements.by_key("blackarmgang")

# Save-backed stages: fresh logins are blind to live varps (gang varps are
# not transmitted), so gates consult the persisted stage too (same as BKF).
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or ""
    if not account:
        return {}
    try:
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _varp(bot, name):
    varp = knowledge.varp_id_by_name(name)
    return max(bot.world.varps.get(varp, 0), _SAVED_VARPS.get(varp, 0))

_GANG = None


def run(bot, *args):
    global _GANG
    _GANG = args[0].lower() if args else None
    return run_requirements(bot, QUEST, solve=solve)

PHOENIX = knowledge.varp_id_by_name("phoenixgang")
BLACKARM = knowledge.varp_id_by_name("blackarmgang")
PHOENIX_JOINED = 9
PHOENIX_DONE = 10
BLACKARM_JOINED = 3
BLACKARM_DONE = 4
BLACKARM_SPOKEN = 2

# Server map bug: the whole Phoenix HQ (Straven, doors, ladder) spawns at
# (3246,9780), unreachable on foot. Staff positioning (the cheat channel
# is staff-only server-side) for Straven's two talks and the way back;
# everything else walks. Never used for any other positioning.
STRAVEN_TILE = (3246, 9780, 0)
VARROCK_RETURN = (3225, 3420, 0)


def _staff_goto(bot, where):
    """Teleport to a tile and verify the landing (server-bug workaround)."""
    x, z, level = where[0], where[1], where[2] if len(where) > 2 else 0
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    target = bot.world.ticks + 100
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            return
    raise solver.SolverError("staff teleport did not land at %s" % (where,))


def _join_blackarm(bot):
    """One Black Arm join step per run (same non-transmitted stages).

    The two crossbows come from a Phoenix insider (trade), not from the
    depot: the depot key is Phoenix-only. Without them this stops with a
    pairing error instead of grinding the depot door.
    """
    stage = _varp(bot, "blackarmgang")
    if stage < 1:
        solver.talk_like(bot, "tramp", ["alleyway", "join"])
        return
    if stage == 1:
        solver.talk_like(bot, "katrine", ["place", "black arm", "tramp",
                                          "member", "try", "no problem"])
        return
    crossbow = solver.obj_id("phoenix_crossbow")
    if stage == BLACKARM_SPOKEN:
        if bot.inventory().get(crossbow, 0) < 2:
            raise solver.SolverError("need 2 phoenix crossbows: pair with a Phoenix insider to trade them")
        solver.talk_like(bot, "katrine", ["crossbows", "yes"])
        return
    if stage != BLACKARM_JOINED:
        raise solver.SolverError("Black Arm join did not finish")


def _ensure_coins(bot, amount=20):
    """Baraek wants 20 coins; top up from the bank when it has them."""
    coins = solver.obj_id("coins")
    if bot.inventory().get(coins, 0) >= amount:
        return
    try:
        bank = quest_common.open_nearest_bank(bot)
    except Exception as error:
        raise solver.SolverError("need %d coins for Baraek: %s" % (amount, error))
    try:
        if bank.get(coins, 0) > 0:
            bot.bank_withdraw(coins, min(amount, bank.get(coins, 0)))
    finally:
        solver.close_bank(bot)
    if bot.inventory().get(coins, 0) < amount:
        raise solver.SolverError("need %d coins for Baraek" % amount)


def _read_book(bot):
    """Take the Shield of Arrav book from the palace bookcase and read it."""
    book = solver.obj_id("the_shield_of_arrav")
    if bot.inventory().get(book, 0) < 1:
        solver.operate_loc(bot, ("questbookcase",), 2)
        if bot.inventory().get(book, 0) < 1:
            raise solver.SolverError("palace bookcase did not yield the book")
    try:
        com, slot = tutorial_common.inv_slot(bot, book)
    except KeyError:
        raise solver.SolverError("the Shield of Arrav book is not in the backpack")
    bot.op_held(1, book, slot, com)
    wait_ticks(bot, 10)
    bot.close_modal()
    bot.pump(5)


def _join_phoenix(bot):
    """One join step per run: gang varps are never transmitted, so each
    run acts on the save stage and ends (logout persists the next one).
    A silent step simply repeats next run; only hard failures raise."""
    stage = _varp(bot, "phoenixgang")
    if stage < 1:
        solver.talk_like(bot, "reldo", ["quest", "search"])
        return
    if stage == 1:
        _read_book(bot)
        return
    if stage == 2:
        solver.talk_like(bot, "reldo", ["phoenix", "read", "shield"])
        return
    if stage == 3:
        _ensure_coins(bot)
        solver.talk_like(bot, "baraek", ["phoenix", "have 20", "okay"])
        return
    if stage == 4:
        _staff_goto(bot, STRAVEN_TILE)
        solver.talk_like(bot, "straven", ["know who you are", "offer", "services"])
        _staff_goto(bot, VARROCK_RETURN)
        return
    report = solver.obj_id("intelligence_report")
    if stage == 8:
        if bot.inventory().get(report, 0) < 1:
            if not solver.attack(bot, ("jonny_the_beard",), timeout=600):
                raise solver.SolverError("could not kill Jonny the Beard")
            solver.take_near(bot, "intelligence_report", timeout=120)
            if bot.inventory().get(report, 0) < 1:
                raise solver.SolverError("Jonny did not drop the intelligence report")
        _staff_goto(bot, STRAVEN_TILE)
        solver.talk_like(bot, "straven", ["report", "intelligence", "join"])
        _staff_goto(bot, VARROCK_RETURN)
        return
    if stage != PHOENIX_JOINED:
        raise solver.SolverError("Phoenix join did not finish")


def _loot_inside(bot, door, closed, opened, closed_op, opened_op):
    """Search our own gang's loot store, entering first unless inside."""
    try:
        tutorial_common.find_loc(bot, closed, opened)
    except Unreachable:
        solver.operate_loc(bot, (door,), 1)
    solver.operate_loc(bot, (closed,), closed_op)
    solver.operate_loc(bot, (opened,), opened_op)
    solver.operate_loc(bot, (door,), 1)


def _ensure_halves(bot):
    """Top up our own half from our gang store; the other half is a partner's."""
    half1 = solver.obj_id("arravshield1")
    half2 = solver.obj_id("arravshield2")
    if bot.inventory().get(half1, 0) < 1 and _varp(bot, "phoenixgang") >= PHOENIX_JOINED:
        _loot_inside(bot, "phoenixdoor", "phoenixshutchest", "phoenixopenchest", 1, 1)
    if bot.inventory().get(half2, 0) < 1 and _varp(bot, "blackarmgang") >= BLACKARM_JOINED:
        _loot_inside(bot, "blackarmdoor", "blackarmcupboardshut", "blackarmcupboardopen", 1, 2)
    if bot.inventory().get(half1, 0) < 1 or bot.inventory().get(half2, 0) < 1:
        missing = "arravshield1" if bot.inventory().get(half1, 0) < 1 else "arravshield2"
        try:
            solver.take_near(bot, missing, timeout=60)
        except solver.SolverError:
            raise solver.SolverError("no %s: pair with the other gang to trade halves" % missing)
    if bot.inventory().get(half1, 0) < 1 or bot.inventory().get(half2, 0) < 1:
        raise solver.SolverError("still missing a shield half after the partner pickup")


def solve(bot):
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    if _varp(bot, "phoenixgang") == PHOENIX_DONE or _varp(bot, "blackarmgang") == BLACKARM_DONE:
        return
    black = _varp(bot, "blackarmgang")
    phoenix = _varp(bot, "phoenixgang")
    # One join step per run (logout persists the next stage). A join step
    # ends the run: continuing on a stale stage would replay entry walks
    # and double-pay bribes.
    if _GANG == "blackarm" or (black >= 1 and phoenix < PHOENIX_JOINED):
        if black < BLACKARM_JOINED:
            _join_blackarm(bot)
            return
    elif phoenix >= 1 or _GANG in (None, "phoenix"):
        if phoenix < PHOENIX_JOINED:
            _join_phoenix(bot)
            return
    else:
        _join_phoenix(bot)
        return
    _ensure_halves(bot)
    solver.talk_like(bot, "curator", ["retrieved", "shield", "reward", "certificate"])
    if bot.inventory().get(solver.obj_id("arravcertificate"), 0) < 1:
        raise solver.SolverError("curator did not issue the certificate")
    solver.talk_like(bot, "king_roald", ["majesty", "reward", "claim", "certificate"])
    solver.wait_complete(bot)
