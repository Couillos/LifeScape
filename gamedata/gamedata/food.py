"""Food healing values, from the generated consumption dump.

Only items the server actually heals on eat are present (normal food and
the special consumption effects). Values are hitpoints restored.
"""

from gamedata import base

_foods = None


def all_foods():
    global _foods
    if _foods is None:
        _foods = base.load_table("foods")
    return _foods


def heal(name):
    return all_foods().get(name, 0)


def best_food(names):
    """Highest-healing food among the given names, or None."""
    best = None
    for name in names:
        amount = heal(name)
        if amount > 0 and (best is None or amount > best[1]):
            best = (name, amount)
    return best
