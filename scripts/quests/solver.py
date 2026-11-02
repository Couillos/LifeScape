"""Quest step helpers shared by the F2P solvers.

Everything moves with pathfinding; no teleport. The solver assumes the
requirement check already proved the items exist (inventory, worn or
bank). Bank items are withdrawn here, then the quest dialogue/actions
run. Item gathering is deliberately out of scope.
"""

import time

from lifescape.client import Unreachable
from lifescape.waits import wait_ticks

import gamedata as knowledge
from scripts.tutorial import common as C


class SolverError(RuntimeError):
    pass


def _known_npc(name):
    entries = knowledge.npc_by_name(name)
    if not entries:
        raise SolverError("unknown NPC %r (check the debugname in gamedata)" % (name,))
    return entries


def go_to_npc(bot, name, near=None):
    """Walk to the NPC: an explicit spot if given, else its nearest F2P spawn."""
    if near is not None:
        if abs(near[0] - bot.world.x) + abs(near[1] - bot.world.z) > 2:
            bot.intend("walking to %s near %d,%d" % (name, near[0], near[1]))
            bot.approach(near[0], near[1], radius=2, timeout=300, open_doors=True)
        bot.intend("talking to %s" % name)
        return
    entries = [entry for entry in _known_npc(name) if not entry.members]
    spawns = [(spot, entry) for entry in entries for spot in entry.spawns]
    if spawns:
        spot, _entry = min(spawns, key=lambda item: abs(item[0].x - bot.world.x) + abs(item[0].z - bot.world.z))
        if spot.level != bot.world.level or abs(spot.x - bot.world.x) + abs(spot.z - bot.world.z) > 2:
            bot.intend("walking to %s at %d,%d,%d" % (name, spot.x, spot.z, spot.level))
            bot.approach(spot.x, spot.z, level=spot.level, radius=2, timeout=300, open_doors=True)
        bot.intend("talking to %s" % name)


def talk(bot, name, answers=(), near=None, timeout=400):
    go_to_npc(bot, name, near=near)
    if bot.world.modal is not None:
        bot.close_modal()
        bot.pump(5)
    if not C.talk(bot, name, timeout=timeout, near=near):
        raise SolverError("cannot talk to %s at %s" % (name, bot.position))
    # finish_dialog_like always answers something (first option as fallback):
    # C.finish_dialog with empty answers stalls forever on a choice page,
    # e.g. the post-quest chat when an earlier step already finished the job.
    finish_dialog_like(bot, answers, timeout=timeout)


def _completion_open(bot):
    """True when the quest-complete scroll is on screen."""
    if not bot.world.modal or bot.world.modal[0] != "main":
        return False
    text = " ".join(value for _com, value in bot.chat_texts()).lower()
    return "you have completed" in text or "quest points." in text


def finish_dialog_like(bot, keywords, timeout=300):
    """Choose any option containing one of the keywords, else continue."""
    target = bot.world.ticks + timeout
    answered = set()
    no_option_since = None
    still_texts = None
    still_rounds = 0
    while bot.world.ticks < target:
        bot.pump(5)
        if _completion_open(bot):
            # The quest finished mid-talk: close its scroll so follow-up
            # talks start from a clean slate instead of piling interfaces.
            bot.close_modal()
            bot.pump(5)
            return True
        options = [
            (com, text) for com, text in bot.chat_options() if text not in answered
        ]
        if options:
            no_option_since = None
            still_rounds = 0
            choice = None
            for keyword in keywords:
                choice = next(
                    ((com, text) for com, text in options if keyword.lower() in text.lower()),
                    None,
                )
                if choice is not None:
                    break
            if choice is None:
                choice = options[0]
            bot.if_button(choice[0])
            answered.add(choice[1])
            continue
        if bot.world.modal:
            if _completion_open(bot):
                bot.close_modal()
                bot.pump(5)
                return True
            if no_option_since is None:
                no_option_since = bot.world.ticks
            # A choice page arrives a tick or two after the NPC line. Never
            # resume a paused choice with RESUME_PAUSEBUTTON: the server's
            # switch_component then falls through to the default branch.
            if bot.world.ticks - no_option_since < 12:
                continue
            if bot.world.modal[0] == "main":
                # A main window with no options (quest scroll, level-up) is
                # not advanced by the chat continue button: close it like a
                # player clicking it away, then carry on.
                bot.close_modal()
            else:
                bot.chat_continue()
            no_option_since = None
            texts_now = sorted(text for _com, text in bot.chat_texts())
            if texts_now != still_texts:
                still_texts = texts_now
                still_rounds = 0
            else:
                still_rounds += 1
            if still_rounds >= 3:
                # No options and nothing new for ~36+ ticks: the server
                # already closed this dialogue and its close packet never
                # landed (missed IF_CLOSE). Hanging here is always wrong,
                # so sync to the closed state and carry on; quest
                # verification afterwards fails loudly if this was a
                # mistake.
                bot.close_modal()
                bot.pump(5)
                return True
            continue
        return True
    raise SolverError("dialog did not close at %s" % (bot.position,))


