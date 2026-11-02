"""Requirement check and completion: Demon Slayer."""

import sys


import gamedata as knowledge
from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as tutorial_common

QUEST = quest_requirements.by_key("demon")

DEMONSTART = knowledge.varp_id_by_name("demonstart")
COMPLETE = 30  # ^demon_complete in content/scripts/general/configs/quest.constant
INCANTATION = "Carlem Aber Camerinthum Purchai Gabindo"


def _wield(bot, item):
    """Wear an item through its Wield op; a missing item is a SolverError."""
    worn_com = knowledge.com_id("wornitems:wear")
    obj = solver.obj_id(item)
    worn = bot.world.inv.get(worn_com, {})
    if any(held == obj for held, _count in worn.values()):
        return
    try:
        com, slot = tutorial_common.inv_slot(bot, obj)
    except KeyError:
        raise solver.SolverError("no %s to wield at %s" % (item, bot.position))
    bot.op_held(2, obj, slot, com)
    target = bot.world.ticks + 30
    while bot.world.ticks < target:
        bot.pump(2)
        worn = bot.world.inv.get(worn_com, {})
        if any(held == obj for held, _count in worn.values()):
            return
    raise solver.SolverError("could not wield %s" % item)


def solve(bot):
    if bot.world.varps.get(DEMONSTART) == COMPLETE:
        return
    # Gypsy Aris starts the quest for 1 coin and names Sir Prysin.
    solver.talk_like(bot, "aris", ["here you go", "delrith", "silverlight", "incantation", "best"])
    # Sir Prysin sends us hunting for his three keys.
    solver.talk_like(bot, "sir_prysin", ["aris", "silverlight", "crystal", "keys", "hunting", "key", "rovin", "wizard"])
    # Captain Rovin hands over his key for the demon story.
    solver.talk_like(bot, "captain_rovin", ["important", "demon", "invade", "silverlight", "key"])
    # Wizard Traiborn wants 25 bones for his key: agree first, then hand them over.
    solver.talk_like(bot, "traiborn", ["knocking", "bones", "key"])
    solver.talk_like(bot, "traiborn", ["bones", "give"])
    # Prysin's own key went down the kitchen drain: wash it into the sewer.
    key3 = solver.obj_id("silverlight_key_3")
    if bot.inventory().get(key3, 0) < 1:
        solver.use_item_on_loc(bot, "bucket_water", ("questdrain",))
        # The key washes into the Varrock sewer: use that manhole, not the
        # nearest one wherever we stand (tower/draynor covers go elsewhere).
        bot.approach(3237, 3458, radius=4, timeout=600, open_doors=True)
        solver.operate_loc(bot, ("manholeclosed", "manholeopen"), 1, timeout=120)
        if bot.world.z < 9000:
            # Still on the surface: the cover was closed, so climb down now.
            solver.operate_loc(bot, ("manholeclosed", "manholeopen"), 1, timeout=120)
        solver.take_near(bot, "silverlight_key_3", timeout=300)
        solver.operate_loc(bot, ("ladder_from_cellar",), 1, timeout=120)
    # Trade all three keys to Sir Prysin for Silverlight.
    solver.talk_like(bot, "sir_prysin", ["aris", "silverlight", "crystal", "keys", "hunting", "key", "rovin", "wizard"])
    # Wielding Silverlight is what lets the attack land and weakens Delrith.
    _wield(bot, "silverlight")
    solver.attack(bot, ("delrith",), timeout=180)
    target = bot.world.ticks + 100
    opened = False
    while bot.world.ticks < target:
        bot.pump(2)
        if bot.chat_options() or bot.world.modal is not None:
            opened = True
            break
    if not opened:
        raise solver.SolverError("delrith never weakened at %s" % (bot.position,))
    solver.finish_dialog_like(bot, [INCANTATION])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
