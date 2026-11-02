"""Aggressive-monster data for the skill bots.

The behaviour lives in ``lifescape.hazards``; this module only derives
the threat tuples from gamedata (server hunt rules: huntrange tiles on
the same level, unless the player's combat level is more than twice the
monster's vislevel).

Static spawns use the spawn position plus a wander margin (the monster
can be anywhere around it). Live monsters use their streamed position
plus a smaller reaction margin, because they are watched every few ticks.

Call ``avoid(bot)`` from every work loop: it returns True after stepping
away, so the caller drops the current spot and re-picks a safe one.
Call ``wait_respawn(bot)`` when hitpoints read 0: the server teleports
the player to Lumbridge and refills stats in a few ticks, then play
resumes at once.
"""



import gamedata as knowledge

from lifescape import hazards as sdk_hazards
from lifescape.hazards import hitpoints  # noqa: F401  (re-exported for scripts)

STATIC_THREATS = None
HUNTERS = {}
LIVE_MARGIN = sdk_hazards.LIVE_MARGIN


def hunter(npc_type):
    """Npc entry when this type hunts players, cached (by_id builds spawns)."""
    if npc_type not in HUNTERS:
        entry = knowledge.npc_by_id(npc_type)
        HUNTERS[npc_type] = entry if entry is not None and entry.hunts_players and entry.huntrange > 0 else None
    return HUNTERS[npc_type]


def static_threats():
    """(x, z, level, huntrange, vislevel, hunts) for every hunter spawn."""
    global STATIC_THREATS
    if STATIC_THREATS is None:
        hunters = {}
        for entry in knowledge.all_npcs():
            if entry is not None and entry.hunts_players and entry.huntrange > 0:
                hunters[entry.npc_id] = entry
        STATIC_THREATS = []
        for spawn in knowledge.all_npc_spawns():
            entry = hunters.get(spawn.npc_id)
            if entry is None:
                continue
            STATIC_THREATS.append((spawn.x, spawn.z, spawn.level, entry.huntrange, entry.vislevel, True))
    return STATIC_THREATS


def live_threats(bot):
    """Same tuples plus the live reaction margin, for streamed hunters."""
    found = []
    for npc in getattr(bot.world, "npcs", {}).values():
        entry = hunter(npc["type"])
        if entry is None:
            continue
        found.append((npc["x"], npc["z"], npc["level"], entry.huntrange, entry.vislevel, True, LIVE_MARGIN))
    return found


def threats(bot):
    return static_threats() + live_threats(bot)


def nearest_threat(bot):
    return sdk_hazards.nearest_threat(bot, live_threats(bot))


def avoid(bot, tiles=3):
    return sdk_hazards.avoid(bot, live_threats(bot), tiles=tiles)


def wait_respawn(bot, timeout=40):
    return sdk_hazards.wait_respawn(bot, timeout)
