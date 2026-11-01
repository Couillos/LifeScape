"""Fishing gear helpers (split from run.py)."""

from scripts.fishing.common import *
from scripts.fishing.pack import *

def best_tool(bot):
    held = set(held for (held, _count) in pack_counts(bot).values())
    best = None
    for tier in knowledge.all_fish_tiers():
        try:
            tool_id = obj_id(bot, tier.tool)
        except KeyError:
            continue
        if tool_id in held and (best is None or tier.level > best.level):
            best = tier
    return best.tool if best is not None else None



def known_tool_ids(bot):
    ids = set()
    for tier in knowledge.all_fish_tiers():
        try:
            ids.add(obj_id(bot, tier.tool))
        except KeyError:
            continue
    return ids
