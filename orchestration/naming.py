"""Turn ids into names for the dashboard, through cached gamedata indexes.

The API returns resolved names so any client (web, CLI, future tools)
sees "bankbooth" or "main_side:inventory" instead of raw ids. Lookups are
O(1): gamedata builds each id->name table once and keeps it.
"""

import re

import gamedata as knowledge


COSMETIC = ("grass", "twig", "flower", "daisy", "weed", "fern", "petal", "leaf", "leaves", "clover", "rug", "dugup")


def humanize(name):
    """Cache names are snake_case: show the words the game shows."""
    text = str(name or "").replace("_", " ").strip()
    text = re.sub(r"^(?:\d+ ){2,3}", "", text)  # fishing spot internals: 0 48 50 saltfish
    if not text:
        return ""
    return text[0].upper() + text[1:]


def _cosmetic(loc):
    """Decorative scenery (grass, twigs, flowers, rugs): no action, no
    animation, nothing to click or collide with in a useful way."""
    if loc.get("act"):
        return False
    name = str(loc.get("name") or "").lower()
    return any(word in name for word in COSMETIC)


def _flowerbed(loc):
    """Planted or wild flowers: dotted on the detailed map, never listed."""
    if loc.get("act"):
        return False
    return "flower" in str(loc.get("name") or "").lower()


def _com_name(modal):
    if not modal or not isinstance(modal, (list, tuple)) or len(modal) < 2:
        return None
    return knowledge.comp_name_by_id(modal[1])


def _hint_name(state):
    hint = state.get("hint")
    if not hint or not isinstance(hint, (list, tuple)):
        return None
    if hint[0] == "npc":
        for npc in state.get("npcs") or []:
            if npc.get("nid") == hint[1]:
                return npc.get("name") or "npc %s" % hint[1]
        name = knowledge.npc_name_by_id(state.get("hint_type"))
        return name or "npc %s" % hint[1]
    if hint[0] == "coord" and len(hint) >= 3:
        return "tile %d,%d" % (hint[1], hint[2])
    return None


def resolve_state(state):
    """Fill every name the snapshot left as an id. Mutates and returns it."""
    if not state:
        return state
    state["account_name"] = humanize(state.get("account"))
    state["modal_name"] = humanize(_com_name(state.get("modal"))) or None
    state["hint_name"] = humanize(_hint_name(state)) or None
    for npc in state.get("npcs") or []:
        if not npc.get("name"):
            npc["name"] = knowledge.npc_name_by_id(npc.get("type")) or ""
        npc["name"] = humanize(npc.get("name"))
    for loc in state.get("locs") or []:
        if not loc.get("name"):
            loc["name"] = knowledge.loc_name_by_id(loc.get("id")) or ""
        loc["name"] = humanize(loc.get("name"))
    for obj in state.get("objs") or []:
        if not obj.get("name"):
            obj["name"] = knowledge.obj_name_by_id(obj.get("id")) or ""
        obj["name"] = humanize(obj.get("name"))
    kept_locs = []
    flora = []
    for loc in state.get("locs") or []:
        if _flowerbed(loc):
            flora.append({"x": loc.get("x"), "z": loc.get("z"), "id": loc.get("id"), "name": loc.get("name")})
        elif not _cosmetic(loc):
            kept_locs.append(loc)
    state["locs"] = kept_locs
    if flora:
        state["flora"] = flora
    return state
