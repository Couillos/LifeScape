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
from scripts.tutorial import common as tutorial_common


class SolverError(RuntimeError):
    pass


# Random-event junk the bank refuses. macro_cube is deliberately NOT here:
# dropping a cube (opheld5) runs try_add_macro_cube and ADDS another one.
# Cubes are cleared by solving the strange box (see solve_strange_box).
JUNK_ITEMS = ()


def drop_junk(bot):
    """Drop any junk in the backpack; silent when there is none."""
    for name in JUNK_ITEMS:
        found = knowledge.objs.by_name(name)
        if not found:
            continue
        obj_id = found[0].obj_id
        while bot.inventory().get(obj_id, 0) > 0:
            try:
                if not bot.drop(obj_id, timeout=15):
                    break
            except Exception:
                break
            bot.pump(3)


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
    if not tutorial_common.talk(bot, name, timeout=timeout, near=near):
        raise SolverError("cannot talk to %s at %s" % (name, bot.position))
    # finish_dialog_like always answers something (first option as fallback):
    # tutorial_common.finish_dialog with empty answers stalls forever on a choice page,
    # e.g. the post-quest chat when an earlier step already finished the job.
    finish_dialog_like(bot, answers, timeout=timeout)


def completion_open(bot):
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
        bot.pump(2)
        # Quest items land mid-dialogue: keep one slot free so a delivery
        # never overflows to the ground while we click through pages.
        drop_junk(bot)
        if completion_open(bot):
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
            if completion_open(bot):
                bot.close_modal()
                bot.pump(5)
                return True
            if no_option_since is None:
                no_option_since = bot.world.ticks
            # A choice page arrives a tick or two after the NPC line. Never
            # resume a paused choice with RESUME_PAUSEBUTTON: the server's
            # switch_component then falls through to the default branch.
            # Keep the grace short (a few ticks): every plain page pays it.
            if bot.world.ticks - no_option_since < 4:
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
                # No options and nothing new for ~9+ ticks: the server
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
    if not tutorial_common.talk(bot, name, timeout=timeout, near=near):
        raise SolverError("cannot talk to %s at %s" % (name, bot.position))
    finish_dialog_like(bot, keywords, timeout=timeout)


def wait_complete(bot, timeout=300):
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        bot.pump(5)
        if tutorial_common.has_message(bot, "Congratulations! Quest complete!"):
            return True
        if completion_open(bot):
            bot.close_modal()
            bot.pump(5)
            return True
    return False


def operate_loc(bot, names, op=1, timeout=60, stand=None):
    x, z, level, loc = tutorial_common.find_loc(bot, *names)
    if stand is not None:
        # Some locs only answer from one side (reachLoc angle): park on the
        # given tile instead of the nearest approach tile, which the server
        # can refuse with "I can't reach that!".
        bot.walk_to(stand[0], stand[1], stand[2] if len(stand) > 2 else level, timeout=timeout, open_doors=True)
    else:
        bot.approach(x, z, level=level, radius=2, timeout=timeout, open_doors=True)
    bot.op_loc(op, x, z, loc)
    wait_ticks(bot, 10)
    # An op often opens a plain mesbox (drink, open, search) that halts the
    # server script until dismissed: click through it (and any story that
    # follows) so follow-up steps and the completion scroll can land.
    # Main windows are left alone for wait_complete.
    drain_dialog(bot)


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


def drain_dialog(bot, extra_ticks=150):
    """Click through plain chat pages until the modal closes or chooses.

    Follows operating/using when the server keeps talking (mesboxes and
    multi-page stories). Stops at choice pages and completion scrolls,
    which belong to the caller. Bounded by ticks, not a fixed click
    count: tales run dozens of pages.
    """
    target = bot.world.ticks + extra_ticks
    while bot.world.ticks < target:
        if bot.world.modal is None or completion_open(bot):
            return
        if bot.world.modal[0] == "main" or bot.chat_options():
            return
        bot.chat_continue()
        bot.pump(5)


