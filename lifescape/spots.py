"""Safe-spot picking from the server's NPC aggression rules.

Mirrors Npc.huntPlayers (engine/.../entity/Npc.ts): an NPC hunts players
within chebyshev distance <= huntrange (same level), unless exempt by
level (player combat > 2 * vislevel outside the wilderness). AFK, busy
and in-combat checks only ever spare the player, so static safety
ignores them. NPCs wander around their spawn (NpcType default 5),
hence the wander margin on top of huntrange. checkVis (line of sight /
walk) is also ignored: without it a spot is safe either way.

No gamedata import here on purpose (lifescape is stdlib-only): callers
pass plain tuples built from gamedata spawns + npc types.
"""

WANDER_MARGIN = 5
EXEMPT_MULTIPLE = 2

ATTACK = 0
DEFENCE = 1
STRENGTH = 2
HITPOINTS = 3
RANGED = 4
PRAYER = 5
MAGIC = 6


def combat_level(levels):
    """Server getCombatLevel formula on base levels (current approximates)."""
    try:
        attack = levels[ATTACK]
        defence = levels[DEFENCE]
        strength = levels[STRENGTH]
        hitpoints = levels[HITPOINTS]
        ranged = levels[RANGED]
        prayer = levels[PRAYER]
        magic = levels[MAGIC]
    except KeyError:
        return None
    base = 0.25 * (defence + hitpoints + prayer // 2)
    melee = 0.325 * (attack + strength)
    ranged_part = 0.325 * (ranged // 2 + ranged)
    magic_part = 0.325 * (magic // 2 + magic)
    return int(base + max(melee, ranged_part, magic_part))


def exempt_by_level(player_combat, vislevel):
    if player_combat is None or vislevel is None or vislevel < 0:
        return False
    return player_combat > EXEMPT_MULTIPLE * vislevel


def danger_radius(huntrange, hunts_players, margin=WANDER_MARGIN):
    if not hunts_players or huntrange <= 0:
        return 0
    return huntrange + margin


def chebyshev(x1, z1, x2, z2):
    return max(abs(x1 - x2), abs(z1 - z2))


def threats_at(spot_x, spot_z, level, npcs, player_combat):
    """NPC threat entries within their danger radius of a tile.

    npcs: iterable of (x, z, level, huntrange, vislevel, hunts_players).
    Returns [(distance, npc), ...] sorted nearest first.
    """
    found = []
    for npc in npcs:
        npc_x, npc_z, npc_level, huntrange, vislevel, hunts = npc[:6]
        if npc_level != level:
            continue
        margin = npc[6] if len(npc) > 6 else WANDER_MARGIN
        radius = danger_radius(huntrange, hunts, margin)
        if radius <= 0:
            continue
        if exempt_by_level(player_combat, vislevel):
            continue
        dist = chebyshev(spot_x, spot_z, npc_x, npc_z)
        if dist <= radius:
            found.append((dist, npc))
    return sorted(found, key=lambda threat: threat[0])


def spot_safe(spot_x, spot_z, level, npcs, player_combat):
    return not threats_at(spot_x, spot_z, level, npcs, player_combat)


def nearest_safe_spot(spots, npcs, player_combat, near_x, near_z, level):
    """Closest spot to (near_x, near_z) with no threat in range."""
    best = None
    for spot in spots:
        spot_x, spot_z, spot_level = spot[0], spot[1], spot[2]
        if spot_level != level:
            continue
        if not spot_safe(spot_x, spot_z, level, npcs, player_combat):
            continue
        dist = abs(spot_x - near_x) + abs(spot_z - near_z)
        if best is None or dist < best[0]:
            best = (dist, spot)
    return best[1] if best is not None else None
