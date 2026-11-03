"""Requirement check and completion: Black Knight's Fortress.

Server truth: content/scripts/quests/quest_blackknight/scripts/quest_blackknight.rs2
(Sir Amik dialogue: areas/area_falador/scripts/sir_amik_varze.rs2).

Sequence: start at Sir Amik (%spy 0->1), disguise (iron chainbody +
bronze med helm worn), south door in, banquet door (guard lets us
through on "going in anyway"), inner door, listen at the grill
(%spy 1->2), upstairs push the secret wall, use a normal cabbage on
the hole (%spy 2->3), report back to Sir Amik for 2500 coins.
"""

import os

from lifescape import save as save_mod
from lifescape.client import Unreachable
from lifescape.waits import wait_ticks

from gamedata import quest_requirements
import gamedata as knowledge
from scripts import randomevents
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

QUEST = quest_requirements.by_key("blackknight")

SPY = knowledge.varp_id_by_name("spy")
STARTED = 1
LISTENED = 2
SABOTAGED = 3
COMPLETE = 4

# Save-backed stage: fresh logins are blind to live varps (spy is not
# transmitted), so resume gates consult the persisted stage too.
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or ""
    if not account:
        return {}
    try:
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _stage(bot):
    return max(bot.world.varps.get(SPY, 0), _SAVED_VARPS.get(SPY, 0))


def _count(bot, name):
    obj = solver.obj_id(name)
    carried = bot.inventory().get(obj, 0)
    worn_com = knowledge.com_id("wornitems:wear")
    for held, _count in bot.world.inv.get(worn_com, {}).values():
        if held == obj:
            carried += _count
    return carried

GRILL_LEVEL = 0  # "Grate 3025 3508 0" header in quest_blackknight.rs2
HOLE_LEVEL = 1  # "Hole 3031 3508 1" header in quest_blackknight.rs2
# Reach tables (engine ReachStrategy.ts, verified against the packed map):
# - witchgrill is a straight wall decor (shape 5): only its own tile.
# - bkfortressdoor3 is a straight wall facing EAST: west is never valid;
#   click from the north/south tile (x == door.x also skips the aggro).
# - bksecretdoor faces SOUTH with the hole room to its south: walk onto
#   its own tile (the north side stays open) and Push from there.
GRILL_STAND = (3025, 3507, 0)
DOOR1_STAND = (3016, 3513, 0)
DOOR2_STAND = (3019, 3515, 0)
DOOR3_STAND = (3025, 3512, 1)
SECRET_STAND = (3030, 3510, 1)
FORTRESS_GATE = (3016, 3515, 0)
BANQUET_HALL = (3028, 3513, 0)


def _worn_count(bot, name):
    obj = solver.obj_id(name)
    worn_com = knowledge.com_id("wornitems:wear")
    return sum(count for held, count in bot.world.inv.get(worn_com, {}).values() if held == obj)


def _wear(bot, name):
    """Wear a disguise piece, verifying the server applied it.

    A door click in the same tick as the wear sees the old state and the
    guard refuses entry, so poll the worn container before carrying on.
    """
    if _worn_count(bot, name) > 0:
        return
    obj = solver.obj_id(name)
    for _ in range(6):
        try:
            slot = solver.find_item_slot(bot, obj)
        except solver.SolverError:
            break
        com, _slots = bot.backpack()
        bot.op_held(2, obj, slot, com)
        bot.pump(10)
        if _worn_count(bot, name) > 0:
            return
    if _worn_count(bot, name) < 1:
        raise solver.SolverError("cannot wear %s at %s" % (name, bot.position))