def talk_like(bot, name, keywords, near=None, timeout=400):
    go_to_npc(bot, name, near=near)
    if bot.world.modal is not None:
        bot.close_modal()
        bot.pump(5)
    if not C.talk(bot, name, timeout=timeout, near=near):
        raise SolverError("cannot talk to %s at %s" % (name, bot.position))
    finish_dialog_like(bot, keywords, timeout=timeout)


def wait_complete(bot, timeout=300):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if C.has_message(bot, "Congratulations! Quest complete!"):
            return True
        if _completion_open(bot):
            bot.close_modal()
            bot.pump(5)
            return True
    return False


def operate_loc(bot, names, op=1, timeout=60):
    x, z, level, loc = C.find_loc(bot, *names)
    bot.approach(x, z, level=level, radius=2, timeout=timeout, open_doors=True)
    bot.op_loc(op, x, z, loc)
    wait_ticks(bot, 10)
    # An op often opens a plain mesbox (drink, open, search) that halts the
    # server script until dismissed: click through it so follow-up steps
    # (and the completion scroll) can land. Main windows are left alone
    # for wait_complete.
    for _ in range(12):
        if bot.world.modal is None or _completion_open(bot):
            break
        if bot.world.modal[0] == "main" or bot.chat_options():
            break
        bot.chat_continue()
        bot.pump(5)


def obj_id(name):
    entries = knowledge.objs.by_name(name)
    if not entries:
        raise SolverError("unknown object %r" % (name,))
    return entries[0].obj_id


def take_near(bot, name, timeout=120):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        try:
            bot.take(name=name, timeout=20)
            return True
        except (Unreachable, TimeoutError) as error:
            print("take %s failed: %s" % (name, error), flush=True)
            wait_ticks(bot, 5)
    raise SolverError("cannot pick up %s at %s" % (name, bot.position))


def use_item_on_loc(bot, item_name, loc_names):
    x, z, level, loc = C.find_loc(bot, *loc_names)
    C.approach_loc(bot, x, z, loc, level)
    C.use_held_on_loc(bot, x, z, loc, obj_id(item_name))
    wait_ticks(bot, 10)
    # Same halt risk as operate_loc: a result mesbox blocks the server
    # script until dismissed.
    for _ in range(12):
        if bot.world.modal is None or _completion_open(bot):
            break
        if bot.world.modal[0] == "main" or bot.chat_options():
            break
        bot.chat_continue()
        bot.pump(5)


def withdraw_requirements(bot, requirements):
    """Withdraw every required item still missing from the backpack."""
    for item in requirements.items:
        have = bot.inventory().get(item.obj_id, 0)
        if have >= item.count:
            continue
        bot.bank_withdraw(item.obj_id, item.count - have)
        if bot.inventory().get(item.obj_id, 0) < item.count:
            raise SolverError("could not withdraw enough %s" % item.item)


def close_bank(bot):
    bot.close_modal()
    bot.pump(5)


def find_item_slot(bot, obj_id):
    _com, slots = bot.backpack()
    for slot, (held, _count) in slots.items():
        if held == obj_id:
            return slot
    raise SolverError("item %d is not in the backpack" % obj_id)


def wait_gone(bot, names, timeout=300):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if not bot.npcs_named(names):
            return True
    return False


def attack(bot, names, timeout=300):
    """Approach and attack the nearest named NPC until it disappears."""
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        candidates = bot.npcs_named(names)
        if not candidates:
            return True
        nid, npc = min(candidates, key=lambda item: abs(item[1]["x"] - bot.world.x) + abs(item[1]["z"] - bot.world.z))
        if abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z) > 5:
            try:
                bot.approach(npc["x"], npc["z"], radius=2, timeout=30, open_doors=True)
            except (Unreachable, TimeoutError):
                bot.pump(5)
                continue
        bot.op_npc(2, nid)
        wait_ticks(bot, 5)
    return not bot.npcs_named(names)
