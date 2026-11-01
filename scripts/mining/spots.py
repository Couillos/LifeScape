"""Mining spots helpers (split from run.py)."""

from scripts.mining.common import *

def usable_tiers():
    # Rune essence is excluded: the essence mine is only reachable by
    # teleport (Aubury), never by walking from a bank.
    return [tier for tier in knowledge.all_rock_tiers() if tier.product and tier.product_members is False and tier.product != "blankrune"]



def tier_price(tier):
    found = knowledge.obj_by_name(tier.product)
    return found[0].price if found else 0



def pick_tier(tiers, mining_level):
    best = None
    for tier in tiers:
        if tier.level <= mining_level and (best is None or (tier.level, tier_price(tier)) > (best.level, tier_price(best))):
            best = tier
    return best



def nearest_spot(spots, near_x, near_z):
    best = None
    for spot in spots:
        dist = abs(spot.x - near_x) + abs(spot.z - near_z)
        if best is None or dist < best[0]:
            best = (dist, spot)
    return best[1] if best is not None else None



def pick_rock_spot(tiers, bank, mining_level, banned, player_combat, near_x=None, near_z=None, bot=None, relaxed=False):
    if relaxed:
        npcs = [threat + (LIVE_MARGIN,) for threat in static_threats()]
    elif bot is not None:
        npcs = threats(bot)
    else:
        npcs = static_threats()
    if near_x is None:
        near_x, near_z = bank.x, bank.z
    affordable = sorted((tier for tier in tiers if tier.level <= mining_level), key=lambda tier: (-tier.level, -tier_price(tier)))
    for tier in affordable:
        rocks = [name for name in tier.rocks if not name.startswith("macro_")]
        spots = [spot for spot in knowledge.loc_spawns_near(rocks, bank.x, bank.z, 0, radius=NEAR_BANK_TILES) if (spot.x, spot.z) not in banned]
        if bot is not None:
            spots = [spot for spot in spots if any(tile_walkable(bot, spot.x + dx, spot.z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz)]
        safe = [spot for spot in spots if spots_module.spot_safe(spot.x, spot.z, 0, npcs, player_combat)]
        if safe:
            best = nearest_spot(safe, near_x, near_z)
            if best is not None and abs(best.x - near_x) + abs(best.z - near_z) > NEAR_BANK_TILES:
                continue
            return tier, best, bank
    return None, None, None



def spot_is_safe(bot, check_x, check_z):
    return spots_module.spot_safe(check_x, check_z, bot.world.level, threats(bot), bot.combat_level())



def live_loc_id(bot, rock_x, rock_z):
    level = bot.world.level
    loc = bot.world.locs.get((rock_x, rock_z, level))
    if loc is None:
        loc = bot.world.static_locs.get((rock_x, rock_z, level))
    if loc is None:
        raise Unreachable("no loc at %d,%d" % (rock_x, rock_z))
    return loc[0]



def is_rock_still_there(bot, spot, tier):
    try:
        loc_id = live_loc_id(bot, spot.x, spot.z)
    except Unreachable:
        return True
    entry = knowledge.loc_by_id(loc_id)
    # All rocks share the display name "Rocks": compare debugnames so a
    # depleted rock (rocks1/rocks2) is not mistaken for a full one.
    debug = (entry.name or "").lower() if entry else ""
    return debug in set(name.lower() for name in tier.rocks if not name.startswith("macro_"))



def obj_loc_id(bot, spot, _tier=None):
    return live_loc_id(bot, spot.x, spot.z)