def _go_level(bot, level, prefer=(), avoid=()):
    """Ride any ladder/stairs between the fortress floors.

    Tries every same-level ladder/stair by distance: the banquet ladder
    (3021,3510) only answers from its east tile and that tile is not
    walkable, so no approach can stand a click there. `prefer` names
    (x, z) tiles tried first, `avoid` names broken ones skipped: the way
    up is the hall tower ladder (3022,3518), not the west spiral stairs.
    """
    from scripts.tutorial import common as tutorial_common

    for _ in range(6):
        if bot.world.level == level:
            return
        names = ("laddertop", "spiralstairstop", "stairstop") if bot.world.level > level else ("ladder", "spiralstairs", "stairs")
        cands = []
        seen = dict(bot.world.static_locs)
        seen.update(getattr(bot.world, "static_actions", None) or {})
        seen.update(bot.world.locs)
        for (x, z, lvl), loc in seen.items():
            if loc is None or lvl != bot.world.level:
                continue
            label = bot.loctypes["loc"][loc[0]]["name"] if loc[0] < len(bot.loctypes["loc"]) else ""
            if any(name in label for name in names):
                cands.append((abs(x - bot.world.x) + abs(z - bot.world.z), x, z, lvl, loc[0]))
        cands.sort()
        if avoid:
            cands = [cand for cand in cands if (cand[1], cand[2]) not in avoid]
        if prefer:
            cands.sort(key=lambda cand: (prefer.index((cand[1], cand[2])) if (cand[1], cand[2]) in prefer else len(prefer), cand[0]))
        print("go_level wants %d from %s: %s" % (
            level, bot.position, [(x, z) for _, x, z, _, _ in cands[:4]]), flush=True)
        climbed = False
        for _, x, z, lvl, loc in cands[:4]:
            print("go_level trying %d,%d from %s" % (x, z, bot.position), flush=True)
            try:
                bot.approach(x, z, level=lvl, radius=2, timeout=1500, open_doors=True)
            except (Unreachable, TimeoutError):
                print("go_level %d,%d unreachable" % (x, z), flush=True)
                continue
            if bot.ride_transition_at(x, z, lvl):
                wait_ticks(bot, 5)
                climbed = True
                break
            print("go_level %d,%d ride refused, clicking" % (x, z), flush=True)
            try:
                bot.op_loc(1, x, z, loc)
                wait_ticks(bot, 5)
            except Exception:
                continue
            if bot.world.level == level:
                climbed = True
                break
        if not climbed:
            raise solver.SolverError("no ladder answers from %s" % (bot.position,))
    raise solver.SolverError("cannot reach level %d from %s" % (level, bot.position))


def _listen_at_grill(bot):
    # The kitchen is NOT reachable from the banquet hall: the direct wall
    # door (3028,3510) is permanently locked ("It's locked") and the
    # planner stalls on it. Always loop through door2's west side first.
    if not (bot.world.level == 0 and (bot.world.x, bot.world.z) == (3019, 3515)):
        bot.walk_to(3019, 3515, 0, timeout=9000, open_doors=True)
    for _ in range(4):
        if bot.world.level != GRILL_LEVEL:
            _go_level(bot, GRILL_LEVEL)
            continue
        try:
            solver.operate_loc(bot, ("witchgrill",), 1, timeout=6000, stand=GRILL_STAND)
            break
        except (Unreachable, TimeoutError):
            continue
    else:
        raise solver.SolverError("cannot reach the fortress grill from %s" % (bot.position,))
    solver.finish_dialog_like(bot, ["cabbage", "potion", "draynor"])


def _go_to_fortress(bot):
    if bot.world.level == FORTRESS_GATE[2] and abs(bot.world.x - FORTRESS_GATE[0]) + abs(bot.world.z - FORTRESS_GATE[1]) <= 20:
        return
    bot.intend("walking to the Black Knights' Fortress")
    # Stage south of the fortress first: the west outer doors never open
    # and the planner burns minutes on them before replanning around.
    # Detour around the Champions' Guild first: the planner cuts through
    # its interior (doors assumed open) but championdoor needs 32 live QP
    # and refuses on a fresh login, stalling the walk the same way.
    try:
        if bot.world.level == 0 and (bot.world.x > 3175 or bot.world.z < 3355):
            bot.walk_to(3175, 3355, 0, timeout=9000, open_doors=True)
    except (Unreachable, TimeoutError):
        pass
        if bot.world.level == 0 and bot.world.z < 3512:
            bot.walk_to(3016, 3500, 0, timeout=4000, open_doors=True)
    except (Unreachable, TimeoutError):
        pass
    try:
        bot.walk_to(FORTRESS_GATE[0], FORTRESS_GATE[1], FORTRESS_GATE[2], timeout=2000, open_doors=True)
    except (Unreachable, TimeoutError):
        # The gate tile itself hosts the open door half the time (and a
        # wall when it is shut): standing next to it is arriving.
        if abs(bot.world.x - FORTRESS_GATE[0]) + abs(bot.world.z - FORTRESS_GATE[1]) > 5:
            raise


