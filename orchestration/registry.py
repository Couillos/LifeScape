"""Scripts the orchestrator may launch.

The module reloads itself when this file changes, so adding a script does
not require restarting the daemon: the next start command sees it. Script
code, the SDK and gamedata are already mounted live and run in a fresh
worker process per job.
"""

import importlib
import importlib.util
import os
import sys

from .paths import ROOT

SCRIPTS = {
    "fishing": "scripts/fishing/run.py",
    "mining": "scripts/mining/run.py",
    "woodcut": "scripts/woodcut/run.py",
    "melee": "scripts/melee/run.py",
    "tutorial": "scripts/tutorial/run.py",
    "quest_black_knights_fortress": "scripts/quests/black_knights_fortress.py",
    "quest_cooks_assistant": "scripts/quests/cooks_assistant.py",
    "quest_demon_slayer": "scripts/quests/demon_slayer.py",
    "quest_dorics_quest": "scripts/quests/dorics_quest.py",
    "quest_dragon_slayer": "scripts/quests/dragon_slayer.py",
    "quest_ernest_the_chicken": "scripts/quests/ernest_the_chicken.py",
    "quest_goblin_diplomacy": "scripts/quests/goblin_diplomacy.py",
    "quest_imp_catcher": "scripts/quests/imp_catcher.py",
    "quest_knights_sword": "scripts/quests/knights_sword.py",
    "quest_pirates_treasure": "scripts/quests/pirates_treasure.py",
    "quest_prince_ali_rescue": "scripts/quests/prince_ali_rescue.py",
    "quest_restless_ghost": "scripts/quests/restless_ghost.py",
    "quest_romeo_and_juliet": "scripts/quests/romeo_and_juliet.py",
    "quest_rune_mysteries": "scripts/quests/rune_mysteries.py",
    "quest_sheep_shearer": "scripts/quests/sheep_shearer.py",
    "quest_shield_of_arrav": "scripts/quests/shield_of_arrav.py",
    "quest_vampire_slayer": "scripts/quests/vampire_slayer.py",
    "quest_witchs_potion": "scripts/quests/witchs_potion.py",
    "rescue_maze": "scripts/rescue_maze.py",    "rescue_down": "scripts/rescue_down.py",
    "rescue_fallback": "scripts/rescue_fallback.py",
    "check_transitions": "scripts/check_transitions.py",
    "check_doors": "scripts/check_doors.py",
    "probe_transitions": "scripts/probe_transitions.py",
    "goto": "scripts/goto.py",
    "bank_deposit": "scripts/bank_deposit.py",
    "drop": "scripts/drop.py",
    "probe_door": "scripts/probe_door.py",
    "probe_maze": "scripts/probe_maze.py",
    "probe_give": "scripts/probe_give.py",
    "probe_talk": "scripts/probe_talk.py",
    "probe_use": "scripts/probe_use.py",
    "probe_vars": "scripts/probe_vars.py",
    "quest_reset": "scripts/quest_reset.py",
    "reset_quests": "scripts/reset_quests.py",
    "give": "scripts/give.py",
    "tests": "scripts/run_tests.py",
}

# Unresolved random event -> rescue script. Events with no dedicated rescue
# fall back to rescue_fallback (bounded relogin + event check loop).
RESCUES = {"maze": "rescue_maze", "tower": "rescue_down"}


_mtime = None
try:
    _mtime = os.path.getmtime(__file__)
except OSError:
    pass


def reload_if_changed():
    """Pick up a registry edit without restarting the daemon."""
    global _mtime
    try:
        mtime = os.path.getmtime(__file__)
    except OSError:
        return
    if mtime != _mtime:
        _mtime = mtime
        importlib.reload(sys.modules[__name__])


def script_path(name):
    reload_if_changed()
    if name not in SCRIPTS:
        raise KeyError("unknown script %r (known: %s)" % (name, ", ".join(sorted(SCRIPTS))))
    return os.path.join(ROOT, SCRIPTS[name])


def load_run(name):
    """Import a supervised script fresh and return its run(bot, *args).

    Each run gets a new module object so one session can load and unload
    the same script without stale module state. Helper modules under
    ``scripts.``/``gamedata`` are also dropped from the import cache: the
    session worker persists across runs, so without this an edit to a
    helper (solver, randomevents, quest module) would stay invisible
    until the session is closed. ``lifescape`` (live connection state)
    and ``orchestration`` (worker code) are never dropped. Raises
    KeyError when the script is unknown or exposes no callable run.
    """
    path = script_path(name)
    for cached in [key for key in sys.modules
                   if key in ("scripts", "gamedata") or key.startswith(("scripts.", "gamedata."))]:
        sys.modules.pop(cached, None)
    module_name = "lifescape_session_script_%s" % abs(hash(path))
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise KeyError("cannot load script %r" % (name,))
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception as error:
        raise KeyError("cannot import script %r: %s" % (name, error)) from error
    run = getattr(module, "run", None)
    if not callable(run):
        raise KeyError("script %s has no run(bot)" % (name,))
    return run, path
