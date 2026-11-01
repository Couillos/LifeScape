"""Fishing spots helpers (split from run.py)."""

from types import SimpleNamespace

from scripts.fishing.common import *

def pick_fish_spot(tiers, fishing_level, banned, player_combat, near_x=None, near_z=None, player_level=0, pinned_bank=None, bot=None, relaxed=False):
    if relaxed:
        # A hunter is camping the water: keep spawns in the picture but with
        # the live reaction margin, so a spot 8 tiles from a spawn is usable.
        npcs = [threat + (LIVE_MARGIN,) for threat in static_threats()]
    elif bot is None:
        npcs = static_threats()
    else:
        npcs = threats(bot)
    affordable = sorted((tier for tier in tiers if tier.level <= fishing_level), key=lambda tier: -tier.level)
    for tier in affordable:
        wanted = set(knowledge.spot_npc_ids(tier.category))
        cands = []
        for npc_id in wanted:
            for spawn in knowledge.all_npc_spawns(npc_id):
                cands.append((None, spawn.x, spawn.z, spawn.level))
        if bot is not None:
            for _slot, npc in bot.world.npcs.items():
                if not npc_matches_tier(bot, npc, tier):
                    continue
                cands.append((None, npc["x"], npc["z"], player_level))
        best = None
        for _slot, spot_x, spot_z, spot_level in cands:
            if spot_level != player_level:
                continue
            if (spot_x, spot_z) in banned:
                continue
            if not spots_module.spot_safe(spot_x, spot_z, spot_level, npcs, player_combat):
                continue
            if bot is not None and not any(tile_walkable(bot, spot_x + dx, spot_z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz):
                continue
            bank = nearest_free_bank_at(spot_x, spot_z, spot_level)
            if bank is None:
                continue
            if pinned_bank is not None and abs(bank.x - pinned_bank.x) + abs(bank.z - pinned_bank.z) > 12:
                continue
            dist = abs(spot_x - bank.x) + abs(spot_z - bank.z)
            if dist > BANK_TREK_LIMIT:
                continue
            if near_x is not None and (abs(spot_x - near_x) + abs(spot_z - near_z) > TREK_LIMIT):
                continue
            if best is None or dist < best[3]:
                best = (tier, SimpleNamespace(x=spot_x, z=spot_z, level=spot_level), bank, dist)
        if best is not None:
            return best[0], best[1], best[2]
    return None, None, None



def npc_matches_tier(bot, npc, tier):
    entry = bot._loctypes["npc"][npc["type"]] if npc["type"] < len(bot._loctypes["npc"]) else None
    name = (entry["name"] if entry else "") or ""
    return name == tier.category or name.endswith("_" + tier.category)



def nearest_live(bot, tier, near_x, near_z):
    best = None
    for slot, npc in bot.world.npcs.items():
        if not npc_matches_tier(bot, npc, tier):
            continue
        if abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z) > LIVE_RADIUS:
            continue
        near_dist = abs(npc["x"] - near_x) + abs(npc["z"] - near_z)
        if best is None or near_dist < best[0]:
            best = (near_dist, slot, npc["x"], npc["z"])
    return best[1:] if best is not None else (None, None, None)


BANK_TREK_LIMIT = 150


def nearest_toll_gate(bot):
    try:
        gate = C.find_loc(bot, "border_gate_toll_left", "border_gate_toll_right")
    except Unreachable:
        return None
    return gate




def nearest_free_bank_at(near_x, near_z, level, bot=None):
    gate = nearest_toll_gate(bot) if bot is not None else None
    best = None
    fallback = None
    for bank in knowledge.all_banks():
        if bank.members or bank.level != level:
            continue
        dist = abs(bank.x - near_x) + abs(bank.z - near_z)
        if gate is not None and (bank.x >= gate[0]) != (near_x >= gate[0]):
            # Crossing the toll fence costs 10 coins each way: only a
            # last resort, never the default bank.
            if fallback is None or dist < fallback[0]:
                fallback = (dist, bank)
            continue
        if best is None or dist < best[0]:
            best = (dist, bank)
    if best is not None:
        return best[1]
    return fallback[1] if fallback is not None else None