def _use_cabbage_on_hole(bot):
    """Use the cabbage on the hole from a reach-valid stand.

    approach_loc stops within radius 2, which includes diagonal tiles the
    server refuses (reachRectangle1 is orthogonal only): name the stands.
    """
    from scripts.tutorial import common as tutorial_common
    x, z, level, loc = tutorial_common.find_loc(bot, "blackknighthole")
    seen = len(bot.world.messages)
    for stand_x, stand_z in ((3031, 3506), (3030, 3507), (3031, 3508)):
        try:
            bot.walk_to(stand_x, stand_z, level, timeout=3000, open_doors=True)
        except (Unreachable, TimeoutError):
            continue
        tutorial_common.use_held_on_loc(bot, x, z, loc, solver.obj_id("cabbage"))
        wait_ticks(bot, 10)
        if any("drop a cabbage" in text.lower() for text in bot.world.messages[seen:]):
            return
        seen = len(bot.world.messages)
    raise Unreachable("cabbage use refused at the hole from %s" % (bot.position,))


def _sabotage_potion(bot):
    _go_level(bot, HOLE_LEVEL)
    try:
        _use_cabbage_on_hole(bot)
    except Unreachable:
        # Hole room hides behind the secret wall: step onto the wall tile
        # (its north side stays open) and push from there, then retry.
        solver.operate_loc(bot, ("bksecretdoor",), 1, timeout=6000, stand=SECRET_STAND)
        _use_cabbage_on_hole(bot)
    solver.finish_dialog_like(bot, ["sabotaged", "successfully"])


def _in_banquet_hall(bot):
    return bot.world.level == 0 and bot.world.x > 3020 and bot.world.z >= 3512


def _past_south_door(bot):
    """True once past bkfortressdoor1 (gate, corridor, kitchen, banquet)."""
    return bot.world.x > 3016 or (bot.world.x == 3016 and bot.world.z > 3514)


def _ensure_inside(bot):
    """Walk into the banquet hall, entering the fortress when still outside."""
    if _in_banquet_hall(bot):
        return
    randomevents.greet(bot)
    _go_level(bot, 0)
    if _in_banquet_hall(bot):
        return
    # The doors shut again behind the bot and an approach can wander back
    # outside mid-loop, so the south door is re-checked every round: a
    # one-shot check before the loop strands the bot outside for good.
    for _ in range(8):
        if _in_banquet_hall(bot):
            return
        print("banquet round from %s" % (bot.position,), flush=True)
        if not _past_south_door(bot):
            solver.operate_loc(bot, ("bkfortressdoor1",), 1, timeout=15000, stand=DOOR1_STAND)
            if bot.world.modal and bot.world.modal[0] != "main":
                raise solver.SolverError("fortress door refused entry: wear iron_chainbody + bronze_med_helm")
            # A wilderness warning (plain main window) is informational, not a
            # refusal: dismiss it instead of aborting the infiltration.
            bot.close_modal()
            bot.pump(5)
            continue
        solver.operate_loc(bot, ("bkfortressdoor2",), 1, timeout=6000, stand=DOOR2_STAND)
        solver.finish_dialog_like(bot, ["going in", "don't care"])
        # The banquet door only opens on a guard chat and shuts again
        # fast: blast the two tiles through raw (no planning dither eats
        # the window), then walk the hall. A planned walk to the banquet
        # usually arrives after it shut and grinds the closed door.
        try:
            bot.walk_steps([(3020, 3515), (3021, 3515)], timeout_ticks=30)
        except (Unreachable, TimeoutError):
            pass
        if _in_banquet_hall(bot):
            return
        try:
            bot.walk_to(BANQUET_HALL[0], BANQUET_HALL[1], BANQUET_HALL[2], timeout=1500, open_doors=True)
        except (Unreachable, TimeoutError):
            pass
        if _in_banquet_hall(bot):
            return
        wait_ticks(bot, 10)
    raise solver.SolverError("banquet door never opened at %s" % (bot.position,))


