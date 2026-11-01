"""Woodcut spots helpers (split from run.py)."""

from scripts.woodcut.common import *

def pick_tier(tiers, woodcutting_level):
    best = None
    for tier in tiers:
        if tier.level <= woodcutting_level and (best is None or tier.level > best.level):
            best = tier
    return best



def nearest_spot(spots, near_x, near_z):
    best = None
    for spot in spots:
        dist = abs(spot.x - near_x) + abs(spot.z - near_z)
        if best is None or dist < best[0]:
            best = (dist, spot)
    return best[1] if best is not None else None



def tree_live(bot, spot, tier_names):
    """False only when the server recently confirmed something else here
    (usually a stump). No live update yet means assume standing."""
    live = bot.world.locs.get((spot.x, spot.z, bot.world.level))
    if live is None:
        return True
    entry = knowledge.loc_by_id(live[0])
    return entry is not None and entry.name in tier_names



def pick_tree_spot(tiers, bank, woodcutting_level, banned, player_combat, near_x=None, near_z=None, bot=None, relaxed=False):
    if relaxed:
        npcs = [threat + (LIVE_MARGIN,) for threat in static_threats()]
    elif bot is not None:
        npcs = threats(bot)
    else:
        npcs = static_threats()
    if near_x is None:
        near_x, near_z = bank.x, bank.z
    affordable = sorted((tier for tier in tiers if tier.level <= woodcutting_level), key=lambda tier: -tier.level)
    for tier in affordable:
        spots = [spot for spot in knowledge.loc_spawns_near(tier.trees, bank.x, bank.z, 0, radius=NEAR_BANK_TILES) if (spot.x, spot.z) not in banned]
        if bot is not None:
            spots = [spot for spot in spots if tree_live(bot, spot, tier.trees)]
        safe = [spot for spot in spots if spots_module.spot_safe(spot.x, spot.z, 0, npcs, player_combat)]
        if safe:
            return tier, nearest_spot(safe, near_x, near_z), bank
    return None, None, None



def live_loc_id(bot, tree_x, tree_z):
    level = bot.world.level
    loc = bot.world.locs.get((tree_x, tree_z, level))
    if loc is None:
        loc = bot.world.static_locs.get((tree_x, tree_z, level))
    if loc is None:
        raise Unreachable("no loc at %d,%d" % (tree_x, tree_z))
    return loc[0]



def is_stump(bot, tree_x, tree_z):
    """True only when the server confirms a stump here. Unknown (no live
    update yet) means keep waiting, never assume depletion."""
    live = bot.world.locs.get((tree_x, tree_z, bot.world.level))
    if live is None:
        return False
    entry = knowledge.loc_by_id(live[0])
    return entry is not None and "stump" in entry.name



def is_tree_still_there(bot, spot, tier_names):
    try:
        loc_id = live_loc_id(bot, spot.x, spot.z)
    except Unreachable:
        return True
    entry = knowledge.loc_by_id(loc_id)
    return entry is not None and entry.name in tier_names



def obj_loc_id(bot, spot, _tier=None):
    return live_loc_id(bot, spot.x, spot.z)
