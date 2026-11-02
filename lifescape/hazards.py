"""Hunter avoidance and death recovery shared by every script.

Pure behaviour: callers pass the threat tuples they built from game data
(see ``scripts/hazards.py``), this module only knows how to stay out of
their way and how to get back on task after a death.
"""

from . import spots as spots_module
from .client import Unreachable

LIVE_MARGIN = 2  # tiles of warning on top of huntrange, for live hunters


def hitpoints(bot):
    return bot.world.stats.get(3, (1, 0))[0]


def _margin(threat, default_margin):
    return threat[6] if len(threat) > 6 else default_margin


def nearest_threat(bot, threats, default_margin=LIVE_MARGIN):
    """Closest hunter whose danger radius already covers the bot."""
    player_combat = bot.combat_level()
    best = None
    for threat in threats:
        threat_x, threat_z, threat_level, huntrange, vislevel, hunts = threat[:6]
        if threat_level != bot.world.level or spots_module.exempt_by_level(player_combat, vislevel):
            continue
        gap = spots_module.chebyshev(bot.world.x, bot.world.z, threat_x, threat_z)
        if gap > huntrange + _margin(threat, default_margin):
            continue
        if best is None or gap < best[0]:
            best = (gap, threat)
    return best[1] if best is not None else None


def avoid(bot, threats, tiles=3):
    """Step out of a hunter's danger radius before it aggros.

    Returns True when the bot moved, so the caller drops its current spot.
    """
    threat = nearest_threat(bot, threats)
    if threat is None:
        return False
    threat_x, threat_z = threat[0], threat[1]
    radius = threat[3] + _margin(threat, LIVE_MARGIN)
    escape_x = 1 if bot.world.x >= threat_x else -1
    escape_z = 1 if bot.world.z >= threat_z else -1
    reach = max(tiles, radius + 1)
    candidates = []
    for step_x, step_z in (
        (escape_x, escape_z),
        (escape_x, 0),
        (0, escape_z),
        (escape_x, -escape_z),
        (-escape_x, escape_z),
        (-escape_x, 0),
        (0, -escape_z),
        (-escape_x, -escape_z),
    ):
        dest_x = bot.world.x + step_x * reach
        dest_z = bot.world.z + step_z * reach
        if (dest_x, dest_z) == (bot.world.x, bot.world.z):
            continue
        gap = spots_module.chebyshev(dest_x, dest_z, threat_x, threat_z)
        if gap <= radius:
            continue
        distance = abs(dest_x - bot.world.x) + abs(dest_z - bot.world.z)
        candidates.append((distance, dest_x, dest_z))
    candidates.sort()
    for _distance, dest_x, dest_z in candidates:
        before = (bot.world.x, bot.world.z)
        try:
            bot.walk_to(dest_x, dest_z, bot.world.level, timeout=250, open_doors=True)
        except (Unreachable, TimeoutError):
            continue
        if (bot.world.x, bot.world.z) == before:
            continue
        print("hunter at (%d,%d), stepped away to %s" % (threat_x, threat_z, bot.position), flush=True)
        bot.intend("stepping away from a hunter")
        return True
    return False


def wait_respawn(bot, timeout=40):
    """Let a death play out (teleport + stat reset) and return as soon as
    control comes back, instead of sitting idle on the old spot."""
    target_ticks = bot.world.ticks + timeout
    while hitpoints(bot) <= 0 and bot.world.ticks < target_ticks:
        bot.pump(2)
    return hitpoints(bot) > 0
