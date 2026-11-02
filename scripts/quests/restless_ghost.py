"""Requirement check and completion: The Restless Ghost."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from lifescape.client import Unreachable
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as C

import gamedata as knowledge


QUEST = quest_requirements.by_key("priest")


def _saved_stage():
    """Save-based quest stage (live varps are blind to pre-login stages)."""
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return 0
    try:
        from lifescape import save as save_mod
        varps = save_mod.load_account(account).get("varps", {})
        return varps.get(QUEST.completion_varps[0][0], 0) or 0
    except Exception:
        return 0


def _wear(bot, name):
    """Wear an item through its Wield op; already-worn is fine."""
    obj = solver.obj_id(name)
    worn = bot.world.inv.get(knowledge.comps.com_id("wornitems:wear"), {})
    if any(held == obj for held, _count in worn.values()):
        return
    try:
        com, slot = C.inv_slot(bot, obj)
    except KeyError:
        raise solver.SolverError("no %s to wear at %s" % (name, bot.position))
    bot.op_held(2, obj, slot, com)
    bot.pump(10)


def solve(bot):
    # Completed stages never need re-talking (fresh logins are blind to
    # live varps, so read the persisted stage instead).
    stage = _saved_stage()
    if stage < 1:
        solver.talk_like(bot, "father_aereck", ["looking for a quest", "quest"])
    if stage < 2:
        solver.talk_like(bot, "father_urhney", ["aereck", "ghost", "sent me"])
    if stage < 3:
        # The ghost only spawns when its coffin is opened, despawns quickly,
        # and only speaks through the ghostspeak amulet: wear it, spawn, and
        # talk fast (no relogin patrols: the ghost would be long gone).
        _wear(bot, "amulet_of_ghostspeak")
        talked = False
        for _ in range(6):
            try:
                solver.operate_loc(bot, ["shutghostcoffin", "restless_ghost_altar"], op=1, timeout=60)
            except (Unreachable, TimeoutError):
                pass
            nid = bot.find_npc("ghost")
            if nid is None:
                bot.pump(10)
                nid = bot.find_npc("ghost")
            if nid is None:
                continue
            npc = bot.world.npcs[nid]
            try:
                bot.approach(npc["x"], npc["z"], radius=2, timeout=20, open_doors=True)
            except (Unreachable, TimeoutError):
                continue
            bot.talk_to(bot.find_npc("ghost") or nid)
            bot.pump(5)
            if bot.world.modal is None:
                continue
            print("ghost talking attempt, npc at %s" % ((bot.world.npcs[nid]["x"], bot.world.npcs[nid]["z"]),), flush=True)
            try:
                solver.finish_dialog_like(bot, ["problem", "doomed", "skull", "yep", "yes"], timeout=400)
            except solver.SolverError:
                continue
            talked = True
            break
        if not talked:
            raise solver.SolverError("ghost never talked at %s" % (bot.position,))
    # The skull lies in the tower basement: taking it wakes its skeleton,
    # which a maxed account simply tanks. Already holding it (saved stage
    # 4): skip straight to the altar.
    if not (stage >= 4 and bot.inventory().get(solver.obj_id("ghostskull"), 0) > 0):
        bot.approach(3120, 9567, radius=6, timeout=600, open_doors=True)
        solver.take_near(bot, "ghostskull", timeout=300)
    # The coffin/altar lives in Lumbridge church, not in the basement: climb
    # back out before the use-item search (find_loc only sees nearby locs).
    # The skull only goes into the OPEN coffin: flip shut->altar first.
    bot.approach(3249, 3194, radius=6, timeout=600, open_doors=True)
    try:
        # solver.operate_loc walks to the blocked coffin tile itself and
        # burns full timeouts; stand on a walkable neighbour and click.
        # (C.approach_loc skips the tile we already stand on and can
        # loop its 25 s walk_to on every candidate.)
        coffin = C.find_loc(bot, "shutghostcoffin")
        cx, cz, clevel, cloc = coffin
        neigh = sorted(
            [(cx + dx, cz + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
            key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
        )
        for nx, nz in neigh:
            try:
                bot.approach(nx, nz, level=clevel, radius=0, timeout=30, open_doors=True)
                break
            except (Unreachable, TimeoutError):
                continue
        bot.op_loc(1, cx, cz, cloc)
        bot.pump(10)
    except (Unreachable, TimeoutError):
        pass
    # Same blocked-tile trap for the skull use: neighbour-stand, then click.
    altar = C.find_loc(bot, "restless_ghost_altar", "shutghostcoffin")
    ax, az, alevel, aloc = altar
    for nx, nz in sorted(
        [(ax + dx, az + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1) if dx or dz],
        key=lambda tile: abs(tile[0] - bot.world.x) + abs(tile[1] - bot.world.z),
    ):
        try:
            bot.approach(nx, nz, level=alevel, radius=0, timeout=30, open_doors=True)
            break
        except (Unreachable, TimeoutError):
            continue
    C.use_held_on_loc(bot, ax, az, aloc, solver.obj_id("ghostskull"))
    bot.pump(10)
    try:
        _wear(bot, "amulet_of_strength")
    except Exception:
        pass
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
