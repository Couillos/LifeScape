"""Requirement check and completion: Pirate's Treasure.

Smuggle Karamjan rum to Port Sarim in a banana crate, trade it to
Redbeard Frank for the chest key, open One-Eyed Hector's chest upstairs
in the Blue Moon Inn, read the pirate message, and dig in Falador park.

Needs from the bank (besides the rum + 10 bananas the requirement check
already proves): ~600 coins buffer for fares/shopping, a white apron and
a spade; both are bought from shops when the bank lacks them.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.waits import wait_ticks
from scripts.quests import common as Q
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as C

QUEST = quest_requirements.by_key("hunt")

COINS_TARGET = 600
DIG_TILES = ((2999, 3383), (2998, 3383), (3000, 3383), (2999, 3382), (2999, 3384))


def _varp(bot, name):
    return bot.world.varps.get(knowledge.varp_id_by_name(name), 0)


def _count(bot, obj_id):
    return bot.inventory().get(obj_id, 0)


def _bank_topup(bot):
    """Withdraw the coins buffer plus apron/spade when the bank has them."""
    try:
        bank = Q._open_nearest_bank(bot)
    except Exception as error:
        print("bank top-up skipped: %s" % (error,), flush=True)
        return
    try:
        coins = solver.obj_id("coins")
        shortfall = COINS_TARGET - _count(bot, coins)
        if shortfall > 0 and bank.get(coins, 0) > 0:
            bot.bank_withdraw(coins, min(shortfall, bank.get(coins, 0)))
        for name in ("white_apron", "spade"):
            wanted = solver.obj_id(name)
            if _count(bot, wanted) < 1 and bank.get(wanted, 0) > 0:
                bot.bank_withdraw(wanted, 1)
    finally:
        solver.close_bank(bot)


def _buy(bot, obj_name):
    """Buy one of an item from the nearest shop that stocks it."""
    wanted = solver.obj_id(obj_name)
    if _count(bot, wanted) > 0:
        return True
    main_shop = knowledge.comps.com_id("shop_template:inv")
    cands = []
    for shop in knowledge.all_shops():
        if any(stock.obj == obj_name and stock.count > 0 for stock in shop.stocks):
            for keeper in shop.keepers:
                for spawn in knowledge.all_npc_spawns(keeper.npc_id):
                    if spawn.level == bot.world.level and not spawn.members:
                        cands.append((
                            abs(spawn.x - bot.world.x) + abs(spawn.z - bot.world.z),
                            keeper.npc, spawn,
                        ))
    cands.sort(key=lambda item: item[0])
    for _dist, name, spawn in cands:
        if not bot.trade_to(name, (spawn.x, spawn.z, spawn.level)):
            continue
        try:
            bot.pump(10)
            before = _count(bot, wanted)
            slots = bot.world.inv.get(main_shop, {})
            slot = next((index for index, (held, _n) in slots.items() if held == wanted), None)
            if slot is None:
                continue
            bot.inv_button(2, wanted, slot, main_shop)
            target = bot.world.ticks + 60
            while bot.world.ticks < target and _count(bot, wanted) <= before:
                bot.pump(2)
            if _count(bot, wanted) > before:
                return True
        finally:
            bot.close_modal()
            bot.pump(5)
    return _count(bot, wanted) > 0


def _wear(bot, obj_name):
    wanted = solver.obj_id(obj_name)
    worn = bot.world.inv.get(Q.WORN_COM, {})
    if any(held == wanted for held, _count in worn.values()):
        return True
    com, slot = C.inv_slot(bot, wanted)
    bot.op_held(2, wanted, slot, com)
    target = bot.world.ticks + 60
    while bot.world.ticks < target:
        bot.pump(2)
        worn = bot.world.inv.get(Q.WORN_COM, {})
        if any(held == wanted for held, _count in worn.values()):
            return True
    raise solver.SolverError("could not wear %s" % obj_name)


def _sail(bot, sailor, keywords):
    old = (bot.world.x, bot.world.z)
    solver.talk_like(bot, sailor, keywords)
    target = bot.world.ticks + 200
    while bot.world.ticks < target:
        bot.pump(5)
        if (bot.world.x, bot.world.z) != old:
            wait_ticks(bot, 10)
            return
    raise solver.SolverError("ship did not sail with %s" % sailor)


def _pack_crate(bot):
    """Stash the rum, then fill the plantation crate with 10 bananas."""
    rum = solver.obj_id("karamja_rum")
    if _varp(bot, "crate_rum") == 0:
        if _count(bot, rum) < 1:
            raise solver.SolverError("no Karamjan rum to smuggle")
        solver.use_item_on_loc(bot, "karamja_rum", ("bananacrate",))
        if _varp(bot, "crate_rum") != 1:
            raise solver.SolverError("rum did not go into the crate")
    banana = solver.obj_id("banana")
    target = bot.world.ticks + 1200
    while _varp(bot, "crate_bananas") < 10:
        if bot.world.ticks > target:
            raise solver.SolverError("crate never filled with bananas")
        if _count(bot, banana) < 1:
            before = _count(bot, banana)
            inner = bot.world.ticks + 300
            while _count(bot, banana) == before and bot.world.ticks < inner:
                solver.operate_loc(bot, ("bananatree",), 1, timeout=60)
            if _count(bot, banana) == before:
                try:
                    bot.approach(bot.world.x + 5, bot.world.z, radius=2, timeout=30)
                except Exception:
                    pass
                continue
        have = _varp(bot, "crate_bananas")
        solver.use_item_on_loc(bot, "banana", ("bananacrate",))
        if _varp(bot, "crate_bananas") <= have and _count(bot, banana) > 0:
            raise solver.SolverError("banana did not go into the crate")


def _dig(bot):
    spade = solver.obj_id("spade")
    if _count(bot, spade) < 1:
        raise solver.SolverError("no spade for the dig")
    com, slots = bot.backpack()
    slot = next((index for index, (held, _n) in slots.items() if held == spade), None)
    if slot is None:
        raise solver.SolverError("spade is not in the backpack")
    for x, z in DIG_TILES:
        try:
            bot.approach(x, z, radius=1, timeout=120, open_doors=True)
        except Exception as error:
            print("dig tile %d,%d unreachable: %s" % (x, z, error), flush=True)
            continue
        bot.op_held(1, spade, slot, com)
        wait_ticks(bot, 10)
        if C.has_message(bot, "dig a hole"):
            return
    raise solver.SolverError("could not dig at the Falador park spot")


def solve(bot):
    rum = solver.obj_id("karamja_rum")
    key = solver.obj_id("chest_key")
    message = solver.obj_id("piratemessage")

    _bank_topup(bot)
    if _count(bot, solver.obj_id("coins")) < 90:
        raise solver.SolverError("need at least 90 coins for the boat fares")
    if not _buy(bot, "white_apron"):
        raise solver.SolverError("need a white apron for Wydin's store (bank/Thessalia)")
    if not _buy(bot, "spade"):
        raise solver.SolverError("need a spade for the dig (bank/general store)")

    if _varp(bot, "hunt") < 2 and _count(bot, key) == 0 and _count(bot, message) == 0:
        solver.talk_like(bot, "redbeard_frank", ["treasure"])
        _sail(bot, "seaman_lorris", ["yes"])
        solver.go_to_npc(bot, "zambo")
        if not _buy(bot, "karamja_rum"):
            raise solver.SolverError("could not buy Karamjan rum from Zambo")
        solver.talk_like(bot, "luthas", ["employment"])
        _pack_crate(bot)
        solver.talk_like(bot, "luthas", ["thank", "another"])
        if _varp(bot, "crate_rum") != 2:
            raise solver.SolverError("Luthas did not ship the crate")
        _sail(bot, "customs_officer", ["journey", "search away", "ok"])
        if not (_varp(bot, "hunt_store_employed") & 2):
            solver.talk_like(bot, "wydin", ["job"])
        _wear(bot, "white_apron")
        _x, _z, level, loc = C.find_loc(bot, "grocerycrate")
        bot.approach(_x, _z, level=level, radius=2, timeout=300, open_doors=True)
        bot.op_loc(1, _x, _z, loc)
        wait_ticks(bot, 10)
        if _count(bot, rum) < 1:
            raise solver.SolverError("no rum in Wydin's back room crate")

    if _varp(bot, "hunt") == 1 and _count(bot, rum) > 0 and _count(bot, key) == 0:
        solver.talk_like(bot, "redbeard_frank", ["ok", "thanks", "here"])
    if _varp(bot, "hunt") >= 2 and _count(bot, key) == 0 and _count(bot, message) == 0:
        # Key lost after Frank handed it over: he makes another, then the
        # trade chat is the only option that closes the dialogue.
        solver.talk_like(bot, "redbeard_frank", ["trade"])
        if _count(bot, key) == 0:
            raise solver.SolverError("Redbeard Frank did not replace the key")

    bot.approach(3227, 3393, radius=3, timeout=600, open_doors=True)
    solver.operate_loc(bot, ("woodenstairs",), 1)
    if bot.world.level != 1:
        raise solver.SolverError("did not climb to the Blue Moon Inn upstairs")
    solver.operate_loc(bot, ("piratechest",), 1)
    solver.use_item_on_loc(bot, "chest_key", ("piratechest",))
    if _count(bot, message) < 1:
        raise solver.SolverError("chest did not yield the pirate message")
    com, slots = bot.backpack()
    slot = next((index for index, (held, _n) in slots.items() if held == message), None)
    if slot is None:
        raise solver.SolverError("pirate message is not in the backpack")
    bot.op_held(1, message, slot, com)
    wait_ticks(bot, 10)
    bot.close_modal()
    bot.pump(5)
    solver.operate_loc(bot, ("woodenstairstop",), 1)

    bot.approach(2999, 3383, radius=4, timeout=600, open_doors=True)
    if bot.npcs_named(("falador_gardener",)):
        solver.attack(bot, ("falador_gardener",), timeout=300)
    _dig(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