def _leave_L1_east(bot):
    """Walk out of the fortress east side after the sabotage.

    Validated live, tile by tile: the secret wall only teleports south,
    but its north side was never walled, so walking north across its tile
    just works; the door-3 click from the east lands on the door tile,
    whose west side was never walled either.
    """
    if bot.world.level != 1 or bot.world.x < 3024:
        return
    try:
        bot.walk_to(3030, 3511, 1, timeout=3000, open_doors=True)
    except (Unreachable, TimeoutError):
        bot.walk_to(3030, 3509, 1, timeout=3000, open_doors=True)
        bot.walk_to(3030, 3510, 1, timeout=1500, open_doors=True)
        bot.walk_to(3030, 3511, 1, timeout=1500, open_doors=True)
    solver.operate_loc(bot, ("bkfortressdoor3",), 1, timeout=6000, stand=(3026, 3511, 1))
    if not bot.step_to(3024, 3511, timeout_ticks=120):
        raise solver.SolverError("inner door shut before stepping through at %s" % (bot.position,))
    _go_level(bot, 0)


def _ensure_L1_east(bot):
    """Stand east of the inner door (hole-room side), entering if needed."""
    if bot.world.level == 1 and bot.world.x >= 3026 and 3505 <= bot.world.z <= 3513:
        return
    _ensure_inside(bot)
    randomevents.greet(bot)
    # Up via the hall tower ladder (3022,3518): the banquet ladder only
    # answers from its east tile and that tile is not walkable, so no
    # approach can ever stand a click there.
    _go_level(bot, 1, prefer=[(3022, 3518)], avoid=[(3021, 3510)])
    solver.operate_loc(bot, ("bkfortressdoor3",), 1, timeout=6000, stand=DOOR3_STAND)


def solve(bot):
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    if _stage(bot) == COMPLETE:
        return
    # The disguise stays worn; the cabbage is eaten at stage 3, so only
    # fresh runs need one in the pack.
    if _count(bot, "iron_chainbody") < 1 or _count(bot, "bronze_med_helm") < 1:
        raise solver.SolverError("no disguise (chainbody + med helm) at %s" % (bot.position,))
    if _stage(bot) < SABOTAGED and _count(bot, "cabbage") < 1:
        raise solver.SolverError("no cabbage for the hole at %s" % (bot.position,))
    if _stage(bot) < STARTED:
        solver.talk_like(bot, "sir_amik_varze", ["quest", "laugh", "give it a go"])
    if _stage(bot) < SABOTAGED:
        # Greet event hosts before the long walk: an ignored host fail-teleports
        # the bot mid-route and every later step fails far from the fortress.
        # Past the sabotage the endgame (leave, report) starts wherever the
        # bot stands: walking back to the fortress here ping-pongs with Amik.
        randomevents.greet(bot)
        _go_to_fortress(bot)
        randomevents.greet(bot)
        _wear(bot, "iron_chainbody")
        _wear(bot, "bronze_med_helm")
        if _stage(bot) < LISTENED:
            _ensure_inside(bot)
            # The grill listens from the kitchen north of the banquet hall; the
            # inner door and the secret wall are upstairs.
            _listen_at_grill(bot)
            _ensure_L1_east(bot)
        _ensure_L1_east(bot)
        solver.solve_strange_box(bot)
        _sabotage_potion(bot)
    if _stage(bot) < COMPLETE:
        _leave_L1_east(bot)
    solver.solve_strange_box(bot)
    solver.talk(bot, "sir_amik_varze")


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
