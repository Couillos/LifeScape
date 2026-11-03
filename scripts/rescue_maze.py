"""Rescue a bot stuck in the Mysterious Old Man maze random event.

The solver walks the maze to the centre. If it cannot, the server still
closes the event once the reward timer runs out (and returns the player
to where they were), so this waits that out instead of leaving the bot
inside. Success is only claimed when no random event is pending at all:
leaving the maze for another event hands that one to its own rescue.

Usage: scripts/orchestrator.py run <account> rescue_maze
"""

from scripts import randomevents
from scripts.randomevents import in_maze, solve_maze

# The event closes itself at 0% reward (100 steps x 5 ticks), plus slack for
# a slow connection. Bounded in ticks like every other game wait.
WAIT_TICKS = 4000


def _verify(bot):
    """0 when the bot is truly free, else report the pending event (1)."""
    event = randomevents.event_at(bot)
    if event is None:
        print("maze rescue done, free at %s" % (bot.position,), flush=True)
        return 0
    randomevents.report(bot, event, False)
    print("maze rescue handed off to %s event at %s" % (event, bot.position), flush=True)
    return 1


def run(bot, *args):
    bot.pump(10)
    bot.intend("rescue: maze")
    print("start %s" % (bot.position,), flush=True)
    if not in_maze(bot):
        print("not in maze, nothing to do", flush=True)
        return _verify(bot)
    if solve_maze(bot):
        return _verify(bot)
    print("maze unsolved, waiting for the event to time out", flush=True)
    deadline = bot.world.ticks + WAIT_TICKS
    while in_maze(bot) and bot.world.ticks < deadline:
        bot.pump(5)
    if in_maze(bot):
        print("maze still active after %d ticks" % WAIT_TICKS, flush=True)
        return 1
    print("maze timed out, back at %s" % (bot.position,), flush=True)
    return _verify(bot)
