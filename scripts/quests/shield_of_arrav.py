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

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.client import Unreachable
from lifescape.waits import wait_ticks
from scripts.quests import common as Q
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as C

QUEST = quest_requirements.by_key("blackarmgang")

PHOENIX = knowledge.varp_id_by_name("phoenixgang")
BLACKARM = knowledge.varp_id_by_name("blackarmgang")
PHOENIX_JOINED = 9
PHOENIX_DONE = 10
BLACKARM_JOINED = 3
BLACKARM_DONE = 4


def _varp(bot, name):
    return bot.world.varps.get(knowledge.varp_id_by_name(name), 0)


def _ensure_coins(bot, amount=20):
    """Baraek wants 20 coins; top up from the bank when it has them."""
    coins = solver.obj_id("coins")
    if bot.inventory().get(coins, 0) >= amount:
        return
    try:
        bank = Q._open_nearest_bank(bot)
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
    if _varp(bot, "phoenixgang") != 2:
        try:
            com, slot = C.inv_slot(bot, book)
        except KeyError:
            raise solver.SolverError("the Shield of Arrav book is not in the backpack")
        bot.op_held(1, book, slot, com)
        wait_ticks(bot, 10)
        bot.close_modal()
        bot.pump(5)
        if _varp(bot, "phoenixgang") != 2:
            raise solver.SolverError("reading the book did not advance the quest")


def _join_phoenix(bot):
    """Run the soloable Phoenix join: Reldo, book, Baraek, Straven, Jonny."""
    if _varp(bot, "phoenixgang") < 1:
        solver.talk_like(bot, "reldo", ["quest", "search"])
        if _varp(bot, "phoenixgang") < 1:
            raise solver.SolverError("Reldo did not start the quest")
    if _varp(bot, "phoenixgang") == 1:
        _read_book()
    if _varp(bot, "phoenixgang") == 2:
        solver.talk_like(bot, "reldo", ["phoenix", "read", "shield"])
        if _varp(bot, "phoenixgang") != 3:
            raise solver.SolverError("Reldo did not name Baraek")
    if _varp(bot, "phoenixgang") == 3:
        _ensure_coins(bot)
        solver.talk_like(bot, "baraek", ["phoenix", "have 20", "okay"])
        if _varp(bot, "phoenixgang") != 4:
            raise solver.SolverError("Baraek did not reveal the hideout")
    if _varp(bot, "phoenixgang") == 4:
        solver.talk_like(bot, "straven", ["know who you are", "offer", "services"])
        if _varp(bot, "phoenixgang") != 8:
            raise solver.SolverError("Straven did not set the Jonny task")
    report = solver.obj_id("intelligence_report")
    if _varp(bot, "phoenixgang") == 8 and bot.inventory().get(report, 0) < 1:
        if not solver.attack(bot, ("jonny_the_beard",), timeout=600):
            raise solver.SolverError("could not kill Jonny the Beard")
        solver.take_near(bot, "intelligence_report", timeout=120)
        if bot.inventory().get(report, 0) < 1:
            raise solver.SolverError("Jonny did not drop the intelligence report")
    if _varp(bot, "phoenixgang") == 8:
        solver.talk_like(bot, "straven", ["report", "intelligence", "join"])
        if _varp(bot, "phoenixgang") != PHOENIX_JOINED:
            raise solver.SolverError("Straven did not let us join")
    if _varp(bot, "phoenixgang") != PHOENIX_JOINED:
        raise solver.SolverError("Phoenix join did not finish")


def _loot_inside(bot, door, closed, opened, closed_op, opened_op):
    """Search our own gang's loot store, entering first unless inside."""
    try:
        C.find_loc(bot, closed, opened)
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
    if _varp(bot, "phoenixgang") == PHOENIX_DONE or _varp(bot, "blackarmgang") == BLACKARM_DONE:
        return
    if _varp(bot, "phoenixgang") < PHOENIX_JOINED and _varp(bot, "blackarmgang") < BLACKARM_JOINED:
        _join_phoenix(bot)
    _ensure_halves(bot)
    solver.talk_like(bot, "curator", ["retrieved", "shield", "reward", "certificate"])
    if bot.inventory().get(solver.obj_id("arravcertificate"), 0) < 1:
        raise solver.SolverError("curator did not issue the certificate")
    solver.talk_like(bot, "king_roald", ["majesty", "reward", "claim", "certificate"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