def use_item_on_npc(bot, item_name, npc_name, near=None, timeout=120):
    """Use an inventory item on an NPC (research notes on Sedridor)."""
    go_to_npc(bot, npc_name, near=near)
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        nid = bot.find_npc(npc_name)
        if nid is None:
            bot.pump(5)
            continue
        npc = bot.world.npcs[nid]
        try:
            bot.approach(npc["x"], npc["z"], radius=2, timeout=20, open_doors=True)
        except (Unreachable, TimeoutError):
            continue
        nid = bot.find_npc(npc_name) or nid
        use_com, use_slot = tutorial_common.inv_slot(bot, obj_id(item_name))
        bot.op_npc_u(nid, obj_id(item_name), use_slot, use_com)
        wait_ticks(bot, 10)
        # Same halt risk as operate_loc: a result mesbox or a multi-page
        # story blocks the server script until dismissed.
        drain_dialog(bot)
        return True
    raise SolverError("cannot use %s on %s at %s" % (item_name, npc_name, bot.position))


CUBE_SHAPES = ("Triangle", "Square", "Circle", "Star", "Halfmoon")
CUBE_COLOURS = ("Red", "Blue", "Yellow")


def _cube_shape_colour(obj_id):
    """(shape, colour) for a shape-model obj id, or None."""
    if 3063 <= obj_id <= 3091 and (obj_id - 3063) % 2 == 0:
        slot = (obj_id - 3063) // 2
        return CUBE_SHAPES[slot // 3], CUBE_COLOURS[slot % 3]
    return None


def _cube_answer(question, shapes_colours):
    """Answer-button index (0-2) for the quiz question, or None."""
    asked = question.strip().rstrip("?")
    if asked.lower().startswith("what colour is the "):
        want = asked[len("what colour is the "):].strip().lower()
        for index, (shape, _colour) in enumerate(shapes_colours):
            if shape.lower() == want:
                return index
    elif asked.lower().startswith("which shape is "):
        want = asked[len("which shape is "):].strip().lower()
        for index, (_shape, colour) in enumerate(shapes_colours):
            if colour.lower() == want:
                return index
    return None


def solve_strange_box(bot, max_rounds=40):
    """Answer the strange-box shape quiz until the cubes are gone.

    The box interface shows three shape models (centre, side, top) and
    asks for one's colour or one's shape. The model obj ids encode both,
    so the correct answer button (1-3, same order) is picked
    deterministically instead of guessed.
    """
    cube = obj_id("macro_cube")
    question_com = knowledge.com_id("macro_cube:question")
    button_coms = [knowledge.com_id("macro_cube:answer_button%d" % i) for i in (1, 2, 3)]
    model_coms = [knowledge.com_id(name) for name in (
        "macro_cube:center_shape_model",
        "macro_cube:side_shape_model",
        "macro_cube:top_shape_model",
    )]
    for _ in range(max_rounds):
        if bot.inventory().get(cube, 0) == 0:
            return True
        try:
            inv_com, slot = tutorial_common.inv_slot(bot, cube)
        except KeyError:
            return True
        bot.op_held(1, cube, slot, inv_com)
        target = bot.world.ticks + 60
        question = None
        models = []
        while bot.world.ticks < target:
            bot.pump(3)
            texts = dict(bot.chat_texts())
            question = texts.get(question_com)
            models = [bot.world.modal_models.get(com) for com in model_coms]
            if question and all(models):
                break
        if not question or not all(models):
            print("cube quiz waiting: question=%r models=%s" % (question, models), flush=True)
            continue
        shapes_colours = [_cube_shape_colour(held) for held in models]
        if any(pair is None for pair in shapes_colours):
            continue
        pick = _cube_answer(question, shapes_colours)
        if pick is None:
            print("cube quiz unparsed: %r" % (question,), flush=True)
            continue
        print("cube quiz: %r models=%s -> button %d" % (question[:60], models, pick + 1), flush=True)
        bot.if_button(button_coms[pick])
        bot.pump(5)
        # Dismiss the result box (success, next-cube prompt, or fail note).
        for _ in range(12):
            if bot.world.modal is None:
                break
            bot.chat_continue()
            bot.pump(3)
    return bot.inventory().get(cube, 0) == 0


def use_item_on_loc(bot, item_name, loc_names):
    x, z, level, loc = tutorial_common.find_loc(bot, *loc_names)
    tutorial_common.approach_loc(bot, x, z, loc, level)
    tutorial_common.use_held_on_loc(bot, x, z, loc, obj_id(item_name))
    wait_ticks(bot, 10)
    # Same halt risk as operate_loc: a result mesbox or a multi-page
    # story blocks the server script until dismissed.
    drain_dialog(bot)


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
