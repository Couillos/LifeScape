"""Requirement check and completion: Demon Slayer."""

import os
import sys


import gamedata as knowledge
from gamedata import quest_requirements
from lifescape import save as save_mod
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as tutorial_common

QUEST = quest_requirements.by_key("demon")

DEMONSTART = knowledge.varp_id_by_name("demonstart")
NOT_STARTED = 0
TALKED_ARIS = 1
KEY_HUNT = 2
FIND_BONES = 3
GOT_TRAIBORN_KEY = 28
SILVERLIGHT = 29
COMPLETE = 30  # ^demon_complete in content/scripts/general/configs/quest.constant
INCANTATION = "Carlem Aber Camerinthum Purchai Gabindo"

# Save-backed stage: fresh logins are blind to live varps (demonstart is
# not transmitted), so resume gates consult the persisted stage too.
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return {}
    try:
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _stage(bot):
    return max(bot.world.varps.get(DEMONSTART, 0), _SAVED_VARPS.get(DEMONSTART, 0))


def _await_stage(bot, want, timeout=60):
    """Poll a stage gate: the live varp update can land a tick after the
    dialogue that set it closes, and a single read falsely fails."""
    target = bot.world.ticks + timeout
    while bot.world.ticks < target and _stage(bot) < want:
        bot.pump(2)
    return _stage(bot)


def _talk_until(bot, name, keywords, want, tries=3):
    """Talk until the demonstart varp reaches want; a wrong option otherwise cascades."""
    for _ in range(tries):
        if _await_stage(bot, want) >= want:
            return
        solver.talk_like(bot, name, keywords)
        if _await_stage(bot, want) >= want:
            return
    if _await_stage(bot, want) < want:
        raise solver.SolverError("talking to %s never reached stage %d (at %d)" % (name, want, _stage(bot)))


def _count(bot, name):
    return bot.inventory().get(solver.obj_id(name), 0)


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
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    if _stage(bot) == COMPLETE:
        return
    # Aris takes a coin; Traiborn eats all 25 bones at stage 28 and the
    # drain drinks the bucket: each consumable is only required while its
    # step is still ahead, so a resume after handover never asks for it.
    if _stage(bot) < TALKED_ARIS and _count(bot, "coins") < 1:
        raise solver.SolverError("no coin for Aris at %s" % (bot.position,))
    if _count(bot, "silverlight_key_3") < 1 and _count(bot, "bucket_water") < 1:
        raise solver.SolverError("need a bucket of water for the drain at %s" % (bot.position,))
    # Wizard Traiborn wants 25 bones for his key: agree first, then hand
    # them over. Without the key past stage 28 the key was lost (or its
    # handover dialogue was cut): Traiborn resets the hunt and the 25
    # bones are due again.
    if _count(bot, "silverlight_key_1") < 1 and _stage(bot) < SILVERLIGHT:
        if _stage(bot) >= GOT_TRAIBORN_KEY or _stage(bot) < FIND_BONES:
            want_bones = 25
        else:
            want_bones = max(1, GOT_TRAIBORN_KEY - _stage(bot))
        if _count(bot, "bones") < want_bones:
            raise solver.SolverError("need %d bones for Traiborn at %s" % (want_bones, bot.position))
        solver.talk_like(bot, "traiborn", ["knocking", "bones", "key"])
        solver.talk_like(bot, "traiborn", ["bones", "give"])
    # Gypsy Aris starts the quest for 1 coin and names Sir Prysin.
    _talk_until(bot, "aris", ["here you go", "delrith", "silverlight", "incantation", "best"], TALKED_ARIS)
    # Sir Prysin sends us hunting for his three keys.
    _talk_until(bot, "sir_prysin", ["aris", "silverlight", "crystal", "keys", "hunting", "key", "rovin", "wizard"], KEY_HUNT)
    # Captain Rovin hands over his key for the demon story (no varp: skip when held).
    if _count(bot, "silverlight_key_2") < 1:
        solver.talk_like(bot, "captain_rovin", ["important", "demon", "invade", "silverlight", "key"])
    # Prysin's own key went down the kitchen drain: wash it into the sewer.
    key3 = solver.obj_id("silverlight_key_3")
    if _count(bot, "silverlight_key_3") < 1:
        bot.approach(3225, 3495, radius=6, timeout=600, open_doors=True)
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
