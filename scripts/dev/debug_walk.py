"""Dev tool: diagnose one walk without walking it (staff testing).

Thin wrapper over Bot.debug_walk (same output as the `debug` CLI
command, but as a supervised script run). Never used by bots.

Usage: scripts/orchestrator.py run <account> debug_walk <x> <z> [level]
"""


def run(bot, *args):
    if len(args) < 2:
        print("usage: run <account> debug_walk <x> <z> [level]", flush=True)
        return 2
    result = bot.debug_walk(*args)
    for line in result["lines"]:
        print(line, flush=True)
    return 0
