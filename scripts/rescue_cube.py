"""Solve the Strange box random event: a three-model shape/colour question.

Each cube opens an interface with three models (centre, side, top) and three
answer buttons in the same order. The question either names a shape and asks
its colour, or names a colour and asks its shape, so the answer is read from
the models themselves. A wrong answer replicates the cube, so answer from the
models, never by guessing.

Usage: scripts/orchestrator.py run <account> rescue_cube
"""

from lifescape.client import Unreachable

import gamedata as knowledge

CUBE = "macro_cube"
INTERFACE = "macro_cube"

COLOURS = {"red": "Red", "blue": "Blue", "yellow": "Yellow"}
SHAPES = {"triangle": "Triangle", "square": "Square", "circle": "Circle",
          "star": "Star", "halfmoon": "Halfmoon"}

MODELS = ("center_shape_model", "side_shape_model", "top_shape_model")

# One cube per pass; the server replicates them over time, so keep the loop
# tight and bound it in ticks.
CUBE_TICKS = 200
TOTAL_TICKS = 6000


def cube_id():
    found = knowledge.objs.by_name(CUBE)
    if not found:
        raise LookupError("macro cube missing from gamedata")
    return found[0].obj_id


def split_name(name):
    """'macro_cube_redhalfmoon' -> ('red', 'halfmoon'), never guessed."""
    if not name or not name.startswith("macro_cube_"):
        return None, None
    tail = name[len("macro_cube_"):]
    for colour in COLOURS:
        if tail.startswith(colour):
            return colour, tail[len(colour):]
    return None, None


def cube_count(bot):
    return bot.inventory().get(cube_id(), 0)


def cube_slot(bot):
    """(com, slot) of a cube stack, in whichever transmitted inventory holds it."""
    obj = cube_id()
    for com, slots in bot.world.inv.items():
        for slot, (held, count) in sorted(slots.items()):
            if held == obj and count:
                return com, slot
    return None


def model_traits(bot):
    """(colour, shape) of the centre, side and top models, in that order."""
    traits = []
    for suffix in MODELS:
        obj_id = bot.world.modal_models.get(bot.com_id(INTERFACE + ":" + suffix))
        traits.append(split_name(knowledge.objs.name_by_id(obj_id)))
    return traits


def read_question(bot):
    """('colour', shape) or ('shape', colour), from the interface text."""
    text = (bot.world.texts.get(bot.com_id(INTERFACE + ":question")) or "").lower()
    if text.startswith("what colour"):
        for shape, label in SHAPES.items():
            if label.lower() in text:
                return "colour", shape
    elif text.startswith("which shape"):
        for colour, label in COLOURS.items():
            if label.lower() in text:
                return "shape", colour
    return None, None


def answer_index(traits, kind, value):
    """Index of the model carrying the asked trait."""
    for index, (colour, shape) in enumerate(traits):
        if kind == "colour" and shape == value:
            return index
        if kind == "shape" and colour == value:
            return index
    return None


def solve_one(bot):
    found = cube_slot(bot)
    if found is None:
        return False
    com, slot = found
    print("cube: slot %d com %d count %d" % (slot, com, cube_count(bot)), flush=True)
    bot.op_held(1, cube_id(), slot, com)
    question_com = bot.com_id(INTERFACE + ":question")
    deadline = bot.world.ticks + CUBE_TICKS
    while bot.world.ticks < deadline and not bot.world.texts.get(question_com):
        bot.pump(2)
    if not bot.world.texts.get(question_com):
        print("cube: the interface did not open", flush=True)
        return False
    kind, value = read_question(bot)
    traits = model_traits(bot)
    index = answer_index(traits, kind, value)
    print("cube: %s=%s models=%s answer=%s" % (kind, value, traits, index), flush=True)
    if index is None:
        bot.close_modal()
        return False
    bot.intend("cube: %s %s" % (kind, value))
    bot.if_button(bot.com_id(INTERFACE + ":answer_button%d" % (index + 1)))
    bot.pump(3)
    return True


def run(bot, *args):
    bot.ensure_nav()
    deadline = bot.world.ticks + TOTAL_TICKS
    solved = 0
    while cube_count(bot) and bot.world.ticks < deadline:
        if bot.world.modal is not None:
            bot.close_modal()
            bot.pump(2)
        try:
            if not solve_one(bot):
                break
        except (Unreachable, TimeoutError) as error:
            print("cube: %s: %s" % (type(error).__name__, error), flush=True)
            break
        solved += 1
    left = cube_count(bot)
    print("cube: solved %d, %d left" % (solved, left), flush=True)
    return 0 if left == 0 else 1
