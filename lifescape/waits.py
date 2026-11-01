"""Tick-based waits shared by the skill bots.

Pacing expressed in server ticks behaves the same at 600ms and 100ms
tick rates. bot.world.ticks advances on every player-info packet, which
the server sends each tick while the bot is logged in.
"""


def wait_ticks(bot, n, per_pump=2):
    target = bot.world.ticks + n
    while bot.world.ticks < target:
        bot.pump(per_pump)


def wait_hitpoints(bot, target, timeout=200):
    target_tick = bot.world.ticks + timeout
    while bot.world.ticks < target_tick:
        bot.pump(5)
        if bot.world.stats.get(3, (0, 0))[0] >= target:
            return True
    return False


def wait_backpack(bot, com_id, timeout=150):
    """Wait for the backpack com after (re)login: acting on an
    unstreamed empty pack misreads the whole inventory (e.g. a present
    tool looks lost and gets banked away)."""
    start = bot.world.ticks
    while not bot.world.inv.get(com_id) and bot.world.ticks - start < timeout:
        bot.pump(5)
    return bool(bot.world.inv.get(com_id))


def ban_area(ban_until, x, z, radius, until):
    for dx in range(-radius, radius + 1):
        for dz in range(-radius, radius + 1):
            ban_until[(x + dx, z + dz)] = until
