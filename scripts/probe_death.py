"""Measure death -> playable-again latency with the shared SDK helper.

Walk into a dark wizard, wait for hitpoints to hit 0, then time
lifescape.hazards.wait_respawn until control comes back and print where
the player lands. Diagnostic only.

Usage: scripts/orchestrator.py run <account> probe_death
"""
import time


from lifescape import hazards


def run(bot, *args):
    bot.pump(20)
    print("start", bot.position, "hp", hazards.hitpoints(bot), flush=True)
    try:
        bot.walk_to(3084, 3236, bot.world.level, timeout=3000, open_doors=True)
    except Exception as walk_error:
        print("walk into range:", walk_error, flush=True)
    died_at = None
    ticks_start = bot.world.ticks
    while bot.world.ticks - ticks_start < 600:
        bot.pump(2)
        if hazards.hitpoints(bot) <= 0:
            died_at = bot.world.ticks
            print("died at", bot.position, "hp=0", flush=True)
            break
    if died_at is None:
        print("not attacked within 600 ticks, hp", hazards.hitpoints(bot), flush=True)
    else:
        wall_start = time.time()
        playable = hazards.wait_respawn(bot)
        print("playable=%s after %d ticks / %.1fs at %s hp=%d"
              % (playable, bot.world.ticks - died_at, time.time() - wall_start, bot.position, hazards.hitpoints(bot)), flush=True)
